//! The JSON API the web pages talk to. Each entry point answers with the same
//! envelope the Java version used: `{ success, obj, msg }`.

use crate::auth::{
    captcha_svg, codes_match, first_admin_allowed, hash_password, password_needs_upgrade,
    random_code, verify_password, RateLimiter, Sessions,
};
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
    pub limits: Mutex<RateLimiter>,
    pub init_token: Mutex<Option<String>>,
    pub secure_cookie: bool,
}

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
    pub cookies: HashMap<String, String>,
    pub content_type: String,
    pub remote_ip: String,
    pub secure_cookie: bool,
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

fn id_of(value: &Value) -> String {
    str_of(value, "id")
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

fn cookie_header(req: &Req, token: &str) -> String {
    let mut cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=28800",
        COOKIE, token
    );
    if req.secure_cookie {
        cookie.push_str("; Secure");
    }
    cookie
}

fn with_cookie(mut reply: Reply, req: &Req, token: &str) -> Reply {
    reply.set_cookie = Some(cookie_header(req, token));
    reply
}

fn persist(store: &Store) -> Result<(), Reply> {
    persist_result(store.flush())
}

fn persist_result(result: std::io::Result<()>) -> Result<(), Reply> {
    result.map_err(|err| fail(&format!("saveFailed: {err}")))
}

fn actor(ctx: &Ctx, req: &Req) -> String {
    let token = token_of(req);
    ctx.sessions
        .lock()
        .unwrap()
        .get(&token)
        .map(|s| s.admin_name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "system".into())
}

pub fn record_system_operate(
    store: &mut Store,
    action: &str,
    target: &str,
    ok: bool,
    detail: &str,
) {
    record_operate(store, "system", action, target, ok, detail);
}

fn record_operate(
    store: &mut Store,
    who: &str,
    action: &str,
    target: &str,
    ok: bool,
    detail: &str,
) {
    let detail: String = detail.chars().take(2000).collect();
    crate::applog::info(format!(
        "operate who={who} action={action} target={target} ok={ok}"
    ));
    let mut row = Map::new();
    row.insert("id".into(), new_id().into());
    row.insert("adminName".into(), who.into());
    row.insert("type".into(), action.into());
    row.insert("name".into(), target.into());
    row.insert("success".into(), if ok { "true" } else { "false" }.into());
    row.insert("msg".into(), detail.into());
    row.insert("time".into(), now_ms().to_string().into());
    store.db.upsert("operateLog", Value::Object(row));
    let mut ids: Vec<(String, i64)> = store
        .db
        .rows("operateLog")
        .iter()
        .map(|r| (str_of(r, "id"), int_of(r, "time")))
        .collect();
    if ids.len() > 200 {
        ids.sort_by_key(|item| item.1);
        let extra = ids.len() - 200;
        for (id, _) in ids.into_iter().take(extra) {
            store.db.delete("operateLog", &id);
        }
    }
}

// ------------------------------------------------------------------ dispatch

pub fn handle(ctx: &Arc<Ctx>, req: &Req) -> Reply {
    let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 || parts[0] != "adminPage" {
        return fail("not found");
    }
    let kind = parts[1];
    let action = parts[2];

    // only a short allow-list is public; changeLang / changePass require a session
    if kind == "login"
        && matches!(
            action,
            "info" | "getCode" | "login" | "addAdmin" | "loginOut"
        )
    {
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
                reply = with_cookie(reply, req, &fresh);
            }
            return reply;
        }
        sessions.touch(&token);
    }

    match (kind, action) {
        ("main", "monitor") => monitor(ctx),
        ("login", _) => login(ctx, action, req),
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
            let session = sessions.get(&token);
            let name = session.map(|s| s.admin_name.clone()).unwrap_or_default();
            let admin_id = session.map(|s| s.admin_id.clone()).unwrap_or_default();
            drop(sessions);
            ok(json!({
                "adminCount": admin_count,
                "needInit": admin_count == 0,
                "needInitToken": admin_count == 0,
                "signedIn": signed_in,
                "name": name,
                "adminId": admin_id,
                "lang": lang,
                "version": crate::VERSION,
                "javaFree": true,
            }))
        }
        "getCode" => {
            {
                let mut limits = ctx.limits.lock().unwrap();
                if let Err(msg) = limits.allow_captcha(&req.remote_ip) {
                    return fail(msg);
                }
            }
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
            reply.set_cookie = Some(cookie_header(req, &token));
            reply
        }
        "login" => {
            let body = body(req);
            let name = field(&body, "name");
            let pass = field(&body, "pass");
            let code = field(&body, "code");

            {
                let limits = ctx.limits.lock().unwrap();
                if let Err(msg) = limits.allow_login(&req.remote_ip, &name) {
                    return fail(msg);
                }
            }

            let mut sessions = ctx.sessions.lock().unwrap();
            if !sessions.touch(&token) {
                drop(sessions);
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail("codeError");
            }
            let expected = sessions
                .get(&token)
                .map(|s| s.code.clone())
                .unwrap_or_default();
            if expected.is_empty() || !codes_match(&expected, &code) {
                drop(sessions);
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail("codeError");
            }
            sessions.set_code(&token, "");
            drop(sessions);

            let mut store = ctx.store.lock().unwrap();
            let admin = store
                .db
                .rows("admin")
                .iter()
                .find(|a| str_of(a, "name") == name)
                .cloned();
            let admin = match admin {
                Some(admin) => admin,
                None => {
                    drop(store);
                    ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                    return fail("adminError");
                }
            };
            let stored = str_of(&admin, "pass");
            if !verify_password(&pass, &stored) {
                drop(store);
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail("adminError");
            }
            if password_needs_upgrade(&stored) {
                if let Ok(upgraded) = hash_password(&pass) {
                    let mut updated = admin.clone();
                    if let Value::Object(map) = &mut updated {
                        map.insert("pass".into(), upgraded.into());
                    }
                    store.db.upsert("admin", updated);
                    let _ = persist(&store);
                }
            }
            drop(store);

            ctx.limits
                .lock()
                .unwrap()
                .succeed_login(&req.remote_ip, &name);
            let mut sessions = ctx.sessions.lock().unwrap();
            let token = if sessions.touch(&token) {
                token.clone()
            } else {
                sessions.new_session()
            };
            sessions.sign_in(&token, &str_of(&admin, "id"), &name);
            with_cookie(ok_empty(), req, &token)
        }
        "addAdmin" => {
            let body = body(req);
            let name = field(&body, "name");
            let pass = field(&body, "pass");
            let code = field(&body, "code");
            let init = field(&body, "initToken");

            {
                let limits = ctx.limits.lock().unwrap();
                if let Err(msg) = limits.allow_login(&req.remote_ip, &name) {
                    return fail(msg);
                }
            }

            let expected_init = ctx.init_token.lock().unwrap().clone();
            if let Err(msg) = first_admin_allowed(expected_init.as_deref(), &init) {
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail(msg);
            }

            let mut sessions = ctx.sessions.lock().unwrap();
            if !sessions.touch(&token) {
                drop(sessions);
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail("codeError");
            }
            let expected = sessions
                .get(&token)
                .map(|s| s.code.clone())
                .unwrap_or_default();
            if expected.is_empty() || !codes_match(&expected, &code) {
                drop(sessions);
                ctx.limits.lock().unwrap().fail_login(&req.remote_ip, &name);
                return fail("codeError");
            }
            sessions.set_code(&token, "");
            drop(sessions);

            let hashed = match hash_password(&pass) {
                Ok(h) => h,
                Err(_) => return fail("passTooShort"),
            };
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
            admin.insert("pass".into(), hashed.into());
            admin.insert("type".into(), "0".into());
            store.db.upsert("admin", Value::Object(admin));
            record_operate(
                &mut store,
                name.trim(),
                "addAdmin",
                name.trim(),
                true,
                "created",
            );
            if let Err(reply) = persist(&store) {
                return reply;
            }
            drop(store);
            *ctx.init_token.lock().unwrap() = None;
            ctx.limits
                .lock()
                .unwrap()
                .succeed_login(&req.remote_ip, &name);
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
            store
                .db
                .set_setting("lang", if lang == "en" { "en" } else { "zh" });
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok_empty()
        }
        "changePass" => {
            let (admin_name, admin_id) = {
                let sessions = ctx.sessions.lock().unwrap();
                if !sessions.is_signed_in(&token) {
                    return fail("not signed in");
                }
                sessions
                    .get(&token)
                    .map(|s| (s.admin_name.clone(), s.admin_id.clone()))
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
            if !verify_password(&old_pass, &str_of(&current, "pass")) {
                return fail("oldPassError");
            }
            let hashed = match hash_password(&new_pass) {
                Ok(h) => h,
                Err(_) => return fail("passTooShort"),
            };
            let mut updated = current.clone();
            if let Value::Object(map) = &mut updated {
                map.insert("pass".into(), hashed.into());
            }
            store.db.upsert("admin", updated);
            record_operate(
                &mut store,
                &admin_name,
                "changePass",
                &admin_name,
                true,
                "updated",
            );
            if let Err(reply) = persist(&store) {
                return reply;
            }
            drop(store);
            ctx.sessions
                .lock()
                .unwrap()
                .revoke_others(&token, &admin_id);
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
            ctx.home
                .join("data/nginxwebui.db")
                .display()
                .to_string()
                .into(),
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
            crate::site::sites_available(&store.db)
                .display()
                .to_string()
                .into(),
        );
        map.insert("install".into(), crate::install::status(&store.db));
        map.insert(
            "nginxFound".into(),
            crate::install::nginx_present(&store.db).into(),
        );
        map.insert(
            "acmeFound".into(),
            crate::install::acme_present(&store.db).into(),
        );
        map.insert(
            "autoInstall".into(),
            crate::install::auto_install_enabled(&store.db).into(),
        );
        map.insert(
            "webRoot".into(),
            crate::site::web_root_base(&store.db)
                .display()
                .to_string()
                .into(),
        );
        map.insert("canInstall".into(), crate::install::can_install().into());
    }
    ok(snapshot)
}

// ---------------------------------------------------------------------- conf

fn sites_include_obj(db: &crate::store::Db) -> Value {
    match crate::site::peek_sites_include(db) {
        Ok(crate::site::InsertPlanView::NeedsConfirm {
            include_line,
            preview,
            reason,
        }) => json!({
            "status": "needsConfirm",
            "includeLine": include_line,
            "preview": preview,
            "reason": reason,
        }),
        Ok(crate::site::InsertPlanView::Ready { new_text }) => json!({
            "status": "ready",
            "preview": new_text,
        }),
        Ok(crate::site::InsertPlanView::AlreadyPresent) => json!({
            "status": "alreadyPresent",
        }),
        Err(err) => json!({ "status": "error", "error": err }),
    }
}

fn fail_maybe_include(db: &crate::store::Db, err: &str, extra: Value) -> Reply {
    if err == "needsSitesIncludeConfirm" {
        let mut obj = extra;
        if let Value::Object(map) = &mut obj {
            if let Value::Object(src) = sites_include_obj(db) {
                for (k, v) in src {
                    map.insert(k, v);
                }
            }
        }
        return json_reply(
            200,
            &json!({
                "success": false,
                "msg": err,
                "obj": obj,
            }),
        );
    }
    if extra == Value::Null {
        return fail(err);
    }
    json_reply(
        200,
        &json!({
            "success": false,
            "msg": err,
            "obj": extra,
        }),
    )
}

fn conf(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    match action {
        "sitesInclude" => {
            let confirm = bool_of(&body(req), "confirm", false);
            let store = ctx.store.lock().unwrap();
            if confirm {
                match crate::site::apply_sites_include(&store.db, true) {
                    Ok(crate::site::SitesIncludeOutcome::Written) => {
                        ok(json!({ "status": "written" }))
                    }
                    Ok(crate::site::SitesIncludeOutcome::AlreadyPresent) => {
                        ok(json!({ "status": "alreadyPresent" }))
                    }
                    Ok(crate::site::SitesIncludeOutcome::NeedsConfirm {
                        include_line,
                        preview,
                        reason,
                    }) => json_reply(
                        200,
                        &json!({
                            "success": false,
                            "msg": "needsSitesIncludeConfirm",
                            "obj": {
                                "status": "needsConfirm",
                                "includeLine": include_line,
                                "preview": preview,
                                "reason": reason,
                            }
                        }),
                    ),
                    Err(err) => fail(&err),
                }
            } else {
                let obj = sites_include_obj(&store.db);
                if obj.get("status").and_then(|v| v.as_str()) == Some("needsConfirm") {
                    json_reply(
                        200,
                        &json!({
                            "success": false,
                            "msg": "needsSitesIncludeConfirm",
                            "obj": obj,
                        }),
                    )
                } else if obj.get("status").and_then(|v| v.as_str()) == Some("error") {
                    fail(obj.get("error").and_then(|v| v.as_str()).unwrap_or("error"))
                } else {
                    ok(obj)
                }
            }
        }
        "show" => {
            let store = ctx.store.lock().unwrap();
            let preview = nginx::preview(&store.db);
            let live_path = nginx::nginx_path(&store.db);
            let live = crate::privfs::read_to_string(&live_path).unwrap_or_default();
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
            let who = actor(ctx, req);
            let db = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            match nginx::stage_main_conf(&db, &conf_text, true) {
                Err(err) => {
                    let mut store = ctx.store.lock().unwrap();
                    record_operate(&mut store, &who, "conf.replace", "", false, &err);
                    let _ = persist(&store);
                    return fail(&err);
                }
                Ok((temp, _)) => {
                    let mut store = ctx.store.lock().unwrap();
                    store
                        .db
                        .set_setting("decompose", if decompose { "true" } else { "false" });
                    if let Err(err) = nginx::commit_staged_conf(&mut store, &conf_text, &temp) {
                        record_operate(&mut store, &who, "conf.replace", "", false, &err);
                        let _ = persist(&store);
                        return fail(&err);
                    }
                    if decompose {
                        let conf_d = nginx::nginx_dir(&store.db).join("conf.d");
                        if let Err(err) = crate::privfs::mkdir_all(&conf_d) {
                            return fail(&err);
                        }
                        for file in files {
                            let name = field(&file, "name");
                            if name.is_empty() || name.contains('/') || name.contains("..") {
                                continue;
                            }
                            if let Err(err) =
                                crate::privfs::write(&conf_d.join(name), field(&file, "conf"))
                            {
                                return fail(&err);
                            }
                        }
                    }
                    record_operate(&mut store, &who, "conf.replace", "", true, "ok");
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                    ok_empty()
                }
            }
        }
        "check" => {
            let db = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            let (good, output) = nginx::test_conf(&db);
            ok(json!({ "success": good, "output": output }))
        }
        "reload" => {
            let db = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            let (test_ok, check_output) = nginx::test_conf(&db);
            if !test_ok {
                return ok(json!({
                    "success": false,
                    "output": check_output,
                    "running": nginx::running(&db),
                }));
            }
            let (good, output) = nginx::reload(&db);
            ok(json!({
                "success": good,
                "output": format!("{}\n{}", check_output, output),
                "running": nginx::running(&db),
            }))
        }
        "status" => {
            let store = ctx.store.lock().unwrap();
            ok(json!({ "running": nginx::running(&store.db) }))
        }
        "signal" => {
            let body = body(req);
            let action = field(&body, "action");
            let db = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            let (good, output) = nginx::signal(&db, &action);
            ok(json!({ "success": good, "output": output, "running": nginx::running(&db) }))
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
                    let value = field(&body, key);
                    if crate::secrets::is_secret_setting(key)
                        && !crate::secrets::should_store_secret(&value)
                    {
                        continue;
                    }
                    if key == "reloadCmd" {
                        match nginx::normalize_reload_cmd(value.trim()) {
                            Some(cmd) => store.db.set_setting(key, &cmd),
                            None => return fail("unsafeReloadCmd"),
                        }
                        continue;
                    }
                    store.db.set_setting(key, value.trim());
                }
            }
            if body.get("decompose").is_some() {
                let decompose = bool_of(&body, "decompose", false);
                store
                    .db
                    .set_setting("decompose", if decompose { "true" } else { "false" });
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok(json!({
                "nginxPath": nginx::nginx_path(&store.db).display().to_string(),
                "nginxDir": nginx::nginx_dir(&store.db).display().to_string(),
                "nginxExe": nginx::nginx_exe(&store.db),
            }))
        }
        "runCmd" => {
            let body = body(req);
            let mut action = field(&body, "action");
            if action.is_empty() {
                action = field(&body, "cmd");
            }
            let db = {
                let store = ctx.store.lock().unwrap();
                store.db.clone()
            };
            let (good, output) = match action.as_str() {
                "reload"
                | "nginx -s reload"
                | "sudo -n systemctl reload nginx"
                | "sudo systemctl reload nginx"
                | "systemctl reload nginx" => nginx::reload(&db),
                "check" | "test" | "nginx -t" => nginx::test_conf(&db),
                "start" | "stop" | "restart" | "quit" => nginx::signal(&db, &action),
                _ => return fail("unsafeCommand"),
            };
            ok(json!({ "success": good, "output": output }))
        }
        "write" => {
            let who = actor(ctx, req);
            let (db, conf_text, files, split, dir) = {
                let store = ctx.store.lock().unwrap();
                let ext = nginx::preview(&store.db);
                (
                    store.db.clone(),
                    ext.conf,
                    ext.files,
                    nginx::decompose(&store.db),
                    nginx::nginx_dir(&store.db),
                )
            };
            match nginx::stage_main_conf(&db, &conf_text, true) {
                Err(err) => {
                    let mut store = ctx.store.lock().unwrap();
                    record_operate(&mut store, &who, "conf.write", "", false, &err);
                    let _ = persist(&store);
                    fail(&err)
                }
                Ok((temp, _)) => {
                    let mut store = ctx.store.lock().unwrap();
                    if let Err(err) = nginx::commit_staged_conf(&mut store, &conf_text, &temp) {
                        record_operate(&mut store, &who, "conf.write", "", false, &err);
                        let _ = persist(&store);
                        return fail(&err);
                    }
                    let mut written = vec![nginx::nginx_path(&store.db).display().to_string()];
                    let conf_d = dir.join("conf.d");
                    if let Err(err) = crate::privfs::mkdir_all(&conf_d) {
                        return fail(&err);
                    }
                    let mut wanted: Vec<String> = Vec::new();
                    if split {
                        for file in &files {
                            if let Err(err) =
                                crate::privfs::write(&conf_d.join(&file.name), &file.conf)
                            {
                                return fail(&err);
                            }
                            wanted.push(file.name.clone());
                            written.push(conf_d.join(&file.name).display().to_string());
                        }
                    }
                    record_operate(&mut store, &who, "conf.write", "", true, "ok");
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                    ok(json!({ "written": written }))
                }
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
        "restore" => {
            drop(store);
            let id = field(&body(req), "id");
            if id.is_empty() {
                return fail("not found");
            }
            let store = ctx.store.lock().unwrap();
            let Some(row) = store.db.get("bak", &id) else {
                return fail("not found");
            };
            let content = str_of(&row, "content");
            if content.trim().is_empty() {
                return fail("emptyBackup");
            }
            let db = store.db.clone();
            drop(store);
            let who = actor(ctx, req);
            match nginx::stage_main_conf(&db, &content, false) {
                Err(err) => {
                    let mut store = ctx.store.lock().unwrap();
                    record_operate(&mut store, &who, "bak.restore", &id, false, &err);
                    let _ = persist(&store);
                    fail(&err)
                }
                Ok((temp, output)) => {
                    let mut store = ctx.store.lock().unwrap();
                    if let Err(err) = nginx::commit_staged_conf(&mut store, &content, &temp) {
                        record_operate(&mut store, &who, "bak.restore", &id, false, &err);
                        let _ = persist(&store);
                        return fail(&err);
                    }
                    record_operate(&mut store, &who, "bak.restore", &id, true, "ok");
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                    ok(json!({ "output": output }))
                }
            }
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
                if let Ok(entries) = crate::privfs::list_names(dir) {
                    for name in entries {
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
            let name = req
                .query
                .get("name")
                .cloned()
                .unwrap_or_else(|| "error.log".into());
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
            ok(crate::secrets::setting_view(&key, &store.db.setting(&key)))
        }
        "set" => {
            let body = body(req);
            let key = field(&body, "key");
            let value = field(&body, "value");
            if key.is_empty() {
                return fail("badKey");
            }
            if crate::secrets::is_secret_setting(&key)
                && !crate::secrets::should_store_secret(&value)
            {
                return ok_empty();
            }
            let mut store = ctx.store.lock().unwrap();
            store.db.set_setting(&key, &value);
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                let mut cloned = rows.clone();
                if name == "cert" {
                    for row in &mut cloned {
                        crate::secrets::strip_object_secrets(row, crate::secrets::SECRET_CERT_KEYS);
                    }
                }
                if name == "password" {
                    for row in &mut cloned {
                        if let Value::Object(map) = row {
                            map.remove("pass");
                        }
                    }
                }
                out.insert(name.clone(), Value::Array(cloned));
            }
            crate::secrets::mask_settings_in_export(&mut out);
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
            let list: Vec<Value> = ordered(&store.db, "site", true)
                .into_iter()
                .map(|row| {
                    let locs = crate::site::locations_of(&store.db, &row);
                    let mut item = row.clone();
                    if let Value::Object(map) = &mut item {
                        map.insert("kind".into(), crate::site::site_kind(&row).into());
                        map.insert("locations".into(), Value::Array(locs.clone()));
                        map.insert(
                            "locationSummary".into(),
                            locs.iter()
                                .map(|l| {
                                    format!("{} {}", crate::site::loc_type(l), str_of(l, "path"))
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
            let apply_now = bool_of(&body, "apply", false);
            let confirm_include = bool_of(&body, "confirmInclude", false);
            let mut row = body.clone();
            let kind = crate::site::site_kind(&row);
            let mut domain = field(&body, "domain").trim().to_string();
            if kind == "forward" && crate::site::safe_domain(&domain).is_none() {
                let listen = field(&body, "listen");
                let port = if listen.trim().is_empty() {
                    "8888"
                } else {
                    listen.trim()
                };
                domain = format!("forward-{port}");
            }
            if crate::site::safe_domain(&domain).is_none() {
                return fail("badDomain");
            }
            if let Value::Object(map) = &mut row {
                map.insert("domain".into(), domain.clone().into());
                map.insert("kind".into(), kind.clone().into());
                map.remove("apply");
                map.remove("confirmInclude");
                if kind == "forward" {
                    map.insert("ssl".into(), false.into());
                    map.insert("redirect".into(), false.into());
                    map.insert("http2".into(), false.into());
                    map.insert("mode".into(), "forward".into());
                    let listen_empty = map
                        .get("listen")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true);
                    if listen_empty {
                        map.insert("listen".into(), "8888".into());
                    }
                } else {
                    if map.get("ssl").is_none() {
                        map.insert("ssl".into(), true.into());
                    }
                    if map.get("http2").is_none() {
                        map.insert("http2".into(), true.into());
                    }
                    map.insert(
                        "mode".into(),
                        if kind == "reverse" { "proxy" } else { "static" }.into(),
                    );
                }
                if apply_now {
                    map.insert("fromDisk".into(), false.into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let locs = crate::site::locations_of(&store.db, &row);
            if let Value::Object(map) = &mut row {
                map.insert("locations".into(), Value::Array(locs));
            }
            let id = store.db.upsert("site", row.clone());
            let mut saved = store.db.get("site", &id).unwrap_or(row);
            if confirm_include {
                if let Value::Object(map) = &mut saved {
                    map.insert("confirmInclude".into(), true.into());
                }
            }
            let mut output = String::new();
            if apply_now {
                if crate::install::should_ensure(&store.db)
                    && !crate::install::nginx_present(&store.db)
                {
                    match crate::install::ensure_nginx(&mut store.db) {
                        Ok(log) => output.push_str(&log),
                        Err(err) => {
                            if let Err(reply) = persist(&store) {
                                return reply;
                            }
                            return fail(&err);
                        }
                    }
                }
                match crate::site::apply(&store.db, &saved) {
                    Ok((path, log)) => {
                        output = format!("{}\n{}\n{}", output, path, log);
                        record_operate(
                            &mut store,
                            &actor(ctx, req),
                            "site.apply",
                            &id_of(&saved),
                            true,
                            &output,
                        );
                    }
                    Err(err) => {
                        record_operate(
                            &mut store,
                            &actor(ctx, req),
                            "site.apply",
                            &id_of(&saved),
                            false,
                            &err,
                        );
                        if let Err(reply) = persist(&store) {
                            return reply;
                        }
                        return fail_maybe_include(&store.db, &err, json!({ "id": id }));
                    }
                }
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok(json!({ "id": id, "output": output }))
        }
        "del" => {
            let id = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            if let Some(row) = store.db.get("site", &id) {
                crate::site::remove_files(&store.db, &row);
            }
            store.db.delete("site", &id);
            if let Err(reply) = persist(&store) {
                return reply;
            }
            let (ok_t, out) = nginx::test_conf(&store.db);
            if ok_t {
                let _ = nginx::reload(&store.db);
            }
            ok(json!({ "tested": ok_t, "output": out }))
        }
        "apply" => {
            let req_body = body(req);
            let id = field(&req_body, "id");
            let confirm_include = bool_of(&req_body, "confirmInclude", false);
            let who = actor(ctx, req);
            let (row, db) = {
                let mut store = ctx.store.lock().unwrap();
                if crate::install::should_ensure(&store.db)
                    && !crate::install::nginx_present(&store.db)
                {
                    if let Err(err) = crate::install::ensure_nginx(&mut store.db) {
                        if let Err(reply) = persist(&store) {
                            return reply;
                        }
                        return fail(&err);
                    }
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                }
                match store.db.get("site", &id) {
                    Some(mut row) => {
                        if confirm_include {
                            if let Value::Object(map) = &mut row {
                                map.insert("confirmInclude".into(), true.into());
                            }
                        }
                        (row, store.db.clone())
                    }
                    None => return fail("not found"),
                }
            };
            match crate::site::apply(&db, &row) {
                Ok((path, log)) => {
                    let mut store = ctx.store.lock().unwrap();
                    record_operate(&mut store, &who, "site.apply", &id, true, &log);
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                    ok(json!({ "path": path, "output": log }))
                }
                Err(err) => {
                    let mut store = ctx.store.lock().unwrap();
                    record_operate(&mut store, &who, "site.apply", &id, false, &err);
                    let _ = persist(&store);
                    fail_maybe_include(&db, &err, json!({ "id": id }))
                }
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
            let page: usize = req
                .query
                .get("page")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let limit: usize = req
                .query
                .get("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(15);
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                if let Err(reply) = persist(&store) {
                    return reply;
                }
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
                    if map
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
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
                    if map
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("param", param);
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                    if map
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
                        map.insert("id".into(), new_id().into());
                    }
                }
                store.db.upsert("param", param);
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok(json!({ "id": id }))
        }
        "del" => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete("template", id);
                store.db.delete_where("param", "templateId", id);
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            let name = field(&body, "name").trim().to_string();
            if name.is_empty() {
                return fail("nameRequired");
            }
            let pass = field(&body, "pass");
            let id_in = field(&body, "id");
            let mut store = ctx.store.lock().unwrap();
            let mut row = if !id_in.is_empty() {
                store
                    .db
                    .get("admin", &id_in)
                    .unwrap_or_else(|| json!({ "id": id_in }))
            } else {
                json!({})
            };
            if let Value::Object(map) = &mut row {
                map.insert("name".into(), name.into());
                if pass.is_empty() {
                    let existing = map
                        .get("pass")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if existing.is_empty() {
                        return fail("passRequired");
                    }
                } else {
                    let hashed = match hash_password(&pass) {
                        Ok(h) => h,
                        Err(_) => return fail("passTooShort"),
                    };
                    map.insert("pass".into(), hashed.into());
                }
            } else {
                return fail("badRequest");
            }
            let id = store.db.upsert("admin", row);
            record_operate(&mut store, &actor(ctx, req), "admin.save", &id, true, "");
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok(json!({ "id": id }))
        }
        "del" => {
            let id = field(&body(req), "id");
            if id.is_empty() {
                return fail("badRequest");
            }
            let me = {
                let token = token_of(req);
                ctx.sessions
                    .lock()
                    .unwrap()
                    .get(&token)
                    .map(|s| s.admin_id.clone())
                    .unwrap_or_default()
            };
            if !me.is_empty() && me == id {
                return fail("cannotDeleteSelf");
            }
            let mut store = ctx.store.lock().unwrap();
            if store.db.rows("admin").len() <= 1 {
                return fail("lastAdmin");
            }
            store.db.delete("admin", &id);
            record_operate(&mut store, &actor(ctx, req), "admin.del", &id, true, "");
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                        "descr": str_of(&row, "descr"),
                        "path": str_of(&row, "path"),
                        "hasPass": std::path::Path::new(&str_of(&row, "path")).is_file(),
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
                map.remove("pass");
            }
            if let Err(err) =
                nginx::write_password_file(&store, &path, &name, &field(&body, "pass"))
            {
                return fail(&err.to_string());
            }
            let id = store.db.upsert("password", row);
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                    crate::secrets::strip_object_secrets(
                        &mut item,
                        crate::secrets::SECRET_CERT_KEYS,
                    );
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
                if let Err(err) = crate::privfs::mkdir_all(parent) {
                    return fail(&err);
                }
            }
            let pem = field(&body, "pemContent");
            let key = field(&body, "keyContent");
            if !pem.trim().is_empty() {
                if let Err(err) = crate::privfs::write(&cer_path, pem) {
                    return fail(&err);
                }
            }
            if !key.trim().is_empty() {
                if let Err(err) = crate::privfs::write(&key_path, key) {
                    return fail(&err);
                }
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
                for key in crate::secrets::SECRET_CERT_KEYS {
                    map.remove(*key);
                }
                if map
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    map.insert("kind".into(), "dns_cf".into());
                }
            }
            let mut store = ctx.store.lock().unwrap();
            let id = store.db.upsert("cert", row);
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                        if let Err(reply) = persist(&store) {
                            return reply;
                        }
                        return fail(&err);
                    }
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
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
                let (ok_apply, log, updated) = crate::acme::apply(&snapshot.0, &snapshot.1, force);
                {
                    let mut store = ctx.store.lock().unwrap();
                    store.db.upsert("cert", updated);
                    record_operate(&mut store, "system", "cert.apply", &id, ok_apply, &log);
                    if persist(&store).is_err() {
                        crate::acme::job_finish(false, "saveFailed");
                        return;
                    }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                if let Err(err) = crate::privfs::mkdir_www(parent) {
                    return fail(&err);
                }
            }
            let mut written: Vec<String> = Vec::new();
            if !zip_bytes.is_empty() {
                match crate::www::unzip_bytes(&zip_bytes, &dest) {
                    Ok(files) => written = files,
                    Err(err) => return fail(&err),
                }
            } else {
                if let Err(err) = crate::privfs::mkdir_www(&dest) {
                    return fail(&err);
                }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
                    ok(json!({ "output": log, "status": crate::install::status(&store.db) }))
                }
                Err(err) => fail(&err),
            }
        }
        "acme" => {
            let mut store = ctx.store.lock().unwrap();
            match crate::install::install_acme(&mut store.db) {
                Ok(log) => {
                    if let Err(reply) = persist(&store) {
                        return reply;
                    }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
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
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok(json!({ "id": id }))
        }
        ("POST", "del") => {
            let ids = field(&body(req), "id");
            let mut store = ctx.store.lock().unwrap();
            for id in ids.split(',').filter(|s| !s.is_empty()) {
                store.db.delete(table, id);
            }
            if let Err(reply) = persist(&store) {
                return reply;
            }
            ok_empty()
        }
        _ => fail("not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_maps_flush_errors() {
        let err = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
        let reply = persist_result(Err(err)).expect_err("flush error must become API fail");
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["success"], false);
        assert_eq!(body["msg"], "saveFailed: disk full");
    }

    fn test_ctx() -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-api-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let store = Store::open(&dir);
        Arc::new(Ctx {
            home: dir.clone(),
            web_dir: dir,
            store: Mutex::new(store),
            sessions: Mutex::new(Sessions::default()),
            limits: Mutex::new(RateLimiter::default()),
            init_token: Mutex::new(Some("init-secret".into())),
            secure_cookie: false,
        })
    }

    fn test_req(path: &str, payload: Value, token: &str) -> Req {
        let mut cookies = HashMap::new();
        if !token.is_empty() {
            cookies.insert(COOKIE.to_string(), token.to_string());
        }
        Req {
            method: "POST".into(),
            path: path.into(),
            query: HashMap::new(),
            body: serde_json::to_vec(&payload).unwrap_or_default(),
            cookies,
            content_type: "application/json".into(),
            remote_ip: "127.0.0.1".into(),
            secure_cookie: false,
        }
    }

    fn signed_in(ctx: &Ctx) -> String {
        let mut sessions = ctx.sessions.lock().unwrap();
        let token = sessions.new_session();
        sessions.sign_in(&token, "1", "tester");
        token
    }

    fn body_of(reply: &Reply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    #[test]
    fn unsigned_api_returns_401() {
        let ctx = test_ctx();
        let reply = handle(&ctx, &test_req("/adminPage/conf/show", json!({}), ""));
        assert_eq!(reply.status, 401);
        assert_eq!(body_of(&reply)["needLogin"], true);
    }

    #[test]
    fn add_admin_requires_init_token_and_captcha() {
        let ctx = test_ctx();
        let token = {
            let mut sessions = ctx.sessions.lock().unwrap();
            let token = sessions.new_session();
            sessions.set_code(&token, "ABC234");
            token
        };
        let missing = handle(
            &ctx,
            &test_req(
                "/adminPage/login/addAdmin",
                json!({
                    "name": "alice",
                    "pass": "secret1",
                    "code": "ABC234",
                    "initToken": "wrong"
                }),
                &token,
            ),
        );
        assert_eq!(body_of(&missing)["success"], false);
        assert_eq!(body_of(&missing)["msg"], "initTokenError");

        let ok_reply = handle(
            &ctx,
            &test_req(
                "/adminPage/login/addAdmin",
                json!({
                    "name": "alice",
                    "pass": "secret1",
                    "code": "ABC234",
                    "initToken": "init-secret"
                }),
                &token,
            ),
        );
        assert_eq!(body_of(&ok_reply)["success"], true);
        assert_eq!(ctx.store.lock().unwrap().db.rows("admin").len(), 1);
        assert!(ctx
            .store
            .lock()
            .unwrap()
            .db
            .rows("operateLog")
            .iter()
            .any(|row| { str_of(row, "type") == "addAdmin" }));
    }

    #[test]
    fn replace_refuses_foreign_nginx_conf() {
        let ctx = test_ctx();
        let token = signed_in(&ctx);
        let conf_path = ctx.home.join("foreign.conf");
        std::fs::write(&conf_path, "user www-data;\nhttp { }\n").unwrap();
        {
            let mut store = ctx.store.lock().unwrap();
            store
                .db
                .set_setting("nginxPath", &conf_path.display().to_string());
            store.flush().unwrap();
        }
        let reply = handle(
            &ctx,
            &test_req(
                "/adminPage/conf/replace",
                json!({ "conf": "# Generated by nginxWebUI\nhttp { }\n" }),
                &token,
            ),
        );
        let body = body_of(&reply);
        assert_eq!(body["success"], false);
        assert_eq!(body["msg"], "refuseOverwriteNginxConf");
    }

    #[test]
    fn run_cmd_rejects_free_form_strings() {
        let ctx = test_ctx();
        let token = signed_in(&ctx);
        let reply = handle(
            &ctx,
            &test_req(
                "/adminPage/conf/runCmd",
                json!({ "cmd": "curl http://example" }),
                &token,
            ),
        );
        assert_eq!(body_of(&reply)["msg"], "unsafeCommand");
    }

    #[test]
    fn cannot_delete_current_admin() {
        let ctx = test_ctx();
        let token = signed_in(&ctx);
        {
            let mut store = ctx.store.lock().unwrap();
            store
                .db
                .upsert("admin", json!({ "id": "1", "name": "tester", "pass": "x" }));
            store
                .db
                .upsert("admin", json!({ "id": "2", "name": "other", "pass": "y" }));
            store.flush().unwrap();
        }
        let reply = handle(
            &ctx,
            &test_req("/adminPage/admin/del", json!({ "id": "1" }), &token),
        );
        assert_eq!(body_of(&reply)["msg"], "cannotDeleteSelf");
        assert_eq!(ctx.store.lock().unwrap().db.rows("admin").len(), 2);

        let reply = handle(
            &ctx,
            &test_req("/adminPage/admin/del", json!({ "id": "2" }), &token),
        );
        assert_eq!(body_of(&reply)["success"], true);
        assert_eq!(ctx.store.lock().unwrap().db.rows("admin").len(), 1);
    }

    #[test]
    fn admin_save_without_password_keeps_existing_hash() {
        let ctx = test_ctx();
        let token = signed_in(&ctx);
        {
            let mut store = ctx.store.lock().unwrap();
            store.db.upsert(
                "admin",
                json!({ "id": "9", "name": "keep", "pass": "hashed-old" }),
            );
            store.flush().unwrap();
        }
        let reply = handle(
            &ctx,
            &test_req(
                "/adminPage/admin/save",
                json!({ "id": "9", "name": "keep-renamed" }),
                &token,
            ),
        );
        assert_eq!(body_of(&reply)["success"], true);
        let row = ctx.store.lock().unwrap().db.get("admin", "9").unwrap();
        assert_eq!(row["name"], "keep-renamed");
        assert_eq!(row["pass"], "hashed-old");
    }

    #[test]
    fn cron_failure_writes_operate_log() {
        let ctx = test_ctx();
        {
            let mut store = ctx.store.lock().unwrap();
            record_system_operate(&mut store, "acme.cron", "", false, "acme failed");
            store.flush().unwrap();
            assert!(store.db.rows("operateLog").iter().any(|row| {
                str_of(row, "type") == "acme.cron" && str_of(row, "success") == "false"
            }));
        }
    }
}
