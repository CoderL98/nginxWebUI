//! Sync groups stored on the panel: active-active or primary-standby.
//!
//! A successful site/cert change or complete manual sync on a member can be
//! replicated (records + cert files + `nginx -t` + reload) to other *online*
//! members. A wave id prevents those replicas from bouncing forever.
//!
//! Primary-standby: only the current primary's writes propagate. If the
//! primary is offline (~90s heartbeat; the local panel is always online), a
//! healthy standby is promoted. This keeps configs in sync and gives load
//! balancers a health endpoint; it does not move virtual IPs.

use crate::api::{fail, ok, ok_empty, Ctx, Reply, Req};
use crate::store::{now_ms, str_of, Db};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const MODE_AA: &str = "active-active";
pub const MODE_PS: &str = "primary-standby";
const WAVE_TTL_MS: u64 = 10 * 60 * 1000;
const FAILOVER_EVERY: Duration = Duration::from_secs(15);

thread_local! {
    static CURRENT_WAVE: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(crate) struct WaveGuard {
    previous: Option<String>,
}

impl Drop for WaveGuard {
    fn drop(&mut self) {
        CURRENT_WAVE.with(|slot| {
            *slot.borrow_mut() = self.previous.take();
        });
    }
}

/// Mark the current thread as applying a given wave (empty id is ignored).
pub(crate) fn enter_wave(wave_id: &str) -> WaveGuard {
    let previous = CURRENT_WAVE.with(|slot| slot.borrow().clone());
    if !wave_id.is_empty() {
        CURRENT_WAVE.with(|slot| {
            *slot.borrow_mut() = Some(wave_id.to_string());
        });
    }
    WaveGuard { previous }
}

pub fn current_wave() -> Option<String> {
    CURRENT_WAVE.with(|slot| slot.borrow().clone().filter(|s| !s.is_empty()))
}

fn waves() -> &'static Mutex<HashMap<String, u64>> {
    static WAVES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    WAVES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn remember_wave(wave_id: &str) {
    if wave_id.is_empty() {
        return;
    }
    let mut map = waves().lock().unwrap();
    let now = now_ms();
    map.retain(|_, at| now.saturating_sub(*at) < WAVE_TTL_MS);
    map.insert(wave_id.to_string(), now);
}

pub fn wave_seen(wave_id: &str) -> bool {
    if wave_id.is_empty() {
        return false;
    }
    let mut map = waves().lock().unwrap();
    let now = now_ms();
    map.retain(|_, at| now.saturating_sub(*at) < WAVE_TTL_MS);
    map.contains_key(wave_id)
}

/// True when this wave should start a new replication (not already running).
pub fn begin_wave(wave_id: &str) -> bool {
    if wave_id.is_empty() {
        return true;
    }
    if wave_seen(wave_id) || current_wave().as_deref() == Some(wave_id) {
        return false;
    }
    remember_wave(wave_id);
    true
}

fn is_local(id: &str) -> bool {
    id.is_empty() || id == "local"
}

fn node_online(ctx: &Ctx, id: &str) -> bool {
    if is_local(id) {
        return true;
    }
    let store = ctx.store.lock().unwrap();
    match store.db.get("node", id) {
        Some(row) => {
            let revoked = str_of(&row, "revoked") == "true"
                || row.get("revoked").and_then(|v| v.as_bool()) == Some(true);
            !revoked && ctx.hub.is_online(id)
        }
        None => false,
    }
}

fn known_member(db: &Db, id: &str) -> bool {
    is_local(id) || db.get("node", id).is_some()
}

fn members_of(row: &Value) -> Vec<String> {
    match row.get("members") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split([',', ' '])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn mode_of(row: &Value) -> String {
    let mode = str_of(row, "mode");
    if mode == MODE_PS {
        MODE_PS.to_string()
    } else {
        MODE_AA.to_string()
    }
}

pub fn public_group(row: &Value, ctx: &Ctx) -> Value {
    let id = str_of(row, "id");
    let mode = mode_of(row);
    let members = members_of(row);
    let primary = if mode == MODE_PS {
        let p = str_of(row, "primary");
        if p.is_empty() {
            members.first().cloned().unwrap_or_else(|| "local".into())
        } else {
            p
        }
    } else {
        String::new()
    };
    let member_views: Vec<Value> = members
        .iter()
        .map(|mid| {
            json!({
                "id": mid,
                "online": node_online(ctx, mid),
                "primary": mode == MODE_PS && mid == &primary,
                "shouldReceiveTraffic": should_receive(ctx, row, mid),
            })
        })
        .collect();
    json!({
        "id": id,
        "name": str_of(row, "name"),
        "mode": mode,
        "members": members,
        "primary": primary,
        "memberViews": member_views,
        "lastSync": row.get("lastSync").cloned().unwrap_or(Value::Null),
        "lastFailover": row.get("lastFailover").cloned().unwrap_or(Value::Null),
        "advertise": row.get("advertise").cloned().unwrap_or(Value::Null),
    })
}

pub fn should_receive(ctx: &Ctx, group: &Value, node_id: &str) -> bool {
    let members = members_of(group);
    if !members.iter().any(|m| m == node_id) {
        return false;
    }
    if !node_online(ctx, node_id) {
        return false;
    }
    if mode_of(group) == MODE_PS {
        let primary = str_of(group, "primary");
        let primary = if primary.is_empty() {
            members.first().map(String::as_str).unwrap_or("local")
        } else {
            primary.as_str()
        };
        return node_id == primary;
    }
    true
}

fn normalize_group(db: &Db, mut row: Value) -> Result<Value, String> {
    let name = str_of(&row, "name").trim().to_string();
    if name.is_empty() {
        return Err("nameRequired".into());
    }
    let mode = mode_of(&row);
    let mut members = members_of(&row);
    members.sort();
    members.dedup();
    if members.len() < 2 {
        return Err("groupMembersRequired".into());
    }
    for id in &members {
        if !known_member(db, id) {
            return Err(format!("unknownNode:{id}"));
        }
    }
    let mut primary = str_of(&row, "primary");
    if mode == MODE_PS {
        if primary.is_empty() || !members.iter().any(|m| m == &primary) {
            primary = members
                .iter()
                .find(|m| is_local(m))
                .cloned()
                .unwrap_or_else(|| members[0].clone());
        }
    } else {
        primary.clear();
    }
    if let Value::Object(map) = &mut row {
        map.insert("name".into(), name.into());
        map.insert("mode".into(), mode.into());
        map.insert("members".into(), json!(members));
        map.insert("primary".into(), primary.into());
    }
    Ok(row)
}

fn list_groups(ctx: &Ctx) -> Vec<Value> {
    let rows = {
        let store = ctx.store.lock().unwrap();
        store.db.rows("syncGroup").to_vec()
    };
    rows.iter().map(|row| public_group(row, ctx)).collect()
}

fn groups_for_node(db: &Db, node_id: &str) -> Vec<Value> {
    db.rows("syncGroup")
        .iter()
        .filter(|row| members_of(row).iter().any(|m| m == node_id))
        .cloned()
        .collect()
}

fn persist_group_field(ctx: &Ctx, id: &str, key: &str, value: Value) {
    let mut store = ctx.store.lock().unwrap();
    if let Some(mut row) = store.db.get("syncGroup", id) {
        if let Value::Object(map) = &mut row {
            map.insert(key.to_string(), value);
        }
        store.db.upsert("syncGroup", row);
        let _ = store.flush();
    }
}

pub fn promote(
    ctx: &Ctx,
    group_id: &str,
    node_id: &str,
    who: &str,
    reason: &str,
) -> Result<Value, String> {
    let store = ctx.store.lock().unwrap();
    let Some(row) = store.db.get("syncGroup", group_id) else {
        return Err("not found".into());
    };
    if mode_of(&row) != MODE_PS {
        return Err("notPrimaryStandby".into());
    }
    let members = members_of(&row);
    if !members.iter().any(|m| m == node_id) {
        return Err("unknownNode".into());
    }
    drop(store);
    if !node_online(ctx, node_id) {
        return Err("nodeOfflineErr".into());
    }
    let mut store = ctx.store.lock().unwrap();
    let Some(mut row) = store.db.get("syncGroup", group_id) else {
        return Err("not found".into());
    };
    let from = str_of(&row, "primary");
    if from == node_id {
        drop(store);
        return Ok(public_group(&row, ctx));
    }
    if let Value::Object(map) = &mut row {
        map.insert("primary".into(), json!(node_id));
        map.insert(
            "lastFailover".into(),
            json!({
                "at": now_ms().to_string(),
                "from": from,
                "to": node_id,
                "reason": reason,
            }),
        );
    }
    store.db.upsert("syncGroup", row.clone());
    crate::api::record_user_operate(
        &mut store,
        who,
        "group.promote",
        group_id,
        true,
        &format!("{from}->{node_id} {reason}"),
    );
    store.flush().map_err(|err| format!("saveFailed: {err}"))?;
    drop(store);
    Ok(public_group(&row, ctx))
}

/// Scan primary-standby groups and promote a healthy standby when the
/// current primary is offline.
pub fn check_failover(ctx: &Ctx) -> Vec<Value> {
    let groups = {
        let store = ctx.store.lock().unwrap();
        store
            .db
            .rows("syncGroup")
            .iter()
            .filter(|row| mode_of(row) == MODE_PS)
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut events = Vec::new();
    for row in groups {
        let id = str_of(&row, "id");
        let members = members_of(&row);
        let primary = {
            let p = str_of(&row, "primary");
            if p.is_empty() {
                members.first().cloned().unwrap_or_else(|| "local".into())
            } else {
                p
            }
        };
        if node_online(ctx, &primary) {
            continue;
        }
        let Some(standby) = members
            .iter()
            .filter(|m| *m != &primary)
            .find(|m| node_online(ctx, m))
            .cloned()
        else {
            continue;
        };
        match promote(ctx, &id, &standby, "system", "primaryOffline") {
            Ok(view) => events.push(view),
            Err(err) => crate::applog::warn(format!("failover {id}: {err}")),
        }
    }
    events
}

pub fn spawn_failover(ctx: Arc<Ctx>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(FAILOVER_EVERY);
        let _ = check_failover(&ctx);
    });
}

fn replicate_targets(ctx: &Ctx, group: &Value, source: &str) -> (Vec<String>, Vec<Value>) {
    let members = members_of(group);
    let mode = mode_of(group);
    if mode == MODE_PS {
        let primary = str_of(group, "primary");
        let primary = if primary.is_empty() {
            members.first().map(String::as_str).unwrap_or("local")
        } else {
            primary.as_str()
        };
        if source != primary {
            return (Vec::new(), Vec::new());
        }
    }
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for member in &members {
        if member == source {
            continue;
        }
        if node_online(ctx, member) {
            targets.push(member.clone());
        } else {
            skipped.push(json!({
                "target": member,
                "success": false,
                "msg": "nodeOfflineErr",
                "skipped": true,
            }));
        }
    }
    (targets, skipped)
}

fn run_replicate(
    ctx: &Arc<Ctx>,
    source: &str,
    site_ids: &[String],
    wave_id: &str,
    extra_skip: &[String],
) {
    if !begin_wave(wave_id) {
        return;
    }
    let _guard = enter_wave(wave_id);
    let groups = {
        let store = ctx.store.lock().unwrap();
        groups_for_node(&store.db, source)
    };
    if groups.is_empty() {
        return;
    }
    let bundle = match crate::sync::collect(ctx, source, site_ids) {
        Ok(b) => b.with_wave(wave_id.to_string()),
        Err(err) => {
            crate::applog::warn(format!("group sync collect from {source}: {err}"));
            return;
        }
    };
    for group in groups {
        let gid = str_of(&group, "id");
        let (targets, mut results) = replicate_targets(ctx, &group, source);
        let targets: Vec<String> = targets
            .into_iter()
            .filter(|t| !extra_skip.iter().any(|s| s == t))
            .collect();
        if targets.is_empty() && results.is_empty() {
            continue;
        }
        let applied = crate::sync::apply_to_targets(ctx, source, &bundle, &targets);
        results.extend(applied);
        persist_group_field(
            ctx,
            &gid,
            "lastSync",
            json!({
                "waveId": wave_id,
                "at": now_ms().to_string(),
                "source": source,
                "results": results,
            }),
        );
    }
}

/// After a successful local or remote mutation, replicate to group peers.
pub fn after_success(ctx: &Arc<Ctx>, source: &str, site_ids: Vec<String>, wave_id: Option<String>) {
    if site_ids.is_empty() {
        return;
    }
    if current_wave().is_some() {
        return;
    }
    if let Some(id) = wave_id.as_deref() {
        if !id.is_empty() && wave_seen(id) {
            return;
        }
    }
    let source = if source.is_empty() {
        "local".to_string()
    } else {
        source.to_string()
    };
    {
        let store = ctx.store.lock().unwrap();
        if groups_for_node(&store.db, &source).is_empty() {
            return;
        }
    }
    let ctx = Arc::clone(ctx);
    std::thread::spawn(move || {
        let wave = wave_id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(crate::store::new_id);
        run_replicate(&ctx, &source, &site_ids, &wave, &[]);
    });
}

/// After a manual complete sync, also fan out to other group members that
/// were not in the original target list, using the same wave id.
pub fn after_manual_sync(
    ctx: &Arc<Ctx>,
    source: &str,
    site_ids: &[String],
    wave_id: &str,
    results: &[Value],
) {
    let already: Vec<String> = results
        .iter()
        .filter_map(|r| {
            r.get("target")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    if current_wave().is_some() {
        return;
    }
    let source = if source.is_empty() {
        "local".to_string()
    } else {
        source.to_string()
    };
    let site_ids = site_ids.to_vec();
    let wave = if wave_id.is_empty() {
        crate::store::new_id()
    } else {
        wave_id.to_string()
    };
    let ctx = Arc::clone(ctx);
    std::thread::spawn(move || {
        run_replicate(&ctx, &source, &site_ids, &wave, &already);
    });
}

/// Watch a remote/local cert job and, when it succeeds, replicate sites that
/// use that certificate.
pub fn after_cert_job(ctx: &Arc<Ctx>, source: &str, cert_id: &str) {
    if current_wave().is_some() {
        return;
    }
    let source_key = if source.is_empty() { "local" } else { source };
    {
        let store = ctx.store.lock().unwrap();
        if groups_for_node(&store.db, source_key).is_empty() {
            return;
        }
    }
    let ctx = Arc::clone(ctx);
    let source = if source.is_empty() {
        "local".to_string()
    } else {
        source.to_string()
    };
    let cert_id = cert_id.to_string();
    std::thread::spawn(move || {
        if source == "local" {
            for _ in 0..600 {
                if !crate::acme::job_snapshot().running {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            let job = crate::acme::job_snapshot();
            if job.success != Some(true) {
                return;
            }
        } else {
            let mut ok = false;
            for _ in 0..600 {
                match ctx.hub.dispatch(
                    &source,
                    "cert.progress",
                    json!({ "query": {}, "body": {} }),
                    crate::registry::RPC_WAIT,
                ) {
                    Ok(value) => {
                        let value = crate::rpc::normalize_result("cert.progress", value);
                        let obj = value.get("obj").cloned().unwrap_or(value);
                        let running = obj
                            .get("running")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let success = obj.get("success").and_then(|v| v.as_bool());
                        if !running {
                            ok = success == Some(true);
                            break;
                        }
                    }
                    Err(_) => break,
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if !ok {
                return;
            }
        }
        let site_ids = sites_for_cert(&ctx, &source, &cert_id);
        if site_ids.is_empty() {
            return;
        }
        let wave = crate::store::new_id();
        run_replicate(&ctx, &source, &site_ids, &wave, &[]);
    });
}

fn sites_for_cert(ctx: &Arc<Ctx>, source: &str, cert_id: &str) -> Vec<String> {
    let collect = |sites: &[Value], certs: &[Value]| -> Vec<String> {
        let cert = certs.iter().find(|c| str_of(c, "id") == cert_id);
        let names: Vec<String> = if let Some(cert) = cert {
            ["id", "certName", "domain"]
                .iter()
                .map(|k| str_of(cert, k))
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![cert_id.to_string()]
        };
        sites
            .iter()
            .filter(|s| {
                let name = str_of(s, "certName");
                let domain = str_of(s, "domain");
                names
                    .iter()
                    .any(|n| n == &name || n == &domain || n == &str_of(s, "cert"))
            })
            .map(|s| str_of(s, "id"))
            .filter(|s| !s.is_empty())
            .collect()
    };
    if source == "local" {
        let store = ctx.store.lock().unwrap();
        return collect(store.db.rows("site"), store.db.rows("cert"));
    }
    match ctx.hub.dispatch(
        source,
        "site.list",
        json!({ "query": {}, "body": {} }),
        crate::registry::RPC_WAIT,
    ) {
        Ok(sites_reply) => {
            let sites_reply = crate::rpc::normalize_result("site.list", sites_reply);
            let sites = sites_reply
                .get("obj")
                .and_then(|o| {
                    o.as_array()
                        .cloned()
                        .or_else(|| o.get("list").and_then(|l| l.as_array().cloned()))
                })
                .unwrap_or_default();
            let certs = ctx
                .hub
                .dispatch(
                    source,
                    "cert.list",
                    json!({ "query": {}, "body": {} }),
                    crate::registry::RPC_WAIT,
                )
                .ok()
                .map(|v| crate::rpc::normalize_result("cert.list", v))
                .and_then(|v| {
                    v.get("obj")
                        .and_then(|o| o.get("certs"))
                        .and_then(|l| l.as_array().cloned())
                })
                .unwrap_or_default();
            collect(&sites, &certs)
        }
        Err(_) => Vec::new(),
    }
}

fn field(body: &Value, key: &str) -> String {
    str_of(body, key)
}

pub fn handle(ctx: &Arc<Ctx>, action: &str, req: &Req) -> Reply {
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    };
    match action {
        "list" | "status" => ok(json!({ "groups": list_groups(ctx) })),
        "save" => {
            let mut store = ctx.store.lock().unwrap();
            match normalize_group(&store.db, body) {
                Ok(row) => {
                    let id = store.db.upsert("syncGroup", row);
                    crate::api::record_user_operate(
                        &mut store,
                        "admin",
                        "group.save",
                        &id,
                        true,
                        "",
                    );
                    if let Err(err) = store.flush() {
                        return fail(&format!("saveFailed: {err}"));
                    }
                    let saved = store.db.get("syncGroup", &id).unwrap_or(Value::Null);
                    drop(store);
                    ok(public_group(&saved, ctx))
                }
                Err(err) => fail(&err),
            }
        }
        "del" => {
            let id = field(&body, "id");
            if id.is_empty() {
                return fail("badRequest");
            }
            let mut store = ctx.store.lock().unwrap();
            store.db.delete("syncGroup", &id);
            crate::api::record_user_operate(&mut store, "admin", "group.del", &id, true, "");
            if let Err(err) = store.flush() {
                return fail(&format!("saveFailed: {err}"));
            }
            ok_empty()
        }
        "promote" => {
            let id = field(&body, "id");
            let node = field(&body, "nodeId");
            if id.is_empty() || node.is_empty() {
                return fail("badRequest");
            }
            match promote(ctx, &id, &node, "admin", "manual") {
                Ok(view) => ok(view),
                Err(err) => fail(&err),
            }
        }
        _ => fail("not found"),
    }
}

/// Unauthenticated health for load balancers.
/// `node_id` empty means the local panel node.
pub fn healthz(ctx: &Ctx, group_id: &str, node_id: &str) -> Reply {
    let node_id = if node_id.is_empty() { "local" } else { node_id };
    let store = ctx.store.lock().unwrap();
    let Some(row) = store.db.get("syncGroup", group_id) else {
        return crate::api::json_reply(
            404,
            &json!({ "ok": false, "msg": "not found", "groupId": group_id }),
        );
    };
    drop(store);
    let receive = should_receive(ctx, &row, node_id);
    let body = json!({
        "ok": receive,
        "groupId": group_id,
        "nodeId": node_id,
        "mode": mode_of(&row),
        "primary": str_of(&row, "primary"),
        "online": node_online(ctx, node_id),
        "shouldReceiveTraffic": receive,
    });
    if receive {
        crate::api::json_reply(200, &body)
    } else {
        crate::api::json_reply(503, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn ctx_at(label: &str) -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-groups-{label}-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let store = Store::open(&dir.join("data"));
        Arc::new(Ctx::standalone(dir, store))
    }

    fn add_node(ctx: &Ctx, name: &str) -> String {
        let mut store = ctx.store.lock().unwrap();
        crate::registry::create_node(&mut store.db, name)
            .map(|row| str_of(&row, "id"))
            .unwrap()
    }

    fn save_group(ctx: &Ctx, name: &str, mode: &str, members: &[&str], primary: &str) -> Value {
        let mut store = ctx.store.lock().unwrap();
        let row = normalize_group(
            &store.db,
            json!({
                "name": name,
                "mode": mode,
                "members": members,
                "primary": primary,
            }),
        )
        .unwrap();
        let id = store.db.upsert("syncGroup", row);
        store.flush().unwrap();
        store.db.get("syncGroup", &id).unwrap()
    }

    #[test]
    fn wave_guard_blocks_a_second_start() {
        let id = crate::store::new_id();
        assert!(begin_wave(&id));
        assert!(!begin_wave(&id));
        assert!(wave_seen(&id));
        let _g = enter_wave(&id);
        assert_eq!(current_wave().as_deref(), Some(id.as_str()));
    }

    #[test]
    fn active_active_skips_offline_and_does_not_loop() {
        let ctx = ctx_at("aa");
        let peer = add_node(&ctx, "peer");
        let group = save_group(&ctx, "aa", MODE_AA, &["local", &peer], "");
        // peer was never touched: offline
        assert!(!node_online(&ctx, &peer));
        assert!(should_receive(&ctx, &group, "local"));
        assert!(!should_receive(&ctx, &group, &peer));
        let wave = crate::store::new_id();
        remember_wave(&wave);
        // a second run with the same wave must be a no-op
        assert!(!begin_wave(&wave));
        let (targets, skipped) = replicate_targets(&ctx, &group, "local");
        assert!(targets.is_empty(), "{targets:?}");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["msg"], "nodeOfflineErr");
    }

    #[test]
    fn primary_standby_promotes_when_primary_offline() {
        let ctx = ctx_at("ps");
        let a = add_node(&ctx, "a");
        let b = add_node(&ctx, "b");
        ctx.hub.touch(&b);
        let group = save_group(&ctx, "ps", MODE_PS, &[&a, &b], &a);
        assert_eq!(str_of(&group, "primary"), a);
        assert!(!node_online(&ctx, &a));
        assert!(node_online(&ctx, &b));
        let events = check_failover(&ctx);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["primary"], b);
        let store = ctx.store.lock().unwrap();
        let saved = store.db.get("syncGroup", &str_of(&group, "id")).unwrap();
        assert_eq!(str_of(&saved, "primary"), b);
        assert_eq!(
            str_of(saved.get("lastFailover").unwrap(), "reason"),
            "primaryOffline"
        );
    }

    #[test]
    fn healthz_status_codes() {
        let ctx = ctx_at("hz");
        let a = add_node(&ctx, "edge");
        ctx.hub.touch(&a);
        let aa = save_group(&ctx, "aa", MODE_AA, &["local", &a], "");
        let aa_id = str_of(&aa, "id");
        let reply = healthz(&ctx, &aa_id, "local");
        assert_eq!(reply.status, 200);
        let reply = healthz(&ctx, &aa_id, &a);
        assert_eq!(reply.status, 200);
        let missing = add_node(&ctx, "down");
        let reply = healthz(&ctx, &aa_id, &missing);
        assert_eq!(reply.status, 503); // not a member
        let ps = save_group(&ctx, "ps", MODE_PS, &["local", &a], "local");
        let ps_id = str_of(&ps, "id");
        assert_eq!(healthz(&ctx, &ps_id, "local").status, 200);
        assert_eq!(healthz(&ctx, &ps_id, &a).status, 503); // standby
        assert_eq!(healthz(&ctx, "no-such", "local").status, 404);
    }
}
