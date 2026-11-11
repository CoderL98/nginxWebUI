//! Manual copy of selected sites (and linked certs / DNS credentials)
//! from one node to one or more others. This is not automatic failover,
//! a floating IP, or an active-active cluster.

use crate::api::{fail, ok, Ctx, Reply, Req};
use crate::secrets::{should_store_secret, SECRET_CERT_KEYS, SECRET_SETTING_KEYS};
use crate::store::{str_of, Db};
use serde_json::{json, Map, Value};
use std::sync::Arc;

fn bool_flag(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true" || s == "1",
        _ => false,
    }
}

fn field_list(body: &Value, key: &str) -> Vec<String> {
    match body.get(key) {
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

#[derive(Clone, Default)]
pub struct Bundle {
    pub sites: Vec<Value>,
    pub certs: Vec<Value>,
    pub dns: Map<String, Value>,
    pub warnings: Vec<String>,
}

impl Bundle {
    pub fn preview(&self) -> Value {
        let sites: Vec<Value> = self
            .sites
            .iter()
            .map(|s| {
                json!({
                    "id": str_of(s, "id"),
                    "domain": str_of(s, "domain"),
                    "certName": str_of(s, "certName"),
                    "kind": str_of(s, "kind"),
                })
            })
            .collect();
        let certs: Vec<Value> = self
            .certs
            .iter()
            .map(|c| {
                json!({
                    "id": str_of(c, "id"),
                    "domain": str_of(c, "domain"),
                    "certName": str_of(c, "certName"),
                    "kind": str_of(c, "kind"),
                })
            })
            .collect();
        let mut keys = Map::new();
        let mut will = false;
        for key in SECRET_SETTING_KEYS.iter().chain(["acmeEmail"].iter()) {
            let set = self
                .dns
                .get(*key)
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            keys.insert((*key).to_string(), json!(set));
            if set && *key != "acmeEmail" {
                will = true;
            }
        }
        json!({
            "sites": sites,
            "certs": certs,
            "dnsCreds": { "willCopy": will, "keys": keys },
            "warnings": self.warnings,
        })
    }
}

fn linked_cert_name(site: &Value) -> String {
    let name = str_of(site, "certName");
    if !name.is_empty() {
        return name;
    }
    let name = str_of(site, "cert");
    if !name.is_empty() {
        return name;
    }
    str_of(site, "domain")
}

fn find_cert(db: &Db, name: &str) -> Option<Value> {
    if name.is_empty() {
        return None;
    }
    db.rows("cert")
        .iter()
        .find(|c| {
            str_of(c, "id") == name || str_of(c, "certName") == name || str_of(c, "domain") == name
        })
        .cloned()
}

fn find_id_by(db: &Db, table: &str, keys: &[(&str, &str)]) -> Option<String> {
    for row in db.rows(table) {
        if keys
            .iter()
            .any(|(k, v)| !v.is_empty() && str_of(row, k) == *v)
        {
            return Some(str_of(row, "id")).filter(|s| !s.is_empty());
        }
    }
    None
}

pub fn collect_local(db: &Db, site_ids: &[String]) -> Result<Bundle, String> {
    if site_ids.is_empty() {
        return Err("siteRequired".into());
    }
    let mut bundle = Bundle::default();
    for id in site_ids {
        match db.get("site", id) {
            Some(site) => {
                let cert_name = linked_cert_name(&site);
                if let Some(cert) = find_cert(db, &cert_name) {
                    let cid = str_of(&cert, "id");
                    if !bundle
                        .certs
                        .iter()
                        .any(|c| str_of(c, "id") == cid && !cid.is_empty())
                    {
                        bundle.certs.push(cert);
                    }
                }
                bundle.sites.push(site);
            }
            None => bundle.warnings.push(format!("missingSite:{id}")),
        }
    }
    if bundle.sites.is_empty() {
        return Err("siteRequired".into());
    }
    for key in SECRET_SETTING_KEYS.iter().chain(["acmeEmail"].iter()) {
        let value = db.setting(key);
        if !value.is_empty() {
            bundle.dns.insert((*key).to_string(), json!(value));
        }
    }
    Ok(bundle)
}

/// Remote source: sites and cert metadata only. DNS secrets are write-only
/// on the agent, so they cannot be copied from a remote source.
pub fn collect_remote(
    sites: Vec<Value>,
    certs: Vec<Value>,
    site_ids: &[String],
) -> Result<Bundle, String> {
    if site_ids.is_empty() {
        return Err("siteRequired".into());
    }
    let mut bundle = Bundle::default();
    for id in site_ids {
        let Some(site) = sites.iter().find(|s| str_of(s, "id") == *id).cloned() else {
            bundle.warnings.push(format!("missingSite:{id}"));
            continue;
        };
        let cert_name = linked_cert_name(&site);
        if let Some(cert) = certs.iter().find(|c| {
            str_of(c, "id") == cert_name
                || str_of(c, "certName") == cert_name
                || str_of(c, "domain") == cert_name
        }) {
            bundle.certs.push(cert.clone());
        }
        bundle.sites.push(site);
    }
    if bundle.sites.is_empty() {
        return Err("siteRequired".into());
    }
    bundle
        .warnings
        .push("dnsCredsUnavailableOnRemoteSource".into());
    Ok(bundle)
}

fn strip_for_save(mut row: Value, extra: &[&str]) -> Value {
    if let Value::Object(map) = &mut row {
        map.remove("id");
        for key in SECRET_CERT_KEYS.iter().chain(extra.iter()) {
            map.remove(*key);
        }
        map.remove("pemContent");
        map.remove("keyContent");
    }
    row
}

fn upsert_row(db: &mut Db, table: &str, mut row: Value, match_keys: &[(&str, String)]) -> String {
    let refs: Vec<(&str, &str)> = match_keys.iter().map(|(k, v)| (*k, v.as_str())).collect();
    if let Some(existing) = find_id_by(db, table, &refs) {
        if let Value::Object(map) = &mut row {
            map.insert("id".into(), existing.clone().into());
        }
    }
    db.upsert(table, row)
}

/// Apply a bundle to an in-process store (local panel or a test agent).
pub fn apply_local(db: &mut Db, bundle: &Bundle, write_conf: bool) -> Result<Value, String> {
    let mut cert_ids = 0u32;
    let mut site_ids = 0u32;
    for cert in &bundle.certs {
        let domain = str_of(cert, "domain");
        let cert_name = str_of(cert, "certName");
        let row = strip_for_save(cert.clone(), &[]);
        upsert_row(
            db,
            "cert",
            row,
            &[("certName", cert_name), ("domain", domain)],
        );
        cert_ids += 1;
    }
    let mut copied_sites = Vec::new();
    for site in &bundle.sites {
        let domain = str_of(site, "domain");
        let mut row = strip_for_save(site.clone(), &[]);
        if let Value::Object(map) = &mut row {
            map.insert("apply".into(), false.into());
        }
        let id = upsert_row(db, "site", row, &[("domain", domain.clone())]);
        copied_sites.push(id);
        site_ids += 1;
        let _ = write_conf;
    }
    let mut dns = false;
    for (key, value) in &bundle.dns {
        if let Some(text) = value.as_str() {
            if key == "acmeEmail" || should_store_secret(text) {
                db.set_setting(key, text);
                if key != "acmeEmail" {
                    dns = true;
                }
            }
        }
    }
    Ok(json!({
        "success": true,
        "sites": site_ids,
        "certs": cert_ids,
        "dns": dns,
        "siteIds": copied_sites,
        "applied": false,
    }))
}

fn rpc_params(body: Value) -> Value {
    json!({ "query": {}, "body": body })
}

fn dispatch(ctx: &Arc<Ctx>, node_id: &str, method: &str, params: Value) -> Result<Value, String> {
    if node_id == "local" || node_id.is_empty() {
        return crate::rpc::execute(ctx, method, &params);
    }
    ctx.hub
        .dispatch(node_id, method, params, crate::registry::RPC_WAIT)
        .map(|v| crate::rpc::normalize_result(method, v))
}

pub fn apply_on_node(ctx: &Arc<Ctx>, node_id: &str, bundle: &Bundle) -> Result<Value, String> {
    if node_id == "local" || node_id.is_empty() {
        let mut store = ctx.store.lock().unwrap();
        let result = apply_local(&mut store.db, bundle, false)?;
        store.flush().map_err(|err| format!("saveFailed: {err}"))?;
        return Ok(result);
    }
    let mut certs = 0u32;
    for cert in &bundle.certs {
        let body = strip_for_save(cert.clone(), &[]);
        let reply = dispatch(ctx, node_id, "cert.save", rpc_params(body))?;
        if reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
            return Err(str_of(&reply, "msg"));
        }
        certs += 1;
    }
    let mut sites = 0u32;
    let mut ids = Vec::new();
    for site in &bundle.sites {
        let mut body = strip_for_save(site.clone(), &[]);
        if let Value::Object(map) = &mut body {
            map.insert("apply".into(), false.into());
        }
        let reply = dispatch(ctx, node_id, "site.save", rpc_params(body))?;
        if reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
            return Err(str_of(&reply, "msg"));
        }
        if let Some(id) = reply
            .get("obj")
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
        {
            ids.push(id.to_string());
        }
        sites += 1;
    }
    let mut dns = false;
    if !bundle.dns.is_empty() {
        let reply = dispatch(
            ctx,
            node_id,
            "cert.dnsSave",
            rpc_params(Value::Object(bundle.dns.clone())),
        )?;
        if reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
            return Err(str_of(&reply, "msg"));
        }
        dns = SECRET_SETTING_KEYS.iter().any(|k| {
            bundle
                .dns
                .get(*k)
                .and_then(|v| v.as_str())
                .map(should_store_secret)
                .unwrap_or(false)
        });
    }
    Ok(json!({
        "success": true,
        "sites": sites,
        "certs": certs,
        "dns": dns,
        "siteIds": ids,
        "applied": false,
    }))
}

fn list_sites_remote(ctx: &Arc<Ctx>, node_id: &str) -> Result<(Vec<Value>, Vec<Value>), String> {
    let sites_reply = dispatch(ctx, node_id, "site.list", rpc_params(json!({})))?;
    if sites_reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return Err(str_of(&sites_reply, "msg"));
    }
    let sites = sites_reply
        .get("obj")
        .and_then(|o| {
            o.as_array()
                .cloned()
                .or_else(|| o.get("list").and_then(|l| l.as_array().cloned()))
                .or_else(|| o.get("sites").and_then(|l| l.as_array().cloned()))
        })
        .unwrap_or_default();
    let certs_reply = dispatch(ctx, node_id, "cert.list", rpc_params(json!({})))?;
    let certs = certs_reply
        .get("obj")
        .and_then(|o| o.get("certs"))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default();
    Ok((sites, certs))
}

fn collect(ctx: &Arc<Ctx>, source: &str, site_ids: &[String]) -> Result<Bundle, String> {
    if source.is_empty() || source == "local" {
        let store = ctx.store.lock().unwrap();
        collect_local(&store.db, site_ids)
    } else {
        let (sites, certs) = list_sites_remote(ctx, source)?;
        collect_remote(sites, certs, site_ids)
    }
}

fn site_summary(row: &Value) -> Value {
    json!({
        "id": str_of(row, "id"),
        "domain": str_of(row, "domain"),
        "certName": str_of(row, "certName"),
        "kind": str_of(row, "kind"),
    })
}

fn list_available(ctx: &Arc<Ctx>, source: &str) -> Reply {
    if source.is_empty() || source == "local" {
        let store = ctx.store.lock().unwrap();
        let sites: Vec<Value> = store.db.rows("site").iter().map(site_summary).collect();
        return ok(json!({ "source": "local", "sites": sites }));
    }
    match list_sites_remote(ctx, source) {
        Ok((sites, _)) => ok(json!({
            "source": source,
            "sites": sites.iter().map(site_summary).collect::<Vec<_>>(),
        })),
        Err(err) => fail(&err),
    }
}

pub fn handle(ctx: &Arc<Ctx>, action: &str, req: &Req) -> Reply {
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    };
    let source = str_of(&body, "source");
    if action == "syncList" {
        return list_available(ctx, &source);
    }
    let site_ids = field_list(&body, "siteIds");
    let bundle = match collect(ctx, &source, &site_ids) {
        Ok(b) => b,
        Err(err) => return fail(&err),
    };
    if action == "syncPreview" || bool_flag(&body, "preview") {
        let mut obj = bundle.preview();
        if let Value::Object(map) = &mut obj {
            map.insert(
                "source".into(),
                if source.is_empty() {
                    json!("local")
                } else {
                    json!(source)
                },
            );
        }
        return ok(obj);
    }
    let targets = field_list(&body, "targets");
    if targets.is_empty() {
        return fail("targetRequired");
    }
    if targets
        .iter()
        .any(|t| t == &source || (source.is_empty() && t == "local"))
        && targets.len() == 1
        && (source.is_empty() || source == "local")
        && targets[0] == "local"
    {
        return fail("targetRequired");
    }
    let mut results = Vec::new();
    for target in targets {
        if target == source || (target == "local" && (source.is_empty() || source == "local")) {
            results.push(json!({
                "target": target,
                "success": false,
                "msg": "cannotSyncToSelf",
            }));
            continue;
        }
        match apply_on_node(ctx, &target, &bundle) {
            Ok(obj) => {
                let mut row = obj;
                if let Value::Object(map) = &mut row {
                    map.insert("target".into(), target.clone().into());
                    map.insert("success".into(), true.into());
                    map.insert("msg".into(), "".into());
                }
                results.push(row);
            }
            Err(err) => results.push(json!({
                "target": target,
                "success": false,
                "msg": err,
            })),
        }
    }
    // never echo dns secret values
    let preview = bundle.preview();
    ok(json!({
        "results": results,
        "preview": preview,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn ctx_at(label: &str) -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-sync-{label}-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let store = Store::open(&dir.join("data"));
        Arc::new(Ctx::standalone(dir, store))
    }

    #[test]
    fn copies_local_site_and_hides_secrets_in_preview() {
        let ctx = ctx_at("src");
        let site_id;
        {
            let mut store = ctx.store.lock().unwrap();
            site_id = store.db.upsert(
                "site",
                json!({
                    "domain": "copy.example.com",
                    "kind": "static",
                    "root": "/var/www/copy",
                    "certName": "copy.example.com"
                }),
            );
            store.db.upsert(
                "cert",
                json!({
                    "domain": "copy.example.com",
                    "certName": "copy.example.com",
                    "kind": "dns_cf",
                    "cfToken": "should-not-leak"
                }),
            );
            store.db.set_setting("CF_Token", "super-secret-token");
            store.db.set_setting("acmeEmail", "ops@example.com");
        }
        let bundle = {
            let store = ctx.store.lock().unwrap();
            collect_local(&store.db, std::slice::from_ref(&site_id)).unwrap()
        };
        let preview = bundle.preview();
        let text = preview.to_string();
        assert!(!text.contains("super-secret-token"), "{text}");
        assert!(!text.contains("should-not-leak"), "{text}");
        assert_eq!(preview["dnsCreds"]["willCopy"], true);
        assert_eq!(preview["sites"][0]["domain"], "copy.example.com");

        let target = ctx_at("dst");
        {
            let mut store = target.store.lock().unwrap();
            apply_local(&mut store.db, &bundle, false).unwrap();
            store.flush().unwrap();
        }
        let store = target.store.lock().unwrap();
        let sites = store.db.rows("site");
        assert!(
            sites
                .iter()
                .any(|s| str_of(s, "domain") == "copy.example.com"),
            "{sites:?}"
        );
        assert_eq!(store.db.setting("CF_Token"), "super-secret-token");
        assert_eq!(store.db.setting("acmeEmail"), "ops@example.com");
        // cert secret fields were not copied (dns goes through settings)
        for cert in store.db.rows("cert") {
            assert!(str_of(cert, "cfToken").is_empty());
        }
    }

    #[test]
    fn remote_source_does_not_claim_dns_secrets() {
        let sites = vec![json!({"id": "s1", "domain": "a.example", "certName": "a.example"})];
        let certs = vec![json!({"id": "c1", "domain": "a.example", "certName": "a.example"})];
        let bundle = collect_remote(sites, certs, &["s1".into()]).unwrap();
        let preview = bundle.preview();
        assert_eq!(preview["dnsCreds"]["willCopy"], false);
        assert!(preview["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str() == Some("dnsCredsUnavailableOnRemoteSource")));
    }
}
