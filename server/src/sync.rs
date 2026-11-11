//! Complete copy of selected sites, linked cert records, DNS credentials,
//! and certificate files from one node to one or more others, followed by
//! `nginx -t` and (only if the test passes) a reload.
//!
//! A sync wave has an id so replicated applies do not bounce forever when
//! used from sync groups. This still does not move virtual IPs.

use crate::api::{fail, ok, Ctx, Reply, Req};
use crate::secrets::{should_store_secret, SECRET_CERT_KEYS, SECRET_SETTING_KEYS};
use crate::ssl::SslFile;
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
    pub files: Vec<SslFile>,
    pub warnings: Vec<String>,
    pub wave_id: String,
}

impl Bundle {
    pub fn with_wave(mut self, wave_id: String) -> Self {
        self.wave_id = wave_id;
        self
    }

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
            "files": crate::ssl::preview_files(&self.files),
            "dnsCreds": { "willCopy": will, "keys": keys },
            "warnings": self.warnings,
            "waveId": self.wave_id,
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

fn cert_names_of(bundle: &Bundle) -> Vec<String> {
    let mut names = Vec::new();
    let push = |names: &mut Vec<String>, s: String| {
        if !s.is_empty() && !names.iter().any(|x| x == &s) {
            names.push(s);
        }
    };
    for cert in &bundle.certs {
        push(&mut names, str_of(cert, "certName"));
        push(&mut names, str_of(cert, "domain"));
    }
    for site in &bundle.sites {
        push(&mut names, linked_cert_name(site));
        push(&mut names, str_of(site, "domain"));
    }
    names
}

fn attach_local_files(db: &Db, bundle: &mut Bundle) {
    let names = cert_names_of(bundle);
    let (files, warnings) = crate::ssl::collect_for_names(db, &names);
    bundle.files = files;
    bundle.warnings.extend(warnings);
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
    attach_local_files(db, &mut bundle);
    Ok(bundle)
}

/// Remote source: sites, cert metadata, and cert *files* via RPC. DNS
/// secrets stay write-only on the agent and cannot be copied.
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

fn save_records(db: &mut Db, bundle: &Bundle) -> (u32, u32, bool, Vec<String>) {
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
    (site_ids, cert_ids, dns, copied_sites)
}

fn test_and_reload(db: &Db) -> (bool, String, bool, String) {
    let (test_ok, test_out) = crate::nginx::test_conf(db);
    if !test_ok {
        return (false, test_out, false, String::new());
    }
    let (reload_ok, reload_out) = crate::nginx::reload(db);
    (true, test_out, reload_ok, reload_out)
}

#[allow(clippy::too_many_arguments)]
fn result_obj(
    sites: u32,
    certs: u32,
    dns: bool,
    site_ids: Vec<String>,
    written: Vec<String>,
    test_ok: bool,
    test_out: String,
    reload_ok: bool,
    reload_out: String,
    apply_err: Option<String>,
) -> Value {
    // Copy success is records + files written. test/reload are reported
    // separately so a missing nginx does not hide a successful file write.
    let copy_ok = apply_err.is_none();
    json!({
        "success": copy_ok,
        "sites": sites,
        "certs": certs,
        "dns": dns,
        "siteIds": site_ids,
        "filesWritten": written,
        "tested": true,
        "testOk": test_ok,
        "testOutput": test_out,
        "reloaded": test_ok,
        "reloadOk": reload_ok,
        "reloadOutput": reload_out,
        "applied": test_ok && reload_ok,
        "msg": apply_err.unwrap_or_default(),
    })
}

/// Apply a bundle to an in-process store (local panel or a test agent).
pub fn apply_local(db: &mut Db, bundle: &Bundle) -> Result<Value, String> {
    let _guard = crate::groups::enter_wave(&bundle.wave_id);
    let (sites, certs, dns, site_ids) = save_records(db, bundle);
    let written = if bundle.files.is_empty() {
        Vec::new()
    } else {
        crate::ssl::put_files(db, &bundle.files)?
    };
    let (test_ok, test_out, reload_ok, reload_out) = test_and_reload(db);
    Ok(result_obj(
        sites, certs, dns, site_ids, written, test_ok, test_out, reload_ok, reload_out, None,
    ))
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

fn reply_ok(reply: &Value) -> Result<(), String> {
    if reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
        Err(str_of(reply, "msg"))
    } else {
        Ok(())
    }
}

fn nginx_outcome(reply: &Value) -> (bool, String) {
    let obj = reply.get("obj").cloned().unwrap_or(reply.clone());
    let ok = obj
        .get("success")
        .and_then(|v| v.as_bool())
        .or_else(|| obj.get("ok").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    let out = str_of(&obj, "output");
    (ok, out)
}

pub fn apply_on_node(ctx: &Arc<Ctx>, node_id: &str, bundle: &Bundle) -> Result<Value, String> {
    let _guard = crate::groups::enter_wave(&bundle.wave_id);
    if node_id == "local" || node_id.is_empty() {
        let mut store = ctx.store.lock().unwrap();
        let result = apply_local(&mut store.db, bundle)?;
        store.flush().map_err(|err| format!("saveFailed: {err}"))?;
        return Ok(result);
    }
    let mut certs = 0u32;
    for cert in &bundle.certs {
        let body = strip_for_save(cert.clone(), &[]);
        let reply = dispatch(ctx, node_id, "cert.save", rpc_params(body))?;
        reply_ok(&reply)?;
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
        reply_ok(&reply)?;
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
        reply_ok(&reply)?;
        dns = SECRET_SETTING_KEYS.iter().any(|k| {
            bundle
                .dns
                .get(*k)
                .and_then(|v| v.as_str())
                .map(should_store_secret)
                .unwrap_or(false)
        });
    }
    let mut written = Vec::new();
    if !bundle.files.is_empty() {
        let files: Vec<Value> = bundle.files.iter().map(SslFile::to_wire).collect();
        let reply = dispatch(
            ctx,
            node_id,
            "ssl.put",
            rpc_params(json!({ "files": files })),
        )?;
        reply_ok(&reply)?;
        if let Some(list) = reply
            .get("obj")
            .and_then(|o| o.get("written"))
            .and_then(|v| v.as_array())
        {
            written = list
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        } else {
            written = bundle.files.iter().map(|f| f.name.clone()).collect();
        }
    }
    let test_reply = dispatch(ctx, node_id, "nginx.test", rpc_params(json!({})))?;
    let (test_ok, test_out) = nginx_outcome(&test_reply);
    let (reload_ok, reload_out) = if test_ok {
        let reload_reply = dispatch(ctx, node_id, "nginx.reload", rpc_params(json!({})))?;
        nginx_outcome(&reload_reply)
    } else {
        (false, String::new())
    };
    Ok(result_obj(
        sites, certs, dns, ids, written, test_ok, test_out, reload_ok, reload_out, None,
    ))
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

fn attach_remote_files(ctx: &Arc<Ctx>, source: &str, bundle: &mut Bundle) {
    let list_reply = match dispatch(ctx, source, "ssl.list", rpc_params(json!({}))) {
        Ok(v) => v,
        Err(err) => {
            bundle.warnings.push(format!("sslListFailed:{err}"));
            return;
        }
    };
    if list_reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
        bundle
            .warnings
            .push(format!("sslListFailed:{}", str_of(&list_reply, "msg")));
        return;
    }
    let available: Vec<String> = list_reply
        .get("obj")
        .and_then(|o| o.get("names"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let wanted = cert_names_of(bundle);
    let mut names = Vec::new();
    for name in &available {
        let Some(safe) = crate::ssl::sanitize_name(name) else {
            continue;
        };
        let stem = safe
            .trim_end_matches(".fullchain.cer")
            .trim_end_matches(".cer")
            .trim_end_matches(".crt")
            .trim_end_matches(".pem")
            .trim_end_matches(".key");
        if wanted
            .iter()
            .any(|c| c == stem || c == &safe || stem.starts_with(c) || c.starts_with(stem))
        {
            names.push(safe);
        }
    }
    if names.is_empty() {
        return;
    }
    match dispatch(
        ctx,
        source,
        "ssl.get",
        rpc_params(json!({ "names": names })),
    ) {
        Ok(reply) => {
            if reply.get("success").and_then(|v| v.as_bool()) == Some(false) {
                bundle
                    .warnings
                    .push(format!("sslGetFailed:{}", str_of(&reply, "msg")));
                return;
            }
            if let Some(list) = reply.get("obj").and_then(|o| o.get("files")) {
                match crate::ssl::files_from_value(list) {
                    Ok(files) => bundle.files = files,
                    Err(err) => bundle.warnings.push(err),
                }
            }
        }
        Err(err) => bundle.warnings.push(format!("sslGetFailed:{err}")),
    }
}

pub fn collect(ctx: &Arc<Ctx>, source: &str, site_ids: &[String]) -> Result<Bundle, String> {
    if source.is_empty() || source == "local" {
        let store = ctx.store.lock().unwrap();
        collect_local(&store.db, site_ids)
    } else {
        let (sites, certs) = list_sites_remote(ctx, source)?;
        let mut bundle = collect_remote(sites, certs, site_ids)?;
        attach_remote_files(ctx, source, &mut bundle);
        Ok(bundle)
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

pub fn apply_to_targets(
    ctx: &Arc<Ctx>,
    source: &str,
    bundle: &Bundle,
    targets: &[String],
) -> Vec<Value> {
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
        match apply_on_node(ctx, target, bundle) {
            Ok(obj) => {
                let mut row = obj;
                if let Value::Object(map) = &mut row {
                    map.insert("target".into(), target.clone().into());
                    map.insert("success".into(), true.into());
                    if !map.contains_key("msg") {
                        map.insert("msg".into(), "".into());
                    }
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
    results
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
    let mut bundle = match collect(ctx, &source, &site_ids) {
        Ok(b) => b,
        Err(err) => return fail(&err),
    };
    if bundle.wave_id.is_empty() {
        bundle.wave_id = str_of(&body, "waveId");
        if bundle.wave_id.is_empty() {
            bundle.wave_id = crate::store::new_id();
        }
    }
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
    let results = apply_to_targets(ctx, &source, &bundle, &targets);
    let preview = bundle.preview();
    crate::groups::after_manual_sync(ctx, &source, &site_ids, &bundle.wave_id, &results);
    ok(json!({
        "results": results,
        "preview": preview,
        "waveId": bundle.wave_id,
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
        let ctx = Ctx::standalone(dir.clone(), store);
        {
            let mut store = ctx.store.lock().unwrap();
            crate::seed::seed_with_root(
                &mut store.db,
                &dir,
                Some("nginx -s reload"),
                &dir.join("no-sys"),
            );
            let ssl = dir.join("ssl");
            let _ = std::fs::create_dir_all(&ssl);
            store.db.set_setting("sslDir", &ssl.display().to_string());
        }
        Arc::new(ctx)
    }

    #[test]
    fn copies_local_site_files_and_hides_secrets_in_preview() {
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
            let ssl = std::path::PathBuf::from(store.db.setting("sslDir"));
            std::fs::write(ssl.join("copy.example.com.cer"), "CER-MATERIAL").unwrap();
            std::fs::write(ssl.join("copy.example.com.key"), "KEY-MATERIAL").unwrap();
        }
        let bundle = {
            let store = ctx.store.lock().unwrap();
            collect_local(&store.db, std::slice::from_ref(&site_id)).unwrap()
        };
        assert_eq!(
            bundle.files.len(),
            2,
            "{:?}",
            bundle.files.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        let preview = bundle.preview();
        let text = preview.to_string();
        assert!(!text.contains("super-secret-token"), "{text}");
        assert!(!text.contains("should-not-leak"), "{text}");
        assert!(!text.contains("CER-MATERIAL"), "{text}");
        assert!(!text.contains("KEY-MATERIAL"), "{text}");
        assert_eq!(preview["dnsCreds"]["willCopy"], true);
        assert_eq!(preview["sites"][0]["domain"], "copy.example.com");
        assert!(preview["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "copy.example.com.key"));

        let target = ctx_at("dst");
        let result = {
            let mut store = target.store.lock().unwrap();
            let result = apply_local(&mut store.db, &bundle).unwrap();
            store.flush().unwrap();
            result
        };
        assert_eq!(result["success"], true, "{result}");
        assert!(result["tested"] == true);
        let written = result["filesWritten"].as_array().unwrap();
        assert_eq!(written.len(), 2, "{result}");
        // no real nginx in the test tree: test may fail, but the attempt is recorded
        // and the files must still be on disk
        let ssl = {
            let store = target.store.lock().unwrap();
            std::path::PathBuf::from(store.db.setting("sslDir"))
        };
        assert_eq!(
            std::fs::read_to_string(ssl.join("copy.example.com.cer")).unwrap(),
            "CER-MATERIAL"
        );
        assert_eq!(
            std::fs::read_to_string(ssl.join("copy.example.com.key")).unwrap(),
            "KEY-MATERIAL"
        );
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
        for cert in store.db.rows("cert") {
            assert!(str_of(cert, "cfToken").is_empty());
        }
        let result_text = result.to_string();
        assert!(!result_text.contains("KEY-MATERIAL"), "{result_text}");
        assert!(!result_text.contains("CER-MATERIAL"), "{result_text}");
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
