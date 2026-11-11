//! Whitelisted RPC methods the agent will run. This is not a remote shell.

use crate::store::{bool_of, str_of, Db};
use serde_json::{json, Value};

pub const ALLOWED: &[&str] = &[
    "ping",
    "sysinfo",
    "nginx.status",
    "nginx.test",
    "nginx.reload",
    "nginx.signal",
    "site.list",
    "site.preview",
    "site.apply",
    "site.detail",
    "conf.show",
    "log.tail",
    "acme.status",
    "acme.apply",
];

pub fn is_allowed(method: &str) -> bool {
    ALLOWED.contains(&method)
}

/// Execute a whitelist RPC against the local machine (agent or panel).
pub fn execute(db: &mut Db, method: &str, params: &Value) -> Result<Value, String> {
    if !is_allowed(method) {
        return Err("unsupportedMethod".into());
    }
    match method {
        "ping" => Ok(json!({ "pong": true, "version": crate::VERSION })),
        "sysinfo" => Ok(crate::sysinfo::snapshot(db)),
        "nginx.status" => Ok(json!({
            "running": crate::nginx::running(db),
            "nginxPath": crate::nginx::nginx_path(db).display().to_string(),
            "nginxExe": crate::nginx::nginx_exe(db),
        })),
        "nginx.test" => {
            let (ok, output) = crate::nginx::test_conf(db);
            Ok(json!({ "ok": ok, "output": output }))
        }
        "nginx.reload" => {
            let (ok, output) = crate::nginx::reload(db);
            Ok(json!({ "ok": ok, "output": output }))
        }
        "nginx.signal" => {
            let action = str_of(params, "action");
            if action.is_empty() {
                return Err("badAction".into());
            }
            let (ok, output) = crate::nginx::signal(db, &action);
            Ok(json!({ "ok": ok, "output": output, "action": action }))
        }
        "site.list" => {
            let _ = crate::compat::adopt(db);
            let list: Vec<Value> = crate::store::ordered(db, "site", true)
                .into_iter()
                .map(|row| summarize_site(db, row))
                .collect();
            Ok(Value::Array(list))
        }
        "site.detail" => {
            let id = str_of(params, "id");
            let mut row = db.get("site", &id).unwrap_or(Value::Null);
            if row.is_object() {
                let locs = crate::site::locations_of(db, &row);
                if let Value::Object(map) = &mut row {
                    map.insert("locations".into(), Value::Array(locs));
                }
            }
            let preview = if row.is_object() {
                crate::site::existing_conf_text(db, &row)
                    .unwrap_or_else(|| crate::site::render(db, &row).unwrap_or_default())
            } else {
                String::new()
            };
            let domain = str_of(&row, "domain");
            Ok(json!({
                "site": row,
                "preview": preview,
                "matchedCert": crate::site::find_cert(db, &domain),
                "sslFiles": crate::site::list_ssl_files(db),
                "webRoot": crate::site::web_root_base(db).display().to_string(),
            }))
        }
        "site.preview" => {
            let site = params
                .get("site")
                .cloned()
                .unwrap_or_else(|| params.clone());
            let text = crate::site::render(db, &site)?;
            Ok(json!({ "conf": text }))
        }
        "site.apply" => {
            let mut site = params
                .get("site")
                .cloned()
                .unwrap_or_else(|| params.clone());
            if let Value::Object(map) = &mut site {
                map.remove("apply");
            }
            let id = db.upsert("site", site.clone());
            let saved = db.get("site", &id).unwrap_or(site);
            match crate::site::apply(db, &saved) {
                Ok((path, output)) => Ok(json!({
                    "id": id,
                    "path": path,
                    "output": output,
                    "ok": true
                })),
                Err(err) => Err(err),
            }
        }
        "conf.show" => {
            let preview = crate::nginx::preview(db);
            let live_path = crate::nginx::nginx_path(db);
            let live = crate::privfs::read_to_string(&live_path).unwrap_or_default();
            Ok(json!({
                "conf": live,
                "generated": preview.conf,
                "generatedMark": crate::conf::GENERATED_MARK,
                "canOverwrite": live.trim().is_empty() || live.contains(crate::conf::GENERATED_MARK),
                "nginxPath": live_path.display().to_string(),
                "nginxExe": crate::nginx::nginx_exe(db),
                "running": crate::nginx::running(db),
            }))
        }
        "log.tail" => {
            let name = {
                let raw = str_of(params, "name");
                if raw.is_empty() {
                    "error.log".into()
                } else {
                    raw
                }
            };
            if name.contains('/') || name.contains('\\') || name.contains("..") {
                return Err("badName".into());
            }
            let lines = params
                .get("lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(200)
                .min(2000) as usize;
            let path = crate::nginx::find_log(db, &name);
            let text = crate::nginx::tail(&path, 256 * 1024, lines);
            Ok(json!({ "name": name, "text": text }))
        }
        "acme.status" => {
            let st = crate::acme::status(db);
            Ok(json!({
                "found": st.found,
                "path": st.path,
                "version": st.version,
            }))
        }
        "acme.apply" => {
            let row = params
                .get("cert")
                .cloned()
                .unwrap_or_else(|| params.clone());
            let force = bool_of(params, "force", false);
            let (ok, output, updated) = crate::acme::apply(db, &row, force);
            Ok(json!({ "ok": ok, "output": output, "cert": updated }))
        }
        _ => Err("unsupportedMethod".into()),
    }
}

fn summarize_site(db: &Db, row: Value) -> Value {
    let locs = crate::site::locations_of(db, &row);
    let mut item = row.clone();
    if let Value::Object(map) = &mut item {
        map.insert("kind".into(), crate::site::site_kind(&row).into());
        map.insert("locations".into(), Value::Array(locs.clone()));
        map.insert(
            "locationSummary".into(),
            locs.iter()
                .map(|l| format!("{} {}", crate::site::loc_type(l), str_of(l, "path")))
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
        );
    }
    item
}

/// Map an authenticated panel API call onto a remote RPC, if it is allowed.
pub fn map_admin(kind: &str, action: &str, req: &crate::api::Req) -> Option<(String, Value)> {
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    };
    let q = |key: &str| req.query.get(key).cloned().unwrap_or_default();
    match (kind, action) {
        ("main", "monitor") => Some(("sysinfo".into(), json!({}))),
        ("conf", "show") => Some(("conf.show".into(), json!({}))),
        ("conf", "check") => Some(("nginx.test".into(), json!({}))),
        ("conf", "reload") => Some(("nginx.reload".into(), json!({}))),
        ("conf", "status") => Some(("nginx.status".into(), json!({}))),
        ("conf", "signal") => Some((
            "nginx.signal".into(),
            json!({ "action": str_of(&body, "action") }),
        )),
        ("site", "list") => Some(("site.list".into(), json!({}))),
        ("site", "detail") => Some(("site.detail".into(), json!({ "id": q("id") }))),
        ("site", "preview") => Some(("site.preview".into(), body)),
        ("site", "apply") => Some(("site.apply".into(), body)),
        ("site", "save") => {
            if bool_of(&body, "apply", false) {
                Some(("site.apply".into(), body))
            } else {
                Some(("site.preview".into(), body))
            }
        }
        ("log", "tail") => Some((
            "log.tail".into(),
            json!({
                "name": q("name"),
                "lines": q("lines").parse::<u64>().unwrap_or(200),
            }),
        )),
        ("cert", "status") => Some(("acme.status".into(), json!({}))),
        ("cert", "apply") | ("cert", "renew") => Some(("acme.apply".into(), body)),
        _ => None,
    }
}

pub fn stays_on_panel(kind: &str, action: &str) -> bool {
    matches!(
        kind,
        "login" | "admin" | "node" | "setting" | "export" | "bak"
    ) || (kind == "log" && action == "operate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_methods() {
        let mut db = Db::new();
        assert_eq!(
            execute(&mut db, "shell.exec", &json!({"cmd": "id"})).unwrap_err(),
            "unsupportedMethod"
        );
        assert_eq!(
            execute(&mut db, "runCmd", &json!({})).unwrap_err(),
            "unsupportedMethod"
        );
        assert!(!is_allowed("rm"));
    }

    #[test]
    fn ping_does_not_need_nginx() {
        let mut db = Db::new();
        let value = execute(&mut db, "ping", &json!({})).unwrap();
        assert_eq!(value["pong"], true);
    }
}
