//! The JSON API the web pages talk to. Each entry point answers with the same
//! envelope the Java version used: `{ success, obj, msg }`.

use crate::auth::{captcha_svg, encode_pass, random_code, Sessions};
use crate::nginx;
use crate::store::{bool_of, int_of, new_id, now_ms, ordered, str_of, Db, Store};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct Ctx {
    pub home: PathBuf,
    pub web_dir: PathBuf,
    pub store: Mutex<Store>,
    pub sessions: Mutex<Sessions>,
}

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
    pub cookies: HashMap<String, String>,
    pub content_type: String,
}

pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub set_cookie: Option<String>,
    pub extra_headers: Vec<(String, String)>,
}

fn json_reply(status: u16, value: &Value) -> Reply {
    Reply {
        status,
        content_type: "application/json; charset=utf-8".to_string(),
        body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        set_cookie: None,
        extra_headers: Vec::new(),
    }
}

pub fn ok(obj: Value) -> Reply {
    json_reply(200, &json!({ "success": true, "obj": obj, "msg": "" }))
}

pub fn ok_empty() -> Reply {
    json_reply(200, &json!({ "success": true, "msg": "" }))
}

pub fn fail(msg: &str) -> Reply {
    json_reply(200, &json!({ "success": false, "msg": msg }))
}

fn body(req: &Req) -> Value {
    if req.body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&req.body).unwrap_or(Value::Null)
}

fn field(value: &Value, key: &str) -> String {
    str_of(value, key)
}

fn rows_with(db: &Db, table: &str, column: &str, id: &str) -> Vec<Value> {
    db.rows(table)
        .iter()
        .filter(|r| str_of(r, column) == id)
        .cloned()
        .collect()
}

const COOKIE: &str = "nginxwebui_token";

fn token_of(req: &Req) -> String {
    req.cookies.get(COOKIE).cloned().unwrap_or_default()
}

fn with_cookie(mut reply: Reply, token: &str) -> Reply {
    reply.set_cookie = Some(format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=28800",
        COOKIE, token
    ));
    reply
}

// ------------------------------------------------------------------ dispatch

pub fn handle(ctx: &Arc<Ctx>, req: &Req) -> Reply {
    let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 || parts[0] != "adminPage" {
        return fail("not found");
    }
    let kind = parts[1];
    let action = parts[2];

    // sign-in endpoints are open; everything else needs a signed-in session
    if kind == "login" {
        return login(ctx, action, req);
    }

    let token = token_of(req);
    {
        let mut sessions = ctx.sessions.lock().unwrap();
        if !sessions.is_signed_in(&token) {
            let mut reply = json_reply(
                401,
                &json!({ "success": false, "msg": "not signed in", "needLogin": true }),
            );
            reply.status = 401;
            if token.is_empty() {
                let fresh = sessions.new_session();
                reply = with_cookie(reply, &fresh);
            }
            return reply;
        }
        sessions.touch(&token);
    }

    match (kind, action) {
        ("main", "monitor") => monitor(ctx),
        ("login", _) => fail("not found"),
        ("conf", _) => conf(ctx, action, req),
        ("bak", _) => bak(ctx, action, req),
        ("log", _) => log(ctx, action, req),
        ("setting", _) => setting(ctx, action, req),
        ("export", _) => export(ctx, action, req),
        ("site", _) => site(ctx, action, req),
        ("server", _) => server(ctx, action, req),
        ("upstream", _) => upstream(ctx, action, req),
        ("template", _) => template(ctx, action, req),
        ("admin", _) => admin(ctx, action, req),
        ("password", _) => password(ctx, action, req),
        ("cert", _) => cert(ctx, action, req),
        ("www", _) => www(ctx, action, req),
        ("install", _) => install(ctx, action, req),
        _ => simple_table(ctx, kind, action, req),
    }
}

// --------------------------------------------------------------------- login

fn login(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    let token = token_of(req);
    match action {
        "info" => {
            let store = ctx.store.lock().unwrap();
            let admin_count = store.db.rows("admin").len();
            let lang = {
                let value = store.db.setting("lang");
                if value.is_empty() {
                    "zh".to_string()
                } else {
                    value
                }
            };
            let sessions = ctx.sessions.lock().unwrap();
            let signed_in = sessions.is_signed_in(&token);
            let name = sessions
                .get(&token)
                .map(|s| s.admin_name.clone())
                .unwrap_or_default();
            drop(sessions);
            ok(json!({
                "adminCount": admin_count,
                "needInit": admin_count == 0,
                "signedIn": signed_in,
                "name": name,
                "lang": lang,
                "version": crate::VERSION,
                "javaFree": true,
            }))
        }
        "getCode" => {
            let code = random_code();
            let mut sessions = ctx.sessions.lock().unwrap();
            let token = if sessions.touch(&token) {
                token.clone()
            } else {
                sessions.new_session()
            };
            sessions.set_code(&token, &code);
            let mut reply = Reply {
                status: 200,
                content_type: "image/svg+xml; charset=utf-8".to_string(),
                body: captcha_svg(&code).into_bytes(),
                set_cookie: None,
                extra_headers: vec![("Cache-Control".to_string(), "no-store".to_string())],
            };
            reply.set_cookie = Some(format!(
                "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=28800",
                COOKIE, token
            ));
            reply
        }
        "login" => {
            let body = body(req);
            let name = field(&body, "name");
            let pass = field(&body, "pass");
            let code = field(&body, "code");

            let mut sessions = ctx.sessions.lock().unwrap();
            if !sessions.touch(&token) {
                return fail("codeError");
            }
            let expected = sessions
                .get(&token)
                .map(|s| s.code.clone())
                .unwrap_or_default();
            if expected.is_empty()
                || code.trim().is_empty()
                || expected.to_lowercase() != code.to_lowercase()
            {
                return fail("codeError");
            }
            sessions.set_code(&token, "");
            drop(sessions);

            let store = ctx.store.lock().unwrap();
            let admin = store
                .db
                .rows("admin")
                .iter()
                .find(|a| str_of(a, "name") == name)
                .cloned();
            let admin = match admin {
                Some(admin) => admin,
                None => return fail("adminError"),
            };
            if str_of(&admin, "pass") != encode_pass(&pass) {
                return fail("adminError");
            }
            drop(store);

            let mut sessions = ctx.sessions.lock().unwrap();
            let token = if sessions.touch(&token) {
                token.clone()
            } else {
                sessions.new_session()
            };
            sessions.sign_in(&token, &str_of(&admin, "id"), &name);
            with_cookie(ok_empty(), &token)
        }
        "addAdmin" => {
            let body = body(req);
            let name = field(&body, "name");
            let pass = field(&body, "pass");
            let mut store = ctx.store.lock().unwrap();
            if !store.db.rows("admin").is_empty() {
                return fail("adminExists");
            }
            if name.trim().len() < 3 {
                return fail("nameTooShort");
            }
            if pass.len() < 6 {
                return fail("passTooShort");
            }
            let mut admin = Map::new();
            admin.insert("id".into(), new_id().into());
            admin.insert("name".into(), name.trim().into());
            admin.insert("pass".into(), encode_pass(&pass).into());
            admin.insert("type".into(), "0".into());
            store.db.upsert("admin", Value::Object(admin));
            let _ = store.flush();
            ok_empty()
        }
        "loginOut" => {
            let mut sessions = ctx.sessions.lock().unwrap();
            sessions.sign_out(&token);
            ok_empty()
        }
        "changeLang" => {
            let body = body(req);
            let lang = field(&body, "lang");
            let mut store = ctx.store.lock().unwrap();
            store.db.set_setting("lang", if lang == "en" { "en" } else { "zh" });
            let _ = store.flush();
            ok_empty()
        }
        "changePass" => {
            let admin_name = {
                let sessions = ctx.sessions.lock().unwrap();
                if !sessions.is_signed_in(&token) {
                    return fail("not signed in");
                }
                sessions
                    .get(&token)
                    .map(|s| s.admin_name.clone())
                    .unwrap_or_default()
            };
            if admin_name.is_empty() {
                return fail("not signed in");
            }
            let body = body(req);
            let old_pass = field(&body, "oldPass");
            let new_pass = field(&body, "newPass");
            if new_pass.len() < 6 {
                return fail("passTooShort");
            }
            let mut store = ctx.store.lock().unwrap();
            let index = store
                .db
                .rows("admin")
                .iter()
                .position(|a| str_of(a, "name") == admin_name);
            let index = match index {
                Some(index) => index,
                None => return fail("adminError"),
            };
            let current = store.db.rows("admin")[index].clone();
            if str_of(&current, "pass") != encode_pass(&old_pass) {
                return fail("oldPassError");
            }
            let mut updated = current.clone();
            if let Value::Object(map) = &mut updated {
                map.insert("pass".into(), encode_pass(&new_pass).into());
            }
            store.db.upsert("admin", updated);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------- monitor

fn monitor(ctx: &Ctx) -> Reply {
    let store = ctx.store.lock().unwrap();
    let mut snapshot = crate::sysinfo::snapshot(&store.db);
    if let Value::Object(map) = &mut snapshot {
        map.insert("webDir".into(), ctx.web_dir.display().to_string().into());
        map.insert(
            "dataFile".into(),
            ctx.home.join("data/nginxwebui.db").display().to_string().into(),
        );
        map.insert("servers".into(), store.db.rows("server").len().into());
        map.insert("sites".into(), store.db.rows("site").len().into());
        map.insert("upstreams".into(), store.db.rows("upstream").len().into());
        map.insert("acme".into(), crate::acme::summarize(&store.db));
        map.insert(
            "sslDir".into(),
            crate::site::ssl_dir(&store.db).display().to_string().into(),
        );
        map.insert(
            "sitesAvailable".into(),
            crate::site::sites_available(&store.db).display().to_string().into(),
        );
        map.insert("install".into(), crate::install::status(&store.db));
        map.insert("nginxFound".into(), crate::install::nginx_present(&store.db).into());
        map.insert("acmeFound".into(), crate::install::acme_present(&store.db).into());
        map.insert("autoInstall".into(), crate::install::auto_install_enabled(&store.db).into());
        map.insert(
            "webRoot".into(),
            crate::site::web_root_base(&store.db).display().to_string().into(),
        );
        map.insert("canInstall".into(), crate::install::can_install().into());
    }
    ok(snapshot)
}

// ---------------------------------------------------------------------- conf

fn conf(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "show" => {
            let store = ctx.store.lock().unwrap();
            let preview = nginx::preview(&store.db);
            let live_path = nginx::nginx_path(&store.db);
            let live = std::fs::read_to_string(&live_path).unwrap_or_default();
            ok(json!({
                "conf": live,
                "generated": preview.conf,
                "generatedMark": crate::conf::GENERATED_MARK,
                "canOverwrite": live.trim().is_empty() || live.contains(crate::conf::GENERATED_MARK),
                "files": preview.files.iter().map(|f| json!({ "name": f.name, "conf": f.conf })).collect::<Vec<_>>(),
                "nginxPath": live_path.display().to_string(),
                "nginxDir": nginx::nginx_dir(&store.db).display().to_string(),
                "nginxExe": nginx::nginx_exe(&store.db),
                "decompose": nginx::decompose(&store.db),
                "running": nginx::running(&store.db),
                "sitesAvailable": crate::site::sites_available(&store.db).display().to_string(),
                "sitesEnabled": crate::site::sites_enabled(&store.db).display().to_string(),
                "sslDir": crate::site::ssl_dir(&store.db).display().to_string(),
                "sslParams": crate::site::ssl_params(&store.db).display().to_string(),
                "webRoot": crate::site::web_root_base(&store.db).display().to_string(),
                "reloadCmd": store.db.setting("reloadCmd"),
            }))
        }
        "replace" => {
            let body = body(req);
            let conf_text = field(&body, "conf");
            let decompose = bool_of(&body, "decompose", false);
            let files = body
                .get("files")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            let mut store = ctx.store.lock().unwrap();
            store.db.set_setting("decompose", if decompose { "true" } else { "false" });
            // keep a backup so the previous configuration can be restored
            let main_path = nginx::nginx_path(&store.db);
            if let Ok(previous) = std::fs::read_to_string(&main_path) {
                if !previous.trim().is_empty() {
                    let time = now_ms().to_string();
                    let mut bak = Map::new();
                    bak.insert("id".into(), time.clone().into());
                    bak.insert("time".into(), time.clone().into());
                    bak.insert("content".into(), previous.into());
                    store.db.upsert("bak", Value::Object(bak));
                }
            }
            if let Some(parent) = main_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&main_path, conf_text) {
                return fail(&err.to_string());
            }
            if decompose {
                let conf_d = nginx::nginx_dir(&store.db).join("conf.d");
                let _ = std::fs::create_dir_all(&conf_d);
                for file in files {
                    let name = field(&file, "name");
                    if name.is_empty() || name.contains('/') || name.contains("..") {
                        continue;
                    }
                    let _ = std::fs::write(conf_d.join(name), field(&file, "conf"));
                }
            }
            let _ = store.flush();
            ok_empty()
        }
        "check" => {
            let store = ctx.store.lock().unwrap();
            let (good, output) = nginx::test_conf(&store.db);
            ok(json!({ "success": good, "output": output }))
        }
        "reload" => {
            let store = ctx.store.lock().unwrap();
            let (test_ok, check_output) = nginx::test_conf(&store.db);
            if !test_ok {
                return ok(json!({
                    "success": false,
                    "output": check_output,
                    "running": nginx::running(&store.db),
                }));
            }
            let (good, output) = nginx::reload(&store.db);
            ok(json!({
                "success": good,
                "output": format!("{}\n{}", check_output, output),
                "running": nginx::running(&store.db),
            }))
        }
        "status" => {
            let store = ctx.store.lock().unwrap();
            ok(json!({ "running": nginx::running(&store.db) }))
        }
        "signal" => {
            let body = body(req);
            let store = ctx.store.lock().unwrap();
            let (good, output) = nginx::signal(&store.db, &field(&body, "action"));
            ok(json!({ "success": good, "output": output, "running": nginx::running(&store.db) }))
        }
        "saveCmd" => {
            let body = body(req);
            let mut store = ctx.store.lock().unwrap();
            for key in [
                "nginxPath",
                "nginxDir",
                "nginxExe",
                "nginxPrefix",
                "reloadCmd",
                "sitesAvailable",
                "sitesEnabled",
                "sslDir",
                "sslParams",
                "webRoot",
                "acmeSh",
                "acmeHome",
                "acmeEmail",
                "nginxLogDir",
                "CF_Token",
                "CF_Account_ID",
                "GCORE_Key",
                "autoInstall",
            ] {
                if body.get(key).is_some() {
                    store.db.set_setting(key, field(&body, key).trim());
                }
            }
            if body.get("decompose").is_some() {
                let decompose = bool_of(&body, "decompose", false);
                store.db.set_setting("decompose", if decompose { "true" } else { "false" });
            }
            let _ = store.flush();
            ok(json!({
                "nginxPath": nginx::nginx_path(&store.db).display().to_string(),
                "nginxDir": nginx::nginx_dir(&store.db).display().to_string(),
                "nginxExe": nginx::nginx_exe(&store.db),
            }))
        }
        "runCmd" => {
            let body = body(req);
            let command = field(&body, "cmd");
            let store = ctx.store.lock().unwrap();
            let reload = store.db.setting("reloadCmd");
            let allowed = command == reload
                || command.starts_with("nginx ")
                || command == "nginx"
                || command.starts_with(&format!("{} ", nginx::nginx_exe(&store.db)));
            if !allowed || !nginx::command_is_safe(&command) {
                return fail("unsafeCommand");
            }
            let (good, output) = nginx::run_command(&store.db, &command);
            ok(json!({ "success": good, "output": output }))
        }
        "write" => {
            let mut store = ctx.store.lock().unwrap();
            match nginx::write_conf(&mut store) {
                Ok(written) => {
                    let _ = store.flush();
                    ok(json!({ "written": written }))
                }
                Err(err) => fail(&err),
            }
        }
        _ => fail("not found"),
    }
}

// ---------------------------------------------------------------- backups

fn bak(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    let store = ctx.store.lock().unwrap();
    match action {
        "list" => {
            let list: Vec<Value> = store
                .db
                .rows("bak")
                .iter()
                .map(|row| {
                    json!({
                        "id": str_of(row, "id"),
                        "time": str_of(row, "time"),
                        "size": str_of(row, "content").len(),
                    })
                })
                .collect();
            ok(Value::Array(list))
        }
        "detail" => {
            let id = req.query.get("id").cloned().unwrap_or_default();
            ok(store.db.get("bak", &id).unwrap_or(Value::Null))
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------- logging

fn log(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    let store = ctx.store.lock().unwrap();
    let dirs = nginx::log_dirs(&store.db);
    match action {
        "names" => {
            let mut names: Vec<String> = Vec::new();
            for dir in &dirs {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".log") && !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
            names.sort();
            ok(json!({
                "dir": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(" · "),
                "names": names
            }))
        }
        "tail" => {
            let name = req.query.get("name").cloned().unwrap_or_else(|| "error.log".into());
            let lines: usize = req
                .query
                .get("lines")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200);
            if name.contains('/') || name.contains('\\') || name.contains("..") {
                return fail("badName");
            }
            let path = nginx::find_log(&store.db, &name);
            let text = nginx::tail(&path, 256 * 1024, lines.min(2000));
            ok(json!({ "name": name, "text": text }))
        }
        "operate" => {
            let list: Vec<Value> = ordered(&store.db, "operateLog", false);
            ok(Value::Array(list))
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------ settings

fn setting(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "get" => {
            let key = req.query.get("key").cloned().unwrap_or_default();
            let store = ctx.store.lock().unwrap();
            ok(json!({ "key": key, "value": store.db.setting(&key) }))
        }
        "set" => {
            let body = body(req);
            let key = field(&body, "key");
            let value = field(&body, "value");
            if key.is_empty() {
                return fail("badKey");
            }
            let mut store = ctx.store.lock().unwrap();
            store.db.set_setting(&key, &value);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------- export

fn export(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "get" => {
            let store = ctx.store.lock().unwrap();
            let mut out = Map::new();
            for (name, rows) in store.db.t.iter() {
                if name == "admin" {
                    continue; // passwords are never exported
                }
                out.insert(name.clone(), Value::Array(rows.clone()));
            }
            ok(Value::Object(out))
        }
        "set" => {
            let body = body(req);
            let mut store = ctx.store.lock().unwrap();
            if let Value::Object(map) = body {
                for (name, rows) in map {
                    if name == "admin" || store.db.t.get(&name).is_none() {
                        continue;
                    }
                    if let Value::Array(rows) = rows {
                        store.db.t.insert(name, rows);
                    }
                }
            }
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// -------------------------------------------------------- debian-style sites

fn site(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let mut store = ctx.store.lock().unwrap();
            let _ = crate::compat::adopt(&mut store.db);
            let _ = store.flush();
            let list: Vec<Value> = ordered(&store.db, "site", true)
                .into_iter()
                .map(|row| {
                    let locs = crate::site::locations_of(&store.db, &row);
                    let mut item = row.clone();
                    if let Value::Object(map) = &mut item {
                        map.insert("locations".into(), Value::Array(locs.clone()));
                        map.insert(
                            "locationSummary".into(),
                            locs.iter()
                                .map(|l| {
                                    format!(
                                        "{} {}",
                                        crate::site::loc_type(l),
                                        str_of(l, "path")
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                                .into(),
                        );
                    }
                    item
                })
                .collect();
            ok(Value::Array(list))
        }
        "detail" => {
            let id = req.query.get("id").cloned().unwrap_or_default();
            let store = ctx.store.lock().unwrap();
            let mut row = store.db.get("site", &id).unwrap_or(Value::Null);
            if row.is_object() {
                let locs = crate::site::locations_of(&store.db, &row);
                if let Value::Object(map) = &mut row {
                    map.insert("locations".into(), Value::Array(locs));
                }
            }
            let preview = if row.is_object() {
                crate::site::existing_conf_text(&store.db, &row)
                    .unwrap_or_else(|| crate::site::render(&store.db, &row).unwrap_or_default())
            } else {
                String::new()
            };
            let domain = field(&row, "domain");
            ok(json!({
                "site": row,
                "preview": preview,
                "matchedCert": crate::site::find_cert(&store.db, &domain),
                "sslFiles": crate::site::list_ssl_files(&store.db),
                "webRoot": crate::site::web_root_base(&store.db).display().to_string(),
            }))
        }
        "preview" => {
            let body = body(req);
            let store = ctx.store.lock().unwrap();
            match crate::site::render(&store.db, &body) {
                Ok(text) => ok(json!({ "conf": text })),
                Err(err) => fail(&err),
            }
        }
        "findCert" => {
            let domain = req
                .query
                .get("domain")
                .cloned()
                .unwrap_or_else(|| field(&body(req), "domain"));
            let store = ctx.store.lock().unwrap();
            ok(json!({
                "domain": domain,
                "certName": crate::site::find_cert(&store.db, &domain),
                "sslFiles": crate::site::list_ssl_files(&store.db),
            }))
        }
        "save" => {
            let body = body(req);
            let domain = field(&body, "domain").trim().to_string();
            if crate::site::safe_domain(&domain).is_none() {
                return fail("badDomain");
            }
            let apply_now = bool_of(&body, "apply", false);
            let mut row = body.clone();
            if let Value::Object(map) = &mut row {
                map.insert("domain".into(), domain.clone().into());
                map.remove("apply");
                if map.get("ssl").is_none() {
                    map.insert("ssl".into(), true.into());
                }
                if map.get("http2").is_none() {
                    map.insert("http2".into(), true.into());
                }
                if apply_now {
                    map.insert("fromDisk".into(), false.into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let locs = crate::site::locations_of(&store.db, &row);
            if let Value::Object(map) = &mut row {
                map.insert("locations".into(), Value::Array(locs.clone()));
                let mode = if locs.iter().any(|l| crate::site::loc_type(l) == "proxy") {
                    "proxy"
                } else {
                    "static"
                };
                map.insert("mode".into(), mode.into());
            }
            let id = store.db.upsert("site", row.clone());
            let saved = store.db.get("site", &id).unwrap_or(row);
            let mut output = String::new();
            if apply_now {
                if crate::install::should_ensure(&store.db)
                    && !crate::install::nginx_present(&store.db)
                {
                    match crate::install::ensure_nginx(&mut store.db) {
                        Ok(log) => output.push_str(&log),
                        Err(err) => {
                            let _ = store.flush();
                            return fail(&err);
                        }
                    }
                }
                match crate::site::apply(&mut store, &saved) {
                    Ok((path, log)) => {
                        output = format!("{}\n{}\n{}", output, path, log);
                    }
                    Err(err) => {
                        let _ = store.flush();
                        return json_reply(
                            200,
                            &json!({
                                "success": false,
                                "obj": { "id": id },
                                "msg": err
                            }),
                        );
                    }
                }
            }
            let _ = store.flush();
            ok(json!({ "id": id, "output": output }))
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            if let Some(row) = store.db.get("site", &id) {
                crate::site::remove_files(&store.db, &row);
            }
            store.db.delete("site", &id);
            let _ = store.flush();
            let (ok_t, out) = nginx::test_conf(&store.db);
            if ok_t {
                let _ = nginx::reload(&store.db);
            }
            ok(json!({ "tested": ok_t, "output": out }))
        }
        "apply" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            let row = match store.db.get("site", &id) {
                Some(row) => row,
                None => return fail("not found"),
            };
            if crate::install::should_ensure(&store.db)
                && !crate::install::nginx_present(&store.db)
            {
                if let Err(err) = crate::install::ensure_nginx(&mut store.db) {
                    let _ = store.flush();
                    return fail(&err);
                }
            }
            match crate::site::apply(&mut store, &row) {
                Ok((path, log)) => {
                    let _ = store.flush();
                    ok(json!({ "path": path, "output": log }))
                }
                Err(err) => fail(&err),
            }
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------- servers

fn server(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let keywords = req.query.get("keywords").cloned().unwrap_or_default();
            let page: usize = req.query.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
            let limit: usize = req.query.get("limit").and_then(|v| v.parse().ok()).unwrap_or(15);
            let store = ctx.store.lock().unwrap();
            let mut all: Vec<Value> = Vec::new();
            for row in ordered(&store.db, "server", false) {
                let name = str_of(&row, "serverName");
                let descr = str_of(&row, "descr");
                if !keywords.trim().is_empty()
                    && !name.contains(&keywords)
                    && !descr.contains(&keywords)
                {
                    continue;
                }
                let listen = str_of(&row, "listen");
                let count = rows_with(&store.db, "location", "serverId", &str_of(&row, "id")).len();
                let mut item = row.clone();
                if let Value::Object(map) = &mut item {
                    map.insert("locationCount".into(), count.into());
                    map.insert("listenText".into(), listen.into());
                }
                all.push(item);
            }
            let total = all.len();
            let start = (page.saturating_sub(1) * limit).min(total);
            let records: Vec<Value> = all.into_iter().skip(start).take(limit).collect();
            ok(json!({ "records": records, "total": total, "page": page, "limit": limit }))
        }
        "detail" => {
            let id = req.query.get("id").cloned().unwrap_or_default();
            let store = ctx.store.lock().unwrap();
            let server = store.db.get("server", &id).unwrap_or(Value::Null);
            ok(json!({
                "server": server,
                "locations": rows_with(&store.db, "location", "serverId", &id),
                "params": rows_with(&store.db, "param", "serverId", &id),
                "upstreams": store.db.rows("upstream").to_vec(),
                "passwords": store.db.rows("password").to_vec(),
                "denyAllows": store.db.rows("denyAllow").to_vec(),
                "certs": store.db.rows("cert").to_vec(),
                "templates": store.db.rows("template").to_vec(),
            }))
        }
        "save" => {
            let body = body(req);
            let mut server = body.get("server").cloned().unwrap_or(Value::Null);
            if !server.is_object() {
                return fail("badRequest");
            }
            if int_of(&server, "seq") == 0 {
                if let Value::Object(map) = &mut server {
                    map.insert("seq".into(), now_ms().into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("server", server);
            let locations = body
                .get("locations")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            store.db.delete_where("location", "serverId", &id);
            for location in locations {
                let mut location = location;
                if let Value::Object(map) = &mut location {
                    map.insert("serverId".into(), id.clone().into());
                    let missing = map
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true);
                    if missing {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("location", location);
            }
            let params = body
                .get("params")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            store.db.delete_where("param", "serverId", &id);
            for param in params {
                let mut param = param;
                if let Value::Object(map) = &mut param {
                    map.insert("serverId".into(), id.clone().into());
                    let missing = map
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true);
                    if missing {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("param", param);
            }
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "del" => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete("server", id);
                store.db.delete_where("location", "serverId", id);
                store.db.delete_where("param", "serverId", id);
            }
            let _ = store.flush();
            ok_empty()
        }
        "enable" => {
            let body = body(req);
            let id = field(&body, "id");
            let enable = bool_of(&body, "enable", true);
            let mut store = ctx.store.lock().unwrap();
            if let Some(mut row) = store.db.get("server", &id) {
                if let Value::Object(map) = &mut row {
                    map.insert("enable".into(), enable.into());
                }
                store.db.upsert("server", row);
                let _ = store.flush();
            }
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ----------------------------------------------------------------- upsteams

fn upstream(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let store = ctx.store.lock().unwrap();
            let mut list: Vec<Value> = Vec::new();
            for row in ordered(&store.db, "upstream", false) {
                let id = str_of(&row, "id");
                let nodes = rows_with(&store.db, "upstreamServer", "upstreamId", &id);
                let mut item = row.clone();
                if let Value::Object(map) = &mut item {
                    map.insert("nodes".into(), Value::Array(nodes));
                }
                list.push(item);
            }
            ok(Value::Array(list))
        }
        "detail" => {
            let id = req.query.get("id").cloned().unwrap_or_default();
            let store = ctx.store.lock().unwrap();
            ok(json!({
                "upstream": store.db.get("upstream", &id).unwrap_or(Value::Null),
                "nodes": rows_with(&store.db, "upstreamServer", "upstreamId", &id),
                "params": rows_with(&store.db, "param", "upstreamId", &id),
            }))
        }
        "save" => {
            let body = body(req);
            let row = body.get("upstream").cloned().unwrap_or(Value::Null);
            if !row.is_object() {
                return fail("badRequest");
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("upstream", row);
            let nodes = body
                .get("nodes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            store.db.delete_where("upstreamServer", "upstreamId", &id);
            for node in nodes {
                let mut node = node;
                if let Value::Object(map) = &mut node {
                    map.insert("upstreamId".into(), id.clone().into());
                    if map.get("id").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("upstreamServer", node);
            }
            let params = body
                .get("params")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            store.db.delete_where("param", "upstreamId", &id);
            for param in params {
                let mut param = param;
                if let Value::Object(map) = &mut param {
                    map.insert("upstreamId".into(), id.clone().into());
                    if map.get("id").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("param", param);
            }
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "del" => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete("upstream", id);
                store.db.delete_where("upstreamServer", "upstreamId", id);
                store.db.delete_where("param", "upstreamId", id);
            }
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ----------------------------------------------------------------- templates

fn template(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let store = ctx.store.lock().unwrap();
            let mut list = Vec::new();
            for row in ordered(&store.db, "template", true) {
                let id = str_of(&row, "id");
                let params = rows_with(&store.db, "param", "templateId", &id);
                let mut item = row.clone();
                if let Value::Object(map) = &mut item {
                    map.insert("params".into(), Value::Array(params));
                }
                list.push(item);
            }
            ok(Value::Array(list))
        }
        "save" => {
            let body = body(req);
            let row = body.get("template").cloned().unwrap_or(Value::Null);
            if !row.is_object() {
                return fail("badRequest");
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("template", row);
            let params = body
                .get("params")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            store.db.delete_where("param", "templateId", &id);
            for param in params {
                let mut param = param;
                if let Value::Object(map) = &mut param {
                    map.insert("templateId".into(), id.clone().into());
                    if map.get("id").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("param", param);
            }
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "del" => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete("template", id);
                store.db.delete_where("param", "templateId", id);
            }
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------------- admins

fn admin(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let store = ctx.store.lock().unwrap();
            let list: Vec<Value> = store
                .db
                .rows("admin")
                .iter()
                .map(|row| json!({ "id": str_of(row, "id"), "name": str_of(row, "name") }))
                .collect();
            ok(Value::Array(list))
        }
        "save" => {
            let body = body(req);
            let mut row = body.clone();
            if !row.is_object() {
                return fail("badRequest");
            }
            let pass = field(&body, "pass");
            if !pass.is_empty() {
                if let Value::Object(map) = &mut row {
                    map.insert("pass".into(), encode_pass(&pass).into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("admin", row);
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            if store.db.rows("admin").len() <= 1 {
                return fail("lastAdmin");
            }
            store.db.delete("admin", &id);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ---------------------------------------------------------------- passwords

fn password(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let store = ctx.store.lock().unwrap();
            let list: Vec<Value> = ordered(&store.db, "password", true)
                .into_iter()
                .map(|row| {
                    json!({
                        "id": str_of(&row, "id"),
                        "name": str_of(&row, "name"),
                        "pass": str_of(&row, "pass"),
                        "descr": str_of(&row, "descr"),
                        "path": str_of(&row, "path"),
                    })
                })
                .collect();
            ok(Value::Array(list))
        }
        "save" => {
            let body = body(req);
            let name = field(&body, "name").trim().to_string();
            if name.is_empty() || name.contains("..") || name.contains('/') || name.contains('\\') {
                return fail("badName");
            }
            let mut store = ctx.store.lock().unwrap();
            let mut row = body.clone();
            let path = ctx.home.join("password").join(&name);
            if let Value::Object(map) = &mut row {
                map.insert("name".into(), name.clone().into());
                map.insert("path".into(), path.display().to_string().into());
            }
            if let Err(err) =
                nginx::write_password_file(&store, &path, &name, &field(&body, "pass"))
            {
                return fail(&err.to_string());
            }
            let id = store.db.upsert("password", row);
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            if let Some(row) = store.db.get("password", &id) {
                let path = str_of(&row, "path");
                if !path.is_empty() {
                    let _ = std::fs::remove_file(path);
                }
            }
            store.db.delete("password", &id);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// -------------------------------------------------------------------- certs

fn cert(ctx: &Arc<Ctx>, action: &str, req: &Req) -> Reply {
    match action {
        "list" | "scan" => {
            let mut store = ctx.store.lock().unwrap();
            let adopted = crate::compat::adopt(&mut store.db);
            let _ = store.flush();
            let certs: Vec<Value> = ordered(&store.db, "cert", true)
                .into_iter()
                .map(|row| {
                    let mut item = row.clone();
                    if let Value::Object(map) = &mut item {
                        map.insert(
                            "kind".into(),
                            crate::compat::normalize_cert_kind(&row).into(),
                        );
                    }
                    item
                })
                .collect();
            ok(json!({
                "certs": certs,
                "sslFiles": crate::site::list_ssl_files(&store.db),
                "acme": crate::acme::summarize(&store.db),
                "canInstall": crate::install::can_install(),
                "adopted": adopted,
                "job": crate::acme::job_snapshot().to_value(),
            }))
        }
        "status" => {
            let store = ctx.store.lock().unwrap();
            ok(crate::acme::summarize(&store.db))
        }
        "progress" | "job" => ok(crate::acme::job_snapshot().to_value()),
        "save" => {
            let body = body(req);
            let domain = field(&body, "domain").trim().to_string();
            if domain.is_empty() || domain.contains("..") || domain.contains('/') {
                return fail("badDomain");
            }
            let primary = domain
                .split(|c: char| c.is_whitespace() || c == ',')
                .find(|s| !s.is_empty())
                .unwrap_or(&domain);
            let (cert_name, _) = crate::acme::issue_names(primary);
            let store_paths = {
                let store = ctx.store.lock().unwrap();
                crate::site::cert_paths(&store.db, &cert_name)
            };
            let (cer_path, key_path) = store_paths;
            if let Some(parent) = cer_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let pem = field(&body, "pemContent");
            let key = field(&body, "keyContent");
            if !pem.trim().is_empty() {
                let _ = std::fs::write(&cer_path, pem);
            }
            if !key.trim().is_empty() {
                let _ = std::fs::write(&key_path, key);
            }
            let mut row = body.clone();
            if let Value::Object(map) = &mut row {
                map.insert("domain".into(), domain.clone().into());
                map.insert("certName".into(), cert_name.into());
                if cer_path.exists() {
                    map.insert("cer".into(), cer_path.display().to_string().into());
                    map.insert("pem".into(), cer_path.display().to_string().into());
                }
                if key_path.exists() {
                    map.insert("key".into(), key_path.display().to_string().into());
                }
                map.remove("pemContent");
                map.remove("keyContent");
                if map.get("kind").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                    map.insert("kind".into(), "dns_cf".into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("cert", row);
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        "apply" | "renew" => {
            let body = body(req);
            let id = field(&body, "id");
            let force = action == "renew" || bool_of(&body, "force", false);
            let kind = if action == "renew" || force {
                "renew"
            } else {
                "apply"
            };
            let snapshot = {
                let mut store = ctx.store.lock().unwrap();
                if crate::install::should_ensure(&store.db)
                    && !crate::install::acme_present(&store.db)
                {
                    if let Err(err) = crate::install::ensure_acme(&mut store.db) {
                        let _ = store.flush();
                        return fail(&err);
                    }
                    let _ = store.flush();
                }
                match store.db.get("cert", &id) {
                    Some(row) => (store.db.clone(), row),
                    None => return fail("not found"),
                }
            };
            if let Err(existing) = crate::acme::try_begin(kind, &id) {
                return json_reply(
                    200,
                    &json!({
                        "success": false,
                        "obj": existing.to_value(),
                        "msg": "jobRunning"
                    }),
                );
            }
            let ctx = Arc::clone(ctx);
            std::thread::spawn(move || {
                let (ok_apply, log, updated) =
                    crate::acme::apply(&snapshot.0, &snapshot.1, force);
                {
                    let mut store = ctx.store.lock().unwrap();
                    store.db.upsert("cert", updated);
                    let _ = store.flush();
                }
                crate::acme::job_finish(ok_apply, &log);
            });
            ok(crate::acme::job_snapshot().to_value())
        }
        "cron" => {
            if let Err(existing) = crate::acme::try_begin("cron", "") {
                return json_reply(
                    200,
                    &json!({
                        "success": false,
                        "obj": existing.to_value(),
                        "msg": "jobRunning"
                    }),
                );
            }
            let snapshot = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            std::thread::spawn(move || {
                let (ok_cron, log) = crate::acme::cron(&snapshot);
                crate::acme::job_finish(ok_cron, &log);
            });
            ok(crate::acme::job_snapshot().to_value())
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            if let Some(row) = store.db.get("cert", &id) {
                let imported = bool_of(&row, "imported", false);
                let source = str_of(&row, "source");
                let keep = imported
                    || source == "acme.sh"
                    || source == "certbot"
                    || source == "nginxWebUI";
                if !keep {
                    for key in ["pem", "cer", "key"] {
                        let path = str_of(&row, key);
                        if crate::compat::cert_file_owned(&store.db, &path) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
            }
            store.db.delete("cert", &id);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

// ------------------------------------------------------------- static folders

fn www(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "list" => {
            let store = ctx.store.lock().unwrap();
            let list: Vec<Value> = ordered(&store.db, "www", true)
                .into_iter()
                .map(|row| {
                    let path = PathBuf::from(str_of(&row, "path"));
                    let info = crate::www::dir_info(&path);
                    let mut item = row.clone();
                    if let Value::Object(map) = &mut item {
                        if let Value::Object(info_map) = info {
                            for (k, v) in info_map {
                                map.insert(k, v);
                            }
                        }
                    }
                    item
                })
                .collect();
            ok(json!({
                "list": list,
                "webRoot": crate::site::web_root_base(&store.db).display().to_string(),
            }))
        }
        "save" | "upload" => {
            let (fields, zip_bytes) = if req.content_type.to_lowercase().contains("multipart/") {
                match crate::www::parse_multipart(&req.body, &req.content_type) {
                    Ok(v) => v,
                    Err(err) => return fail(&err),
                }
            } else {
                let body = body(req);
                let zip = body
                    .get("zipBase64")
                    .and_then(|v| v.as_str())
                    .and_then(|s| decode_base64(s))
                    .unwrap_or_default();
                (body, zip)
            };
            let dir_raw = {
                let d = field(&fields, "dir");
                if d.is_empty() {
                    req.query.get("dir").cloned().unwrap_or_default()
                } else {
                    d
                }
            };
            let descr = field(&fields, "descr");
            let store = ctx.store.lock().unwrap();
            let dest = match crate::www::resolve_dir(&store.db, &dir_raw) {
                Ok(p) => p,
                Err(err) => return fail(&err),
            };
            drop(store);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut written: Vec<String> = Vec::new();
            if !zip_bytes.is_empty() {
                match crate::www::unzip_bytes(&zip_bytes, &dest) {
                    Ok(files) => written = files,
                    Err(err) => return fail(&err),
                }
            } else {
                let _ = std::fs::create_dir_all(&dest);
            }
            let mut row = fields.clone();
            if let Value::Object(map) = &mut row {
                map.insert("dir".into(), dest.display().to_string().into());
                map.insert("path".into(), dest.display().to_string().into());
                if !descr.is_empty() {
                    map.insert("descr".into(), descr.into());
                }
                map.remove("zipBase64");
            }
            let mut store = ctx.store.lock().unwrap();
            let dest_s = dest.display().to_string();
            if let Some(existing) = store
                .db
                .rows("www")
                .iter()
                .find(|r| str_of(r, "dir") == dest_s || str_of(r, "path") == dest_s)
                .cloned()
            {
                if let Value::Object(map) = &mut row {
                    map.insert("id".into(), str_of(&existing, "id").into());
                }
            }
            let id = store.db.upsert("www", row);
            let _ = store.flush();
            ok(json!({
                "id": id,
                "path": dest.display().to_string(),
                "files": written,
            }))
        }
        "clean" => {
            let id = field(&body(req), "id");
            let store = ctx.store.lock().unwrap();
            let row = match store.db.get("www", &id) {
                Some(row) => row,
                None => return fail("not found"),
            };
            let path = PathBuf::from(str_of(&row, "path"));
            if crate::www::resolve_dir(&store.db, &path.display().to_string()).is_err() {
                return fail("dirNotAllowed");
            }
            drop(store);
            if let Err(err) = crate::www::clean_dir(&path) {
                return fail(&err);
            }
            ok_empty()
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            store.db.delete("www", &id);
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}

fn install(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "status" => {
            let store = ctx.store.lock().unwrap();
            ok(crate::install::status(&store.db))
        }
        "nginx" => {
            let mut store = ctx.store.lock().unwrap();
            match crate::install::install_nginx(&mut store.db) {
                Ok(log) => {
                    let _ = store.flush();
                    ok(json!({ "output": log, "status": crate::install::status(&store.db) }))
                }
                Err(err) => fail(&err),
            }
        }
        "acme" => {
            let mut store = ctx.store.lock().unwrap();
            match crate::install::install_acme(&mut store.db) {
                Ok(log) => {
                    let _ = store.flush();
                    ok(json!({ "output": log, "status": crate::install::status(&store.db) }))
                }
                Err(err) => fail(&err),
            }
        }
        "auto" => {
            let body = body(req);
            let enable = bool_of(&body, "enable", true);
            let mut store = ctx.store.lock().unwrap();
            store
                .db
                .set_setting("autoInstall", if enable { "true" } else { "false" });
            let _ = store.flush();
            ok(json!({ "autoInstall": enable }))
        }
        _ => fail("not found"),
    }
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = Vec::new();
    let bytes = cleaned.as_bytes();
    let table = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    };
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = table(bytes[i])?;
        let b = table(bytes[i + 1])?;
        let c = table(bytes[i + 2])?;
        let d = table(bytes[i + 3])?;
        out.push((a << 2) | (b >> 4));
        if bytes[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if bytes[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    Some(out)
}

// -------------------------------------------- plain tables (basic, http, ...)

fn table_name(kind: &str) -> Option<&'static str> {
    match kind {
        "basic" => Some("basic"),
        "http" => Some("http"),
        "stream" => Some("stream"),
        "denyAllow" => Some("denyAllow"),
        _ => None,
    }
}

fn simple_table(ctx: &Ctx, kind: &str, action: &str, req: &Req) -> Reply {
    let table = match table_name(kind) {
        Some(table) => table,
        None => return fail("not found"),
    };
    match (req.method.as_str(), action) {
        ("GET", "list") => {
            let store = ctx.store.lock().unwrap();
            ok(Value::Array(ordered(&store.db, table, true)))
        }
        ("POST", "save") => {
            let body = body(req);
            if !body.is_object() {
                return fail("badRequest");
            }
            let mut row = body;
            let mut store = ctx.store.lock().unwrap();
            if int_of(&row, "seq") == 0 {
                if let Value::Object(map) = &mut row {
                    map.insert("seq".into(), now_ms().into());
                }
            }
            let id = store.db.upsert(table, row);
            let _ = store.flush();
            ok(json!({ "id": id }))
        }
        ("POST", "del") => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete(table, id);
            }
            let _ = store.flush();
            ok_empty()
        }
        _ => fail("not found"),
    }
}
