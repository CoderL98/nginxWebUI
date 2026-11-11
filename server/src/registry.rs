//! Outbound-agent registry: join tokens, long-lived credentials, job hub.

use crate::api::{fail, json_reply, ok, Ctx, Reply, Req};
use crate::store::{now_ms, str_of, Db};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use subtle::ConstantTimeEq;

pub const JOIN_TTL_MS: u64 = 24 * 60 * 60 * 1000;
pub const POLL_WAIT: Duration = Duration::from_secs(25);
pub const RPC_WAIT: Duration = Duration::from_secs(30);
pub const ONLINE_AFTER_MS: u64 = 90_000;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Mutex<Inner>>,
}

/// What a parked long-poll is woken with.
enum Wake {
    Job(JobWire),
    /// A newer poll for the same node took over (or the node was revoked).
    Superseded,
    Revoked,
}

struct Waiter {
    id: u64,
    tx: mpsc::Sender<Wake>,
}

struct Inner {
    queues: HashMap<String, VecDeque<JobWire>>,
    /// At most one parked poll per node: a newer poll supersedes the older
    /// one, so a stale connection never swallows a job and two hosts sharing
    /// one credential cannot silently split the work between them.
    waiters: HashMap<String, Waiter>,
    inflight: HashMap<String, mpsc::Sender<Result<Value, String>>>,
    last_seen: HashMap<String, u64>,
    revoked: HashSet<String>,
    /// last agent instance id seen per node, and when two different
    /// instances were last seen polling within the online window
    instances: HashMap<String, (String, u64)>,
    conflicts: HashMap<String, u64>,
    next_waiter: u64,
}

#[derive(Clone, Debug)]
pub struct JobWire {
    pub id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub enum Polled {
    Job(JobWire),
    Empty,
    Superseded,
}

/// Window in which polls from two different agent instances count as a
/// duplicated credential.
pub const CONFLICT_SHOW_MS: u64 = 10 * 60 * 1000;

impl Hub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                queues: HashMap::new(),
                waiters: HashMap::new(),
                inflight: HashMap::new(),
                last_seen: HashMap::new(),
                revoked: HashSet::new(),
                instances: HashMap::new(),
                conflicts: HashMap::new(),
                next_waiter: 1,
            })),
        }
    }

    pub fn touch(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_seen.insert(node_id.to_string(), now_ms());
    }

    pub fn last_seen(&self, node_id: &str) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .last_seen
            .get(node_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn is_online(&self, node_id: &str) -> bool {
        let seen = self.last_seen(node_id);
        seen > 0 && now_ms().saturating_sub(seen) <= ONLINE_AFTER_MS
    }

    /// Two agent processes recently polled with the same credential.
    pub fn has_conflict(&self, node_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .conflicts
            .get(node_id)
            .map(|at| now_ms().saturating_sub(*at) <= CONFLICT_SHOW_MS)
            .unwrap_or(false)
    }

    /// Record the agent instance id that is polling (sent by the agent on
    /// every poll). A different instance inside the online window means the
    /// credential is in use on two hosts.
    pub fn note_instance(&self, node_id: &str, instance: &str) {
        if instance.is_empty() {
            return;
        }
        let now = now_ms();
        let mut inner = self.inner.lock().unwrap();
        if let Some((prev, at)) = inner.instances.get(node_id) {
            if prev != instance && now.saturating_sub(*at) <= ONLINE_AFTER_MS {
                inner.conflicts.insert(node_id.to_string(), now);
            }
        }
        inner
            .instances
            .insert(node_id.to_string(), (instance.to_string(), now));
    }

    pub fn revoke(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.revoked.insert(node_id.to_string());
        if let Some(waiter) = inner.waiters.remove(node_id) {
            let _ = waiter.tx.send(Wake::Revoked);
        }
        inner.queues.remove(node_id);
        let prefix = format!("{node_id}:");
        let stale: Vec<String> = inner
            .inflight
            .keys()
            .filter(|id| id.starts_with(&prefix))
            .cloned()
            .collect();
        for id in stale {
            if let Some(tx) = inner.inflight.remove(&id) {
                let _ = tx.send(Err("nodeRevoked".into()));
            }
        }
    }

    pub fn is_revoked(&self, node_id: &str) -> bool {
        self.inner.lock().unwrap().revoked.contains(node_id)
    }

    /// Clear the in-memory revoked flag so a new join token can be used.
    /// Does not restore a previously issued credential.
    pub fn unrevoke(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.revoked.remove(node_id);
        inner.last_seen.remove(node_id);
        inner.conflicts.remove(node_id);
        inner.instances.remove(node_id);
        inner.queues.remove(node_id);
        if let Some(waiter) = inner.waiters.remove(node_id) {
            let _ = waiter.tx.send(Wake::Superseded);
        }
    }

    /// Long-poll for one job.
    pub fn wait_job(&self, node_id: &str, timeout: Duration) -> Option<JobWire> {
        match self.poll(node_id, timeout) {
            Polled::Job(job) => Some(job),
            _ => None,
        }
    }

    pub fn poll(&self, node_id: &str, timeout: Duration) -> Polled {
        let (rx, waiter_id) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.revoked.contains(node_id) {
                return Polled::Empty;
            }
            inner.last_seen.insert(node_id.to_string(), now_ms());
            if let Some(queue) = inner.queues.get_mut(node_id) {
                if let Some(job) = queue.pop_front() {
                    return Polled::Job(job);
                }
            }
            let (tx, rx) = mpsc::channel();
            let id = inner.next_waiter;
            inner.next_waiter = inner.next_waiter.wrapping_add(1);
            if let Some(old) = inner.waiters.insert(node_id.to_string(), Waiter { id, tx }) {
                let _ = old.tx.send(Wake::Superseded);
            }
            (rx, id)
        };
        let outcome = rx.recv_timeout(timeout);
        // whatever happened, make sure our own waiter is not left behind
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.waiters.get(node_id).map(|w| w.id) == Some(waiter_id) {
                inner.waiters.remove(node_id);
            }
        }
        match outcome {
            Ok(Wake::Job(job)) => Polled::Job(job),
            Ok(Wake::Superseded) => Polled::Superseded,
            Ok(Wake::Revoked) => Polled::Empty,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                // dispatch may have handed us a job between the timeout and
                // the cleanup above (it sends while holding the lock, so it
                // is in the channel by now), or queued one
                if let Ok(Wake::Job(job)) = rx.try_recv() {
                    return Polled::Job(job);
                }
                let mut inner = self.inner.lock().unwrap();
                match inner.queues.get_mut(node_id).and_then(|q| q.pop_front()) {
                    Some(job) => Polled::Job(job),
                    None => Polled::Empty,
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn waiter_count(&self) -> usize {
        self.inner.lock().unwrap().waiters.len()
    }

    pub fn dispatch(
        &self,
        node_id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if self.is_revoked(node_id) {
            return Err("nodeRevoked".into());
        }
        let job_id = format!("{}:{}", node_id, crate::store::new_id());
        let (tx, rx) = mpsc::channel();
        let wire = JobWire {
            id: job_id.clone(),
            method: method.to_string(),
            params,
        };
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.revoked.contains(node_id) {
                return Err("nodeRevoked".into());
            }
            inner.inflight.insert(job_id.clone(), tx);
            let mut pending = Some(wire);
            if let Some(waiter) = inner.waiters.remove(node_id) {
                if let Err(mpsc::SendError(Wake::Job(job))) =
                    waiter.tx.send(Wake::Job(pending.take().unwrap()))
                {
                    pending = Some(job);
                }
            }
            if let Some(wire) = pending {
                inner
                    .queues
                    .entry(node_id.to_string())
                    .or_default()
                    .push_back(wire);
            }
        }
        recv_result(rx, timeout, &self.inner, &job_id)
    }

    pub fn complete(&self, job_id: &str, result: Result<Value, String>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(tx) = inner.inflight.remove(job_id) {
            let _ = tx.send(result);
            true
        } else {
            false
        }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

fn recv_result(
    rx: Receiver<Result<Value, String>>,
    timeout: Duration,
    inner: &Mutex<Inner>,
    job_id: &str,
) -> Result<Value, String> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            let mut inner = inner.lock().unwrap();
            inner.inflight.remove(job_id);
            for queue in inner.queues.values_mut() {
                queue.retain(|job| job.id != job_id);
            }
            Err("nodeTimeout".into())
        }
        Err(RecvTimeoutError::Disconnected) => Err("nodeOffline".into()),
    }
}

pub struct PollTask {
    pub request: tiny_http::Request,
    pub app: Arc<Ctx>,
    pub req: Req,
}

pub struct AgentPool {
    tx: SyncSender<PollTask>,
}

impl AgentPool {
    /// A pool that accepts nothing (agent process, tests).
    pub fn disabled() -> Arc<Self> {
        let (tx, _rx) = mpsc::sync_channel::<PollTask>(0);
        Arc::new(Self { tx })
    }

    pub fn new(workers: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::sync_channel::<PollTask>(workers.saturating_mul(2).max(8));
        let rx = Arc::new(Mutex::new(rx));
        let n = workers.clamp(1, 128);
        for _ in 0..n {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name("agent-poll".into())
                .spawn(move || loop {
                    let task = {
                        let locked = rx.lock().unwrap();
                        locked.recv()
                    };
                    match task {
                        Ok(task) => {
                            let reply = handle_agent(&task.app, &task.req);
                            crate::server::respond(task.request, reply);
                        }
                        Err(_) => break,
                    }
                });
        }
        Arc::new(Self { tx })
    }

    pub fn submit(&self, task: PollTask) -> Result<(), Box<PollTask>> {
        self.tx.try_send(task).map_err(|err| match err {
            TrySendError::Full(task) | TrySendError::Disconnected(task) => Box::new(task),
        })
    }
}

pub fn hash_secret(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"nginxwebui-node-v1:");
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn secrets_match(provided: &str, stored_hash: &str) -> bool {
    if stored_hash.is_empty() || provided.is_empty() {
        return false;
    }
    let hashed = hash_secret(provided);
    hashed.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

pub fn new_secret(prefix: &str) -> String {
    format!("{prefix}{}", crate::auth::random_token(32))
}

pub fn public_node(row: &Value, hub: &Hub) -> Value {
    let id = str_of(row, "id");
    let revoked = str_of(row, "revoked") == "true" || bool_flag(row, "revoked");
    let pending =
        !str_of(row, "joinTokenHash").is_empty() && str_of(row, "credHash").is_empty() && !revoked;
    let online = !revoked && hub.is_online(&id);
    let status = if revoked {
        "revoked"
    } else if online {
        "online"
    } else if pending {
        "pending"
    } else {
        "offline"
    };
    json!({
        "id": id,
        "name": str_of(row, "name"),
        "status": status,
        "kind": "agent",
        "lastSeen": hub.last_seen(&id),
        "hostname": str_of(row, "hostname"),
        "os": str_of(row, "os"),
        "agentVersion": str_of(row, "agentVersion"),
        "joined": !str_of(row, "credHash").is_empty(),
        "conflict": !revoked && hub.has_conflict(&id),
    })
}

pub fn local_node() -> Value {
    json!({
        "id": "local",
        "name": "local",
        "status": "online",
        "kind": "local",
        "lastSeen": now_ms(),
        "hostname": "",
        "os": std::env::consts::OS,
        "agentVersion": crate::VERSION,
        "joined": true,
    })
}

fn bool_flag(row: &Value, key: &str) -> bool {
    match row.get(key) {
        Some(Value::Bool(v)) => *v,
        Some(Value::String(s)) => s == "true" || s == "1",
        _ => false,
    }
}

pub fn strip_secrets(row: &mut Value) {
    if let Value::Object(map) = row {
        map.remove("joinTokenHash");
        map.remove("credHash");
        map.remove("joinToken");
        map.remove("credential");
    }
}

pub fn issue_join_token(db: &mut Db, node_id: &str) -> Result<(String, u64), String> {
    issue_join_token_with_hub(db, None, node_id)
}

/// Issue a one-time join token. For a revoked (or previously joined) device
/// this clears the revoked flag and any leftover credential hash so a new
/// `join` can succeed without deleting the row.
pub fn issue_join_token_with_hub(
    db: &mut Db,
    hub: Option<&Hub>,
    node_id: &str,
) -> Result<(String, u64), String> {
    let Some(mut row) = db.get("node", node_id) else {
        return Err("not found".into());
    };
    let was_revoked = bool_flag(&row, "revoked");
    let token = new_secret("nwj_");
    let expires = now_ms().saturating_add(JOIN_TTL_MS);
    if let Value::Object(map) = &mut row {
        map.insert("joinTokenHash".into(), hash_secret(&token).into());
        map.insert("joinExpires".into(), expires.to_string().into());
        if was_revoked {
            map.insert("revoked".into(), false.into());
            map.insert("credHash".into(), "".into());
        }
    }
    db.upsert("node", row);
    if was_revoked {
        if let Some(hub) = hub {
            hub.unrevoke(node_id);
        }
    }
    Ok((token, expires))
}

pub fn create_node(db: &mut Db, name: &str) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("nameRequired".into());
    }
    if name.eq_ignore_ascii_case("local") {
        return Err("reservedName".into());
    }
    let row = json!({
        "name": name,
        "revoked": false,
        "created": now_ms().to_string(),
        "hostname": "",
        "os": "",
        "agentVersion": "",
        "joinTokenHash": "",
        "joinExpires": "0",
        "credHash": "",
    });
    let id = db.upsert("node", row);
    db.get("node", &id).ok_or_else(|| "saveFailed".into())
}

pub fn rename_node(db: &mut Db, id: &str, name: &str) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("nameRequired".into());
    }
    let Some(mut row) = db.get("node", id) else {
        return Err("not found".into());
    };
    if let Value::Object(map) = &mut row {
        map.insert("name".into(), name.into());
    }
    db.upsert("node", row);
    db.get("node", id).ok_or_else(|| "saveFailed".into())
}

pub fn revoke_node(db: &mut Db, hub: &Hub, id: &str) -> Result<(), String> {
    let Some(mut row) = db.get("node", id) else {
        return Err("not found".into());
    };
    if let Value::Object(map) = &mut row {
        map.insert("revoked".into(), true.into());
        map.insert("joinTokenHash".into(), "".into());
        map.insert("credHash".into(), "".into());
    }
    db.upsert("node", row);
    hub.revoke(id);
    Ok(())
}

pub fn join_with_token(db: &mut Db, token: &str, meta: &Value) -> Result<(Value, String), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("badToken".into());
    }
    let now = now_ms();
    let mut found: Option<Value> = None;
    for row in db.rows("node") {
        if bool_flag(row, "revoked") {
            continue;
        }
        let hash = str_of(row, "joinTokenHash");
        if hash.is_empty() {
            continue;
        }
        if secrets_match(token, &hash) {
            found = Some(row.clone());
            break;
        }
    }
    let Some(mut row) = found else {
        return Err("badToken".into());
    };
    let expires = str_of(&row, "joinExpires").parse::<u64>().unwrap_or(0);
    if expires > 0 && now > expires {
        return Err("tokenExpired".into());
    }
    let cred = new_secret("nwk_");
    if let Value::Object(map) = &mut row {
        map.insert("credHash".into(), hash_secret(&cred).into());
        map.insert("joinTokenHash".into(), "".into());
        map.insert("joinExpires".into(), "0".into());
        map.insert("hostname".into(), str_of(meta, "hostname").into());
        map.insert("os".into(), str_of(meta, "os").into());
        map.insert("agentVersion".into(), str_of(meta, "version").into());
        map.insert("joinedAt".into(), now.to_string().into());
    }
    let id = db.upsert("node", row);
    let saved = db.get("node", &id).unwrap();
    Ok((saved, cred))
}

pub fn node_by_credential(db: &Db, credential: &str) -> Option<Value> {
    if credential.is_empty() {
        return None;
    }
    for row in db.rows("node") {
        if bool_flag(row, "revoked") {
            continue;
        }
        let hash = str_of(row, "credHash");
        if secrets_match(credential, &hash) {
            return Some(row.clone());
        }
    }
    None
}

/// The agent credential, from `Authorization: Bearer …` only. Query strings
/// end up in access logs and bodies get echoed by debugging proxies, so
/// neither is accepted.
fn bearer(req: &Req) -> String {
    let header = req.authorization.trim();
    if let Some(rest) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    {
        return rest.trim().to_string();
    }
    String::new()
}

fn parse_body(req: &Req) -> Value {
    if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    }
}

pub fn handle_agent(ctx: &Ctx, req: &Req) -> Reply {
    let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 || parts[0] != "agent" || parts[1] != "v1" {
        return fail("not found");
    }
    match parts[2] {
        "join" => {
            let body = parse_body(req);
            let token = str_of(&body, "token");
            let mut store = ctx.store.lock().unwrap();
            let before = store.db.t.get("node").cloned();
            match join_with_token(&mut store.db, &token, &body) {
                Ok((row, cred)) => {
                    if let Err(err) = store.flush() {
                        // do not hand out a credential the panel will forget
                        // on restart; keep the join token usable for a retry
                        if let Some(rows) = before {
                            store.db.t.insert("node".into(), rows);
                        }
                        crate::applog::error(format!("agent join: cannot save node: {err}"));
                        return json_reply(500, &json!({ "success": false, "msg": "saveFailed" }));
                    }
                    ok(json!({
                        "nodeId": str_of(&row, "id"),
                        "name": str_of(&row, "name"),
                        "credential": cred,
                    }))
                }
                Err(err) => fail(&err),
            }
        }
        "hello" | "poll" | "result" => {
            let cred = bearer(req);
            let store = ctx.store.lock().unwrap();
            let Some(row) = node_by_credential(&store.db, &cred) else {
                return json_reply(
                    401,
                    &json!({ "success": false, "msg": "unauthorizedAgent" }),
                );
            };
            let id = str_of(&row, "id");
            if bool_flag(&row, "revoked") || ctx.hub.is_revoked(&id) {
                return fail("nodeRevoked");
            }
            drop(store);
            ctx.hub.touch(&id);
            match parts[2] {
                "hello" => {
                    let body = parse_body(req);
                    if !body.is_null() {
                        let mut store = ctx.store.lock().unwrap();
                        if let Some(mut row) = store.db.get("node", &id) {
                            let mut changed = false;
                            if let Value::Object(map) = &mut row {
                                for (src, dst) in [
                                    ("hostname", "hostname"),
                                    ("os", "os"),
                                    ("version", "agentVersion"),
                                ] {
                                    let value = str_of(&body, src);
                                    if !value.is_empty()
                                        && map.get(dst).and_then(|v| v.as_str())
                                            != Some(value.as_str())
                                    {
                                        map.insert(dst.into(), value.into());
                                        changed = true;
                                    }
                                }
                            }
                            // hello runs before every poll; only write when
                            // something actually changed
                            if changed {
                                let before = store.db.get("node", &id);
                                store.db.upsert("node", row);
                                if let Err(err) = store.flush() {
                                    if let Some(old) = before {
                                        store.db.upsert("node", old);
                                    }
                                    crate::applog::error(format!(
                                        "agent hello: cannot save node: {err}"
                                    ));
                                    return json_reply(
                                        500,
                                        &json!({ "success": false, "msg": "saveFailed" }),
                                    );
                                }
                            }
                        }
                    }
                    let os = str_of(&body, "os");
                    let arch = str_of(&body, "arch");
                    ok(json!({
                        "nodeId": id,
                        "ok": true,
                        "agent": advertised_agent(&ctx.exe_dir, &os, &arch),
                    }))
                }
                "poll" => {
                    let body = parse_body(req);
                    ctx.hub.note_instance(&id, &str_of(&body, "instance"));
                    match ctx.hub.poll(&id, POLL_WAIT) {
                        Polled::Job(job) => ok(json!({ "jobs": [{
                            "id": job.id,
                            "method": job.method,
                            "params": job.params,
                        }] })),
                        Polled::Empty => ok(json!({ "jobs": [] })),
                        Polled::Superseded => fail("pollSuperseded"),
                    }
                }
                "result" => {
                    let body = parse_body(req);
                    let job_id = str_of(&body, "jobId");
                    if job_id.is_empty() || !job_id.starts_with(&format!("{id}:")) {
                        return fail("badJob");
                    }
                    let result = if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(true)
                        && body
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty()
                    {
                        Ok(body.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(str_of(&body, "error"))
                    };
                    if ctx.hub.complete(&job_id, result) {
                        ok(json!({ "ok": true }))
                    } else {
                        fail("unknownJob")
                    }
                }
                _ => fail("not found"),
            }
        }
        _ => fail("not found"),
    }
}

pub fn proxy(ctx: &Ctx, req: &Req, node_id: &str, method: &str, params: Value) -> Reply {
    if !crate::rpc::is_allowed(method) {
        return fail("unsupportedMethod");
    }
    let (name, revoked) = {
        let store = ctx.store.lock().unwrap();
        match store.db.get("node", node_id) {
            Some(row) => (str_of(&row, "name"), bool_flag(&row, "revoked")),
            None => return fail("unknownNode"),
        }
    };
    if revoked || ctx.hub.is_revoked(node_id) {
        return fail("nodeRevoked");
    }
    // fail fast instead of making the user wait RPC_WAIT for a node that is
    // not connected
    if !ctx.hub.is_online(node_id) {
        return fail("nodeOfflineErr");
    }
    let who = {
        let token = req
            .cookies
            .get("nginxwebui_token")
            .cloned()
            .unwrap_or_default();
        ctx.sessions
            .lock()
            .unwrap()
            .get(&token)
            .map(|s| s.admin_name.clone())
            .unwrap_or_else(|| "admin".into())
    };
    let target = format!("{name}/{node_id}");
    let record = |ok: bool, detail: &str| {
        // reads (lists, progress polling) would flood the 200-entry log
        if !crate::rpc::is_mutating(method) {
            return;
        }
        let mut store = ctx.store.lock().unwrap();
        crate::api::record_user_operate(&mut store, &who, method, &target, ok, detail);
        let _ = store.flush();
    };
    match ctx.hub.dispatch(node_id, method, params, RPC_WAIT) {
        Ok(value) => {
            let value = crate::rpc::normalize_result(method, value);
            // check/reload report the nginx result in obj.success
            let good = value
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                && value
                    .get("obj")
                    .and_then(|o| o.get("success"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
            record(
                good,
                if good {
                    "ok"
                } else {
                    value.get("msg").and_then(|v| v.as_str()).unwrap_or("")
                },
            );
            json_reply(200, &value)
        }
        Err(err) => {
            record(false, &err);
            fail(&err)
        }
    }
}

const INSTALL_SCRIPT: &str = include_str!("../assets/agent-install.sh");
const INSTALL_SCRIPT_PS1: &str = include_str!("../assets/agent-install.ps1");

pub fn install_script(req: &Req) -> Reply {
    let base = if req.panel_base.is_empty() || !crate::api::valid_panel_url(&req.panel_base) {
        "https://panel.example".to_string()
    } else {
        req.panel_base.clone()
    };
    let script = INSTALL_SCRIPT.replace("__PANEL__", &base);
    Reply {
        status: 200,
        content_type: "text/x-shellscript; charset=utf-8".into(),
        body: script.into_bytes(),
        set_cookie: None,
        extra_headers: vec![("Cache-Control".into(), "no-store".into())],
    }
}

pub fn install_script_ps1(req: &Req) -> Reply {
    let base = if req.panel_base.is_empty() || !crate::api::valid_panel_url(&req.panel_base) {
        "https://panel.example".to_string()
    } else {
        req.panel_base.clone()
    };
    let script = INSTALL_SCRIPT_PS1.replace("__PANEL__", &base);
    Reply {
        status: 200,
        content_type: "text/plain; charset=utf-8".into(),
        body: script.into_bytes(),
        set_cookie: None,
        extra_headers: vec![("Cache-Control".into(), "no-store".into())],
    }
}

pub fn normalize_os(os: &str) -> String {
    match os.trim().to_ascii_lowercase().as_str() {
        "linux" => "linux".into(),
        "darwin" | "macos" => "macos".into(),
        "windows" | "win32" | "win" => "windows".into(),
        other => other.to_string(),
    }
}

pub fn normalize_arch(arch: &str) -> String {
    match arch.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        other => other.to_string(),
    }
}

/// Layout: `agents/{os}-{arch}/nginxwebui-agent`, then
/// `{exe_dir}/nginxwebui-agent` for the panel's own os/arch.
pub fn agent_binary_path(
    exe_dir: &std::path::Path,
    os: &str,
    arch: &str,
) -> Option<std::path::PathBuf> {
    let os = normalize_os(os);
    let arch = normalize_arch(arch);
    let current_os = normalize_os(std::env::consts::OS);
    let current_arch = normalize_arch(std::env::consts::ARCH);
    fn names_for(os_name: &str) -> [&'static str; 2] {
        if os_name == "windows" {
            ["nginxwebui-agent.exe", "nginxwebui-agent"]
        } else {
            ["nginxwebui-agent", "nginxwebui-agent.exe"]
        }
    }
    if !os.is_empty() && !arch.is_empty() {
        let dir = exe_dir.join("agents").join(format!("{os}-{arch}"));
        for name in names_for(&os) {
            let pinned = dir.join(name);
            if pinned.is_file() {
                return Some(pinned);
            }
        }
    }
    let matches_current =
        (os.is_empty() || os == current_os) && (arch.is_empty() || arch == current_arch);
    if matches_current {
        for name in names_for(&current_os) {
            let fallback = exe_dir.join(name);
            if fallback.is_file() {
                return Some(fallback);
            }
        }
        let dir = exe_dir
            .join("agents")
            .join(format!("{current_os}-{current_arch}"));
        for name in names_for(&current_os) {
            let pinned = dir.join(name);
            if pinned.is_file() {
                return Some(pinned);
            }
        }
    }
    None
}

pub fn agent_sha256(path: &std::path::Path) -> Option<String> {
    crate::upgrade::sha256_file(path)
}

pub fn advertised_agent(exe_dir: &std::path::Path, os: &str, arch: &str) -> Value {
    let os_n = if os.trim().is_empty() {
        normalize_os(std::env::consts::OS)
    } else {
        normalize_os(os)
    };
    let arch_n = if arch.trim().is_empty() {
        normalize_arch(std::env::consts::ARCH)
    } else {
        normalize_arch(arch)
    };
    let mut obj = json!({
        "version": crate::VERSION,
        "os": os_n,
        "arch": arch_n,
    });
    if let Some(path) = agent_binary_path(exe_dir, &os_n, &arch_n) {
        if let Some(sum) = agent_sha256(&path) {
            obj["sha256"] = json!(sum);
        }
        obj["size"] = json!(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
    }
    obj
}

pub fn download_agent(ctx: &Ctx, req: &Req) -> Reply {
    let os = req.query.get("os").cloned().unwrap_or_default();
    let arch = req.query.get("arch").cloned().unwrap_or_default();
    let os_n = normalize_os(&os);
    let arch_n = normalize_arch(&arch);
    let meta = req
        .query
        .get("meta")
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if meta {
        return ok(advertised_agent(&ctx.exe_dir, &os_n, &arch_n));
    }
    let Some(path) = agent_binary_path(&ctx.exe_dir, &os_n, &arch_n) else {
        let current_os = std::env::consts::OS;
        let current_arch = std::env::consts::ARCH;
        let label = if os.is_empty() && arch.is_empty() {
            format!("{current_os}/{current_arch}")
        } else {
            format!("{os_n}/{arch_n}")
        };
        return Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".into(),
            body: format!(
                "no agent binary for {label}. Place it at agents/{os_n}-{arch_n}/nginxwebui-agent (or next to the panel for {current_os}/{current_arch})."
            )
            .into_bytes(),
            set_cookie: None,
            extra_headers: Vec::new(),
        };
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            let sum = crate::upgrade::sha256_hex(&bytes);
            Reply {
                status: 200,
                content_type: "application/octet-stream".into(),
                body: bytes,
                set_cookie: None,
                extra_headers: vec![
                    (
                        "Content-Disposition".into(),
                        format!(
                            "attachment; filename={}",
                            if os_n == "windows" {
                                "nginxwebui-agent.exe"
                            } else {
                                "nginxwebui-agent"
                            }
                        ),
                    ),
                    ("X-Checksum-Sha256".into(), sum),
                ],
            }
        }
        Err(_) => Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".into(),
            body: b"nginxwebui-agent is not installed next to the panel binary".to_vec(),
            set_cookie: None,
            extra_headers: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Db, Store};

    fn agent_ctx() -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-reg-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let store = Store::open(&dir);
        Arc::new(Ctx::standalone(dir, store))
    }

    fn agent_req(path: &str, body: Value, auth: &str) -> Req {
        let mut req = Req::internal("POST", path, Default::default(), body);
        req.authorization = auth.to_string();
        req
    }

    fn reply_json(reply: &Reply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    fn break_sqlite(ctx: &Ctx) {
        let path = ctx.store.lock().unwrap().db_path.clone();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("DROP TABLE records;").unwrap();
    }

    fn joined(ctx: &Ctx) -> (String, String) {
        let mut store = ctx.store.lock().unwrap();
        let node = create_node(&mut store.db, "edge").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut store.db, &id).unwrap();
        store.flush().unwrap();
        (id, token)
    }

    #[test]
    fn join_does_not_issue_credentials_when_flush_fails() {
        let ctx = agent_ctx();
        let (id, token) = joined(&ctx);
        break_sqlite(&ctx);
        let reply = handle_agent(
            &ctx,
            &agent_req(
                "/agent/v1/join",
                json!({ "token": token, "hostname": "h" }),
                "",
            ),
        );
        let body = reply_json(&reply);
        assert_eq!(body["success"], false);
        assert_eq!(body["msg"], "saveFailed");
        assert!(!reply.body.windows(4).any(|w| w == b"nwk_"));
        // the in-memory row is rolled back: token still valid, no credential
        let store = ctx.store.lock().unwrap();
        let row = store.db.get("node", &id).unwrap();
        assert!(str_of(&row, "credHash").is_empty());
        assert!(!str_of(&row, "joinTokenHash").is_empty());
    }

    #[test]
    fn join_then_hello_and_query_credentials_are_rejected() {
        let ctx = agent_ctx();
        let (_, token) = joined(&ctx);
        let reply = handle_agent(
            &ctx,
            &agent_req(
                "/agent/v1/join",
                json!({ "token": token, "hostname": "h" }),
                "",
            ),
        );
        let body = reply_json(&reply);
        assert_eq!(body["success"], true, "{body}");
        let cred = str_of(&body["obj"], "credential");
        assert!(cred.starts_with("nwk_"));

        let ok_reply = handle_agent(
            &ctx,
            &agent_req(
                "/agent/v1/hello",
                json!({ "hostname": "h" }),
                &format!("Bearer {cred}"),
            ),
        );
        assert_eq!(ok_reply.status, 200);
        assert_eq!(reply_json(&ok_reply)["success"], true);

        // credential in the query string or body is not accepted any more
        let mut req = agent_req("/agent/v1/hello", json!({ "credential": cred }), "");
        req.query.insert("credential".into(), cred.clone());
        let denied = handle_agent(&ctx, &req);
        assert_eq!(denied.status, 401);
        assert_eq!(reply_json(&denied)["msg"], "unauthorizedAgent");
    }

    #[test]
    fn hello_reports_flush_failure_only_when_it_writes() {
        let ctx = agent_ctx();
        let (_, token) = joined(&ctx);
        let body = reply_json(&handle_agent(
            &ctx,
            &agent_req(
                "/agent/v1/join",
                json!({ "token": token, "hostname": "h", "os": "linux", "version": "1" }),
                "",
            ),
        ));
        let auth = format!("Bearer {}", str_of(&body["obj"], "credential"));
        break_sqlite(&ctx);
        // unchanged metadata: nothing to write, still fine
        let same = handle_agent(
            &ctx,
            &agent_req(
                "/agent/v1/hello",
                json!({ "hostname": "h", "os": "linux", "version": "1" }),
                &auth,
            ),
        );
        assert_eq!(reply_json(&same)["success"], true);
        let changed = handle_agent(
            &ctx,
            &agent_req("/agent/v1/hello", json!({ "hostname": "other" }), &auth),
        );
        assert_eq!(reply_json(&changed)["msg"], "saveFailed");
    }

    #[test]
    fn timed_out_polls_do_not_leave_waiters() {
        let hub = Hub::new();
        for _ in 0..5 {
            assert!(hub.wait_job("n1", Duration::from_millis(5)).is_none());
        }
        assert_eq!(hub.waiter_count(), 0);
    }

    #[test]
    fn newer_poll_supersedes_older_one() {
        let hub = Hub::new();
        let first = {
            let hub = hub.clone();
            std::thread::spawn(move || hub.poll("n1", Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(50));
        let second = {
            let hub = hub.clone();
            std::thread::spawn(move || hub.poll("n1", Duration::from_secs(5)))
        };
        assert!(matches!(first.join().unwrap(), Polled::Superseded));
        std::thread::sleep(Duration::from_millis(20));
        let dispatcher = {
            let hub = hub.clone();
            std::thread::spawn(move || {
                hub.dispatch("n1", "ping", json!({}), Duration::from_secs(2))
            })
        };
        match second.join().unwrap() {
            Polled::Job(job) => {
                assert_eq!(job.method, "ping");
                hub.complete(&job.id, Ok(json!({ "pong": true })));
            }
            other => panic!("expected job, got {other:?}"),
        }
        assert_eq!(dispatcher.join().unwrap().unwrap()["pong"], true);
        assert_eq!(hub.waiter_count(), 0);
    }

    #[test]
    fn two_instances_with_one_credential_are_flagged() {
        let hub = Hub::new();
        hub.note_instance("n1", "a");
        hub.note_instance("n1", "a");
        assert!(!hub.has_conflict("n1"));
        hub.note_instance("n1", "b");
        assert!(hub.has_conflict("n1"));
    }

    #[test]
    fn install_script_has_no_insecure_flag_and_warns_on_http() {
        let mut req = Req {
            panel_base: "https://panel.example".into(),
            ..Req::default()
        };
        let reply = install_script(&req);
        let text = String::from_utf8(reply.body).unwrap();
        assert!(text.contains("PANEL=\"https://panel.example\""));
        assert!(text.contains("--foreground"));
        assert!(text.contains("nginxwebui-agent.service"));
        assert!(text.contains("NGINXWEBUI_ALLOW_HTTP"));
        assert!(!text.contains("INSECURE="));
        // a hostile Host header cannot inject shell
        req.panel_base = "https://x\"; rm -rf /; \"".into();
        let text = String::from_utf8(install_script(&req).body).unwrap();
        assert!(text.contains("PANEL=\"https://panel.example\""));
    }

    #[test]
    fn join_token_is_one_time_and_can_expire() {
        let mut db = Db::new();
        let node = create_node(&mut db, "edge-1").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut db, &id).unwrap();
        let meta = json!({ "hostname": "a", "os": "linux", "version": "1.0.0" });
        let (saved, cred) = join_with_token(&mut db, &token, &meta).unwrap();
        assert_eq!(str_of(&saved, "hostname"), "a");
        assert!(str_of(&saved, "joinTokenHash").is_empty());
        assert!(!cred.is_empty());
        assert_eq!(
            join_with_token(&mut db, &token, &meta).unwrap_err(),
            "badToken"
        );

        let (token2, _) = issue_join_token(&mut db, &id).unwrap();
        if let Some(mut row) = db.get("node", &id) {
            if let Value::Object(map) = &mut row {
                map.insert("joinExpires".into(), "1".into());
            }
            db.upsert("node", row);
        }
        assert_eq!(
            join_with_token(&mut db, &token2, &meta).unwrap_err(),
            "tokenExpired"
        );
    }

    #[test]
    fn revoked_node_cannot_poll() {
        let mut db = Db::new();
        let hub = Hub::new();
        let node = create_node(&mut db, "edge-2").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut db, &id).unwrap();
        let (_, cred) = join_with_token(
            &mut db,
            &token,
            &json!({ "hostname": "b", "os": "linux", "version": "1" }),
        )
        .unwrap();
        assert!(node_by_credential(&db, &cred).is_some());
        revoke_node(&mut db, &hub, &id).unwrap();
        assert!(node_by_credential(&db, &cred).is_none());
        assert!(hub.is_revoked(&id));
        assert!(hub.wait_job(&id, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn revoked_node_can_rejoin_after_new_token() {
        let mut db = Db::new();
        let hub = Hub::new();
        let node = create_node(&mut db, "edge-rejoin").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut db, &id).unwrap();
        let (_, old_cred) = join_with_token(
            &mut db,
            &token,
            &json!({ "hostname": "b", "os": "linux", "version": "1" }),
        )
        .unwrap();
        revoke_node(&mut db, &hub, &id).unwrap();
        assert!(hub.is_revoked(&id));
        // a new one-time token clears revoked state so the same device can join again
        let (token2, _) = issue_join_token_with_hub(&mut db, Some(&hub), &id).unwrap();
        let row = db.get("node", &id).unwrap();
        assert!(!bool_flag(&row, "revoked"));
        assert!(str_of(&row, "credHash").is_empty());
        assert!(!hub.is_revoked(&id));
        assert!(node_by_credential(&db, &old_cred).is_none());
        let (saved, new_cred) = join_with_token(
            &mut db,
            &token2,
            &json!({ "hostname": "b2", "os": "linux", "version": "2" }),
        )
        .unwrap();
        assert_eq!(str_of(&saved, "hostname"), "b2");
        assert!(node_by_credential(&db, &new_cred).is_some());
        assert_eq!(
            join_with_token(&mut db, &token2, &json!({})).unwrap_err(),
            "badToken"
        );
    }

    #[test]
    fn poll_receives_job_and_returns_result() {
        let hub = Hub::new();
        let node_id = "n1";
        let hub2 = hub.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            hub2.dispatch(node_id, "ping", json!({}), Duration::from_secs(2))
        });
        let job = hub
            .wait_job(node_id, Duration::from_secs(2))
            .expect("job should arrive");
        assert_eq!(job.method, "ping");
        hub.complete(&job.id, Ok(json!({ "pong": true })));
        let value = handle.join().unwrap().unwrap();
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn public_node_hides_hashes() {
        let hub = Hub::new();
        let row = json!({
            "id": "x",
            "name": "n",
            "joinTokenHash": "abc",
            "credHash": "",
            "revoked": false
        });
        let view = public_node(&row, &hub);
        assert!(view.get("joinTokenHash").is_none());
        assert!(view.get("credHash").is_none());
        assert_eq!(view["status"], "pending");
    }

    #[test]
    fn download_serves_agents_dir_then_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-agents-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let other = dir.join("agents").join("linux-aarch64");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("nginxwebui-agent"), b"aarch-agent").unwrap();
        std::fs::write(dir.join("nginxwebui-agent"), b"native-agent").unwrap();
        let aarch = agent_binary_path(&dir, "linux", "aarch64").unwrap();
        assert_eq!(std::fs::read(&aarch).unwrap(), b"aarch-agent");
        let native = agent_binary_path(&dir, std::env::consts::OS, std::env::consts::ARCH).unwrap();
        assert_eq!(std::fs::read(&native).unwrap(), b"native-agent");
        assert!(agent_binary_path(&dir, "linux", "riscv64").is_none());
        let win = dir.join("agents").join("windows-x86_64");
        std::fs::create_dir_all(&win).unwrap();
        std::fs::write(win.join("nginxwebui-agent.exe"), b"win-agent").unwrap();
        let got = agent_binary_path(&dir, "windows", "x86_64").unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), b"win-agent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_ps1_is_served_and_mentions_windows_service() {
        let req = agent_req("/agent/install.ps1", Value::Null, "");
        let reply = install_script_ps1(&req);
        assert_eq!(reply.status, 200);
        let text = String::from_utf8_lossy(&reply.body);
        assert!(text.contains("nginxwebui-agent.exe"), "{text}");
        assert!(text.contains("New-Service"), "{text}");
        assert!(text.contains("os=windows"), "{text}");
        assert!(!text.contains("__PANEL__"), "{text}");
        assert!(text.contains("https://panel.example"), "{text}");
    }
}
