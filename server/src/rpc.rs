//! Whitelisted RPC methods the agent will run. This is not a remote shell.
//!
//! Each method maps to exactly one panel API action (`kind/action`). On the
//! agent the job runs through the *same* handler the panel uses locally
//! ([`crate::api::dispatch_local`]) against the node's own SQLite and nginx,
//! so a remote node answers with the same `{ success, obj, msg }` JSON as the
//! local panel: `site.apply` looks the row up by id, `nginx.test` reports
//! `obj.success`, `site.save` with `apply=false` really saves, and so on.
//!
//! Anything not in [`ROUTES`] is rejected on both sides. Panel-only areas
//! (settings, admins, backups, export, www uploads, upstream/stream, htpasswd,
//! raw `conf/write|replace`, `runCmd`) are deliberately absent. nginx/acme.sh
//! install is allowlisted (same confirm / autoInstall-off defaults as the panel).

use crate::api::{Ctx, Req};
use crate::store::str_of;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// One allowlisted RPC.
pub struct Route {
    pub method: &'static str,
    pub kind: &'static str,
    pub action: &'static str,
    /// HTTP verb the handler expects.
    pub verb: &'static str,
    /// Changes state on the node (recorded in the panel's operation log).
    pub mutating: bool,
}

const fn r(
    method: &'static str,
    kind: &'static str,
    action: &'static str,
    verb: &'static str,
    mutating: bool,
) -> Route {
    Route {
        method,
        kind,
        action,
        verb,
        mutating,
    }
}

pub const ROUTES: &[Route] = &[
    r("sysinfo", "main", "monitor", "GET", false),
    r("conf.show", "conf", "show", "GET", false),
    r("nginx.status", "conf", "status", "GET", false),
    r("nginx.test", "conf", "check", "POST", false),
    r("nginx.reload", "conf", "reload", "POST", true),
    r("nginx.signal", "conf", "signal", "POST", true),
    r("site.list", "site", "list", "GET", false),
    r("site.detail", "site", "detail", "GET", false),
    r("site.preview", "site", "preview", "POST", false),
    r("site.findCert", "site", "findCert", "GET", false),
    r("site.save", "site", "save", "POST", true),
    r("site.apply", "site", "apply", "POST", true),
    r("site.del", "site", "del", "POST", true),
    r("log.names", "log", "names", "GET", false),
    r("log.tail", "log", "tail", "GET", false),
    r("cert.list", "cert", "list", "GET", false),
    r("cert.scan", "cert", "scan", "POST", true),
    r("cert.status", "cert", "status", "GET", false),
    r("cert.progress", "cert", "progress", "GET", false),
    r("cert.save", "cert", "save", "POST", true),
    r("cert.del", "cert", "del", "POST", true),
    r("cert.apply", "cert", "apply", "POST", true),
    r("cert.renew", "cert", "renew", "POST", true),
    r("cert.cron", "cert", "cron", "POST", true),
    r("cert.dns", "cert", "dns", "GET", false),
    r("cert.dnsSave", "cert", "dnsSave", "POST", true),
    r("install.status", "install", "status", "GET", false),
    r("install.nginx", "install", "nginx", "POST", true),
    r("install.acme", "install", "acme", "POST", true),
    r("install.auto", "install", "auto", "POST", true),
];

/// Older method names (agent ≤ 1.0 / panel ≤ 1.0) accepted by the agent.
const ALIASES: &[(&str, &str)] = &[("acme.status", "cert.status"), ("acme.apply", "cert.apply")];

pub fn route(method: &str) -> Option<&'static Route> {
    let method = ALIASES
        .iter()
        .find(|(old, _)| *old == method)
        .map(|(_, new)| *new)
        .unwrap_or(method);
    ROUTES.iter().find(|route| route.method == method)
}

pub fn is_allowed(method: &str) -> bool {
    method == "ping" || route(method).is_some()
}

pub fn is_mutating(method: &str) -> bool {
    route(method).map(|r| r.mutating).unwrap_or(false)
}

/// Execute a whitelisted RPC against this machine (normally on the agent).
/// Returns the API envelope `{ success, obj, msg }` the local handler built.
pub fn execute(ctx: &Arc<Ctx>, method: &str, params: &Value) -> Result<Value, String> {
    if method == "ping" {
        return Ok(json!({ "pong": true, "version": crate::VERSION }));
    }
    let Some(route) = route(method) else {
        return Err("unsupportedMethod".into());
    };
    let (query, body) = split_params(params);
    let req = Req::internal(
        route.verb,
        &format!("/adminPage/{}/{}", route.kind, route.action),
        query,
        body,
    );
    let reply = crate::api::dispatch_local(ctx, route.kind, route.action, &req);
    serde_json::from_slice::<Value>(&reply.body).map_err(|_| "badReply".to_string())
}

/// Wire format: `{ "query": {k: v}, "body": <json> }`. A legacy params object
/// without those keys is treated as the body.
fn split_params(params: &Value) -> (HashMap<String, String>, Value) {
    let is_wire = params
        .as_object()
        .map(|m| m.keys().all(|k| k == "query" || k == "body") && !m.is_empty())
        .unwrap_or(false);
    if !is_wire {
        return (HashMap::new(), params.clone());
    }
    let mut query = HashMap::new();
    if let Some(Value::Object(map)) = params.get("query") {
        for (k, v) in map {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            query.insert(k.clone(), text);
        }
    }
    (query, params.get("body").cloned().unwrap_or(Value::Null))
}

/// Map an authenticated panel API call onto a remote RPC, if it is allowed.
pub fn map_admin(kind: &str, action: &str, req: &Req) -> Option<(String, Value)> {
    let route = ROUTES
        .iter()
        .find(|route| route.kind == kind && route.action == action)?;
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    };
    let query: serde_json::Map<String, Value> = req
        .query
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    Some((
        route.method.to_string(),
        json!({ "query": query, "body": body }),
    ))
}

/// Requests that always run on the panel, whatever device is selected.
pub fn stays_on_panel(kind: &str, action: &str) -> bool {
    matches!(
        kind,
        "login" | "admin" | "node" | "setting" | "export" | "bak"
    ) || (kind == "log" && action == "operate")
}

/// Turn whatever the agent returned into the panel's envelope. Current agents
/// already return `{ success, obj, msg }`; older ones returned bare results
/// such as `{ ok, output }` for `nginx.test`, which used to be wrapped as
/// `{ success: true, obj: { ok } }` and shown as a failure.
pub fn normalize_result(method: &str, value: Value) -> Value {
    if value.get("success").and_then(|v| v.as_bool()).is_some() {
        return value;
    }
    let legacy_ok = value.get("ok").and_then(|v| v.as_bool());
    match (route(method).map(|r| r.method), legacy_ok) {
        (Some("nginx.test" | "nginx.reload" | "nginx.signal"), Some(good)) => {
            let mut obj = json!({
                "success": good,
                "output": str_of(&value, "output"),
            });
            if let Some(running) = value.get("running") {
                obj["running"] = running.clone();
            }
            json!({ "success": true, "obj": obj, "msg": "" })
        }
        (Some("site.apply" | "cert.apply"), Some(false)) => json!({
            "success": false,
            "obj": value,
            "msg": "failed",
        }),
        _ => json!({ "success": true, "obj": value, "msg": "" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn ctx() -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-rpc-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let store = Store::open(&dir.join("data"));
        let mut ctx = Ctx::standalone(dir.clone(), store);
        ctx.agent_mode = true;
        {
            let mut store = ctx.store.lock().unwrap();
            crate::seed::seed_with_root(&mut store.db, &dir, None, &dir.join("no-system"));
        }
        Arc::new(ctx)
    }

    fn admin_req(kind: &str, action: &str, query: &[(&str, &str)], body: Value) -> Req {
        let mut req = Req::internal(
            "POST",
            &format!("/adminPage/{kind}/{action}"),
            query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body,
        );
        req.node = "n1".into();
        req
    }

    #[test]
    fn rejects_unknown_methods() {
        let ctx = ctx();
        for method in [
            "shell.exec",
            "runCmd",
            "conf.write",
            "setting.set",
            "www.upload",
        ] {
            assert_eq!(
                execute(&ctx, method, &json!({"cmd": "id"})).unwrap_err(),
                "unsupportedMethod",
                "{method}"
            );
        }
        assert!(!is_allowed("rm"));
        assert!(is_allowed("acme.apply"), "legacy alias");
    }

    #[test]
    fn ping_does_not_need_nginx() {
        let ctx = ctx();
        let value = execute(&ctx, "ping", &json!({})).unwrap();
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn install_actions_are_mapped() {
        let req = admin_req("install", "nginx", &[], json!({}));
        let (method, _) = map_admin("install", "nginx", &req).unwrap();
        assert_eq!(method, "install.nginx");
        let (method, _) = map_admin("install", "acme", &req).unwrap();
        assert_eq!(method, "install.acme");
        let (method, _) = map_admin("install", "status", &req).unwrap();
        assert_eq!(method, "install.status");
        let (method, params) = map_admin(
            "install",
            "auto",
            &admin_req("install", "auto", &[], json!({ "enable": false })),
        )
        .unwrap();
        assert_eq!(method, "install.auto");
        assert_eq!(params["body"]["enable"], false);
        assert!(is_mutating("install.nginx"));
        assert!(!is_mutating("install.status"));
    }

    #[test]
    fn panel_only_areas_are_not_mapped() {
        let req = admin_req("x", "y", &[], json!({}));
        for (kind, action) in [
            ("conf", "write"),
            ("conf", "replace"),
            ("conf", "runCmd"),
            ("conf", "saveCmd"),
            ("www", "upload"),
            ("upstream", "save"),
            ("password", "save"),
        ] {
            assert!(map_admin(kind, action, &req).is_none(), "{kind}/{action}");
        }
    }

    #[test]
    fn site_apply_by_id_keeps_the_stored_row() {
        let ctx = ctx();
        let id = {
            let mut store = ctx.store.lock().unwrap();
            store.db.upsert(
                "site",
                json!({
                    "domain": "keep.example.com",
                    "kind": "static",
                    "mode": "static",
                    "root": "/var/www/keep",
                    "certName": "keep.example.com",
                    "ssl": false
                }),
            )
        };
        // the list page sends only the id
        let req = admin_req("site", "apply", &[], json!({ "id": id }));
        let (method, params) = map_admin("site", "apply", &req).unwrap();
        assert_eq!(method, "site.apply");
        assert_eq!(params["body"]["id"], id.as_str());
        let reply = execute(&ctx, &method, &params).unwrap();
        // nginx is not installed in tests, so the apply itself may fail, but
        // the reply must be a normal envelope and the row must be intact
        assert!(reply.get("success").is_some(), "{reply}");
        let store = ctx.store.lock().unwrap();
        let row = store.db.get("site", &id).unwrap();
        assert_eq!(str_of(&row, "domain"), "keep.example.com");
        assert_eq!(str_of(&row, "root"), "/var/www/keep");
        assert_eq!(str_of(&row, "certName"), "keep.example.com");
    }

    #[test]
    fn site_save_without_apply_writes_the_agent_db() {
        let ctx = ctx();
        let body = json!({
            "domain": "draft.example.com",
            "kind": "static",
            "root": "/var/www/draft",
            "apply": false
        });
        let req = admin_req("site", "save", &[], body);
        let (method, params) = map_admin("site", "save", &req).unwrap();
        assert_eq!(method, "site.save");
        let reply = execute(&ctx, &method, &params).unwrap();
        assert_eq!(reply["success"], true, "{reply}");
        let id = str_of(&reply["obj"], "id");
        // a partial save afterwards keeps the other fields
        let patch = admin_req("site", "save", &[], json!({ "id": id, "descr": "x" }));
        let (method, params) = map_admin("site", "save", &patch).unwrap();
        let reply = execute(&ctx, &method, &params).unwrap();
        assert_eq!(reply["success"], true, "{reply}");
        let store = ctx.store.lock().unwrap();
        let row = store.db.get("site", &id).unwrap();
        assert_eq!(str_of(&row, "root"), "/var/www/draft");
        assert_eq!(str_of(&row, "descr"), "x");
    }

    #[test]
    fn cert_apply_by_id_resolves_the_full_row() {
        let ctx = ctx();
        let id = {
            let mut store = ctx.store.lock().unwrap();
            store.db.upsert(
                "cert",
                json!({ "domain": "c.example.com", "kind": "dns_cf" }),
            )
        };
        let req = admin_req("cert", "apply", &[], json!({ "id": id, "force": false }));
        let (method, params) = map_admin("cert", "apply", &req).unwrap();
        assert_eq!(method, "cert.apply");
        let reply = execute(&ctx, &method, &params).unwrap();
        // the handler found the row (no "not found") and started a job, or
        // reports another job / missing acme.sh — never "badDomain"
        assert_ne!(reply["msg"], "not found", "{reply}");
        assert_ne!(reply["msg"], "badDomain", "{reply}");
        let unknown = admin_req("cert", "apply", &[], json!({ "id": "nope" }));
        let (method, params) = map_admin("cert", "apply", &unknown).unwrap();
        assert_eq!(execute(&ctx, &method, &params).unwrap()["msg"], "not found");
        // wait for the background job so it does not leak into other tests
        for _ in 0..200 {
            if !crate::acme::job_snapshot().running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn nginx_test_has_local_shape() {
        let ctx = ctx();
        let req = admin_req("conf", "check", &[], json!({}));
        let (method, params) = map_admin("conf", "check", &req).unwrap();
        let reply = execute(&ctx, &method, &params).unwrap();
        assert_eq!(reply["success"], true);
        assert!(reply["obj"].get("success").is_some(), "{reply}");
        assert!(reply["obj"].get("output").is_some(), "{reply}");
        assert!(reply["obj"].get("ok").is_none());
    }

    #[test]
    fn legacy_ok_results_are_normalized() {
        let value = normalize_result("nginx.test", json!({ "ok": true, "output": "fine" }));
        assert_eq!(value["success"], true);
        assert_eq!(value["obj"]["success"], true);
        assert_eq!(value["obj"]["output"], "fine");
        let value = normalize_result("nginx.reload", json!({ "ok": false, "output": "bad" }));
        assert_eq!(value["success"], true);
        assert_eq!(value["obj"]["success"], false);
        let passthrough = json!({ "success": false, "msg": "needsSitesIncludeConfirm", "obj": {} });
        assert_eq!(
            normalize_result("site.apply", passthrough.clone()),
            passthrough
        );
    }

    #[test]
    fn log_names_and_dns_are_mapped_and_dns_is_write_only() {
        let ctx = ctx();
        let req = admin_req("log", "names", &[], Value::Null);
        let (method, params) = map_admin("log", "names", &req).unwrap();
        assert_eq!(method, "log.names");
        assert_eq!(execute(&ctx, &method, &params).unwrap()["success"], true);

        let save = admin_req(
            "cert",
            "dnsSave",
            &[],
            json!({ "CF_Token": "tok-123", "GCORE_Key": "********" }),
        );
        let (method, params) = map_admin("cert", "dnsSave", &save).unwrap();
        let reply = execute(&ctx, &method, &params).unwrap();
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["obj"]["CF_Token"]["set"], true);
        assert_eq!(reply["obj"]["GCORE_Key"]["set"], false);
        assert!(!reply.to_string().contains("tok-123"));
        assert_eq!(ctx.store.lock().unwrap().db.setting("CF_Token"), "tok-123");
    }

    #[test]
    fn query_parameters_travel_in_the_wire_format() {
        let req = admin_req(
            "log",
            "tail",
            &[("name", "error.log"), ("lines", "50")],
            Value::Null,
        );
        let (method, params) = map_admin("log", "tail", &req).unwrap();
        assert_eq!(method, "log.tail");
        assert_eq!(params["query"]["name"], "error.log");
        let (query, _) = split_params(&params);
        assert_eq!(query.get("lines").map(String::as_str), Some("50"));
    }
}
