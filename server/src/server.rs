//! nginxWebUI — Rust backend.
//!
//! One small process serves the JSON API and the static web pages that the
//! SvelteKit build produced. No Java virtual machine, no database server, no
//! background workers: a handful of threads and a few megabytes of memory.

use crate::api::{Ctx, Reply, Req};
use crate::store::Store;
use crate::{registry, VERSION};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Response, StatusCode};

// The default nginx files, taken from the same conf.zip the Java version ships.
const MIME_TYPES: &str = include_str!("../assets/conf/mime.types");
const FASTCGI_CONF: &str = include_str!("../assets/conf/fastcgi.conf");
const FASTCGI_PARAMS: &str = include_str!("../assets/conf/fastcgi_params");
const SCGI_PARAMS: &str = include_str!("../assets/conf/scgi_params");
const UWSGI_PARAMS: &str = include_str!("../assets/conf/uwsgi_params");
const KOI_UTF: &str = include_str!("../assets/conf/koi-utf");
const KOI_WIN: &str = include_str!("../assets/conf/koi-win");
const WIN_UTF: &str = include_str!("../assets/conf/win-utf");

const NGINX_CONF_TEMPLATE: &str =
    "worker_processes  1;\n\nevents {\n    worker_connections  1024;\n}\n";

struct Options {
    port: u16,
    bind: String,
    home: PathBuf,
    web: Option<PathBuf>,
    threads: usize,
    reload_cmd: Option<String>,
    secure_cookie: bool,
    config_path: Option<PathBuf>,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn parse_args() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("nginxWebUI (Rust backend)");
        println!("  --config=FILE        配置文件（默认寻找 nginxwebui.conf）");
        println!("  --server.port=8080   监听端口");
        println!("  --bind=127.0.0.1     监听地址（默认本机；Docker 需 0.0.0.0）");
        println!("  --project.home=DIR   数据目录");
        println!("  --web.dir=DIR        静态前端目录");
        println!("  --threads=N          工作线程（默认 2）");
        println!();
        println!("优先顺序：命令行 > 环境变量 > 配置文件 > 默认值");
        std::process::exit(0);
    }

    let mut options = Options {
        port: 8080,
        bind: "127.0.0.1".into(),
        home: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        web: None,
        threads: 2,
        reload_cmd: None,
        secure_cookie: false,
        config_path: None,
    };

    let cli_config = crate::config::cli_config_path(&args);
    if let Some(path) = crate::config::find(cli_config.as_deref(), &[]) {
        match crate::config::load(&path) {
            Ok(file) => {
                if let Some(port) = file.port {
                    options.port = port;
                }
                if let Some(home) = file.home {
                    options.home = home;
                }
                if let Some(web) = file.web {
                    options.web = Some(web);
                }
                if let Some(threads) = file.threads {
                    options.threads = threads.clamp(1, 64);
                }
                options.reload_cmd = file.reload_cmd;
                if let Some(bind) = file.bind {
                    if !bind.trim().is_empty() {
                        options.bind = bind.trim().to_string();
                    }
                }
                if let Some(secure) = file.secure_cookie {
                    options.secure_cookie = secure;
                }
                options.config_path = Some(path);
            }
            Err(err) => {
                eprintln!("配置文件无效: {err}");
                std::process::exit(1);
            }
        }
    }

    if let Some(port) = std::env::var("NGINXWEBUI_PORT")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
    {
        options.port = port;
    }
    if let Some(home) = env_path("NGINXWEBUI_HOME") {
        options.home = home;
    }
    if let Some(web) = env_path("NGINXWEBUI_WEB_DIR") {
        options.web = Some(web);
    }
    if let Some(threads) = std::env::var("NGINXWEBUI_THREADS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
    {
        options.threads = threads.clamp(1, 64);
    }
    if let Ok(cmd) = std::env::var("NGINXWEBUI_RELOAD_CMD") {
        if !cmd.trim().is_empty() {
            options.reload_cmd = Some(cmd);
        }
    }
    if let Ok(bind) = std::env::var("NGINXWEBUI_BIND") {
        if !bind.trim().is_empty() {
            options.bind = bind.trim().to_string();
        }
    }
    if let Some(secure) = std::env::var("NGINXWEBUI_SECURE_COOKIE")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
    {
        options.secure_cookie = matches!(secure.as_str(), "1" | "true" | "yes" | "on");
    }

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let mut take = |default: Option<String>| -> Option<String> {
            if let Some(value) = default {
                return Some(value);
            }
            index += 1;
            args.get(index).cloned()
        };
        match key.as_str() {
            "--config" | "-c" => {
                let _ = take(inline);
            }
            "--server.port" | "--port" | "-p" => {
                if let Some(value) = take(inline) {
                    options.port = value.trim().parse().unwrap_or(8080);
                }
            }
            "--bind" | "--listen" => {
                if let Some(value) = take(inline) {
                    if !value.trim().is_empty() {
                        options.bind = value.trim().to_string();
                    }
                }
            }
            "--secure-cookie" => {
                options.secure_cookie = match take(inline) {
                    Some(value) => matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on" | ""
                    ),
                    None => true,
                };
            }
            "--project.home" | "--home" => {
                if let Some(value) = take(inline) {
                    options.home = PathBuf::from(value);
                }
            }
            "--web.dir" => {
                if let Some(value) = take(inline) {
                    options.web = Some(PathBuf::from(value));
                }
            }
            "--threads" => {
                if let Some(value) = take(inline) {
                    options.threads = value.trim().parse().unwrap_or(2).clamp(1, 64);
                }
            }
            _ => {}
        }
        index += 1;
    }
    options
}

/// Write the small standard nginx files the first time, like the Java version did
/// when it unpacked conf.zip.
fn write_default_files(home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let files: [(&str, &str); 8] = [
        ("mime.types", MIME_TYPES),
        ("fastcgi.conf", FASTCGI_CONF),
        ("fastcgi_params", FASTCGI_PARAMS),
        ("scgi_params", SCGI_PARAMS),
        ("uwsgi_params", UWSGI_PARAMS),
        ("koi-utf", KOI_UTF),
        ("koi-win", KOI_WIN),
        ("win-utf", WIN_UTF),
    ];
    for (name, content) in files {
        let path = home.join(name);
        if !path.exists() {
            std::fs::write(path, content)?;
        }
    }
    let conf_path = home.join("nginx.conf");
    if !conf_path.exists() {
        std::fs::write(conf_path, NGINX_CONF_TEMPLATE)?;
    }
    Ok(())
}

fn seed(db: &mut crate::store::Db, home: &Path, reload_cmd: Option<&str>) {
    db.set_setting("home", &home.display().to_string());
    if db.setting("lang").is_empty() {
        db.set_setting("lang", "zh");
    }
    let system_nginx = crate::nginx::unix_is_file("/etc/nginx/nginx.conf");
    if db.setting("nginxPath").is_empty() {
        if system_nginx {
            db.set_setting("nginxPath", "/etc/nginx/nginx.conf");
        } else {
            db.set_setting("nginxPath", &home.join("nginx.conf").display().to_string());
        }
    }
    if db.setting("nginxDir").is_empty() {
        if system_nginx {
            db.set_setting("nginxDir", "/etc/nginx");
        } else {
            db.set_setting("nginxDir", &home.display().to_string());
        }
    }
    let fallback = |name: &str| home.join(name).display().to_string();
    if db.setting("sitesAvailable").is_empty() {
        let value = if crate::nginx::unix_is_dir("/etc/nginx/sites-available") {
            "/etc/nginx/sites-available".to_string()
        } else {
            fallback("sites-available")
        };
        db.set_setting("sitesAvailable", &value);
    }
    if db.setting("sitesEnabled").is_empty() {
        let value = if crate::nginx::unix_is_dir("/etc/nginx/sites-enabled") {
            "/etc/nginx/sites-enabled".to_string()
        } else {
            fallback("sites-enabled")
        };
        db.set_setting("sitesEnabled", &value);
    }
    if db.setting("sslDir").is_empty() {
        let value = if crate::nginx::unix_is_dir("/etc/nginx/ssl") {
            "/etc/nginx/ssl".to_string()
        } else {
            fallback("ssl")
        };
        db.set_setting("sslDir", &value);
    }
    if db.setting("sslParams").is_empty() {
        let value = if crate::nginx::unix_is_file("/etc/nginx/ssl-params.conf")
            || crate::nginx::unix_is_dir("/etc/nginx")
        {
            "/etc/nginx/ssl-params.conf".to_string()
        } else {
            fallback("ssl-params.conf")
        };
        db.set_setting("sslParams", &value);
    }
    if db.setting("webRoot").is_empty() {
        let value = if crate::nginx::unix_is_dir("/var/www") {
            "/var/www".to_string()
        } else {
            fallback("www")
        };
        db.set_setting("webRoot", &value);
    }
    if db.setting("reloadCmd").is_empty() {
        let cmd = reload_cmd
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if Path::new("/.dockerenv").exists() {
                    "nginx -s reload".into()
                } else {
                    "sudo -n systemctl reload nginx".into()
                }
            });
        db.set_setting("reloadCmd", &cmd);
    } else if db.setting("reloadCmd") == "sudo systemctl reload nginx" {
        db.set_setting("reloadCmd", "sudo -n systemctl reload nginx");
    }
    if db.setting("decompose").is_empty() {
        db.set_setting("decompose", "false");
    }
    if db.setting("denyAllow").is_empty() {
        db.set_setting("denyAllow", "0");
    }
    if db.setting("denyAllowStream").is_empty() {
        db.set_setting("denyAllowStream", "0");
    }
    if db.setting("acmeSh").is_empty() {
        let found = crate::acme::resolve_bin(db);
        if found.exists() {
            db.set_setting("acmeSh", &found.display().to_string());
        }
    }
    if db.setting("autoInstall").is_empty() {
        db.set_setting("autoInstall", "false");
    }

    if db.rows("basic").is_empty() {
        db.upsert(
            "basic",
            serde_json::json!({ "name": "worker_processes", "value": "auto", "seq": 1 }),
        );
        db.upsert(
            "basic",
            serde_json::json!({
                "name": "events",
                "value": "{\n    worker_connections  1024;\n    accept_mutex on;\n}",
                "seq": 2
            }),
        );
    }
    if db.rows("http").is_empty() {
        db.upsert(
            "http",
            serde_json::json!({ "name": "include", "value": "mime.types", "enable": true, "seq": 0 }),
        );
        db.upsert(
            "http",
            serde_json::json!({
                "name": "default_type",
                "value": "application/octet-stream",
                "enable": true,
                "seq": 1
            }),
        );
    }
}

fn find_web_dir(options: &Options, exe_dir: &Path) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(web) = &options.web {
        candidates.push(web.clone());
    }
    if let Ok(env) = std::env::var("NGINXWEBUI_WEB_DIR") {
        candidates.push(PathBuf::from(env));
    }
    candidates.push(exe_dir.join("web"));
    candidates.push(exe_dir.join("web/build"));
    candidates.push(exe_dir.join("../web/build"));
    candidates.push(exe_dir.join("../../web/build"));
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("web/build"));
        candidates.push(cwd.join("web"));
    }
    for candidate in &candidates {
        if candidate.join("index.html").is_file() {
            return candidate.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("web/build"))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Map a URL to a prerendered file: `/server/` → `server/index.html`.
fn resolve_static(web_dir: &Path, url: &str) -> Option<PathBuf> {
    let path_only = url.split('?').next().unwrap_or("/");
    let relative = path_only
        .trim_start_matches('/')
        .trim_end_matches('/')
        .replace('\\', "/");
    if relative.contains("..") {
        return None;
    }
    if relative.is_empty() {
        let index = web_dir.join("index.html");
        return index.is_file().then_some(index);
    }
    let direct = web_dir.join(&relative);
    if direct.is_file() {
        return Some(direct);
    }
    let as_dir = direct.join("index.html");
    if as_dir.is_file() {
        return Some(as_dir);
    }
    if !relative.contains('.') {
        let as_html = web_dir.join(format!("{relative}.html"));
        if as_html.is_file() {
            return Some(as_html);
        }
    }
    None
}

/// Serve a prerendered page from the built web folder.
fn serve_static(web_dir: &Path, url: &str) -> Reply {
    let Some(file_path) = resolve_static(web_dir, url) else {
        return Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: format!(
                "Not found: {}\nThe web pages were not found in {}.\nBuild them with: npm run build",
                url,
                web_dir.display()
            )
            .into_bytes(),
            set_cookie: None,
            extra_headers: Vec::new(),
        };
    };
    match std::fs::read(&file_path) {
        Ok(bytes) => {
            let name = file_path.to_string_lossy().replace('\\', "/");
            let cache = if name.contains("/_app/immutable/") {
                "public, max-age=31536000, immutable"
            } else if name.ends_with(".html") {
                "no-cache"
            } else {
                "public, max-age=3600"
            };
            Reply {
                status: 200,
                content_type: content_type(&file_path).to_string(),
                body: bytes,
                set_cookie: None,
                extra_headers: vec![("Cache-Control".to_string(), cache.to_string())],
            }
        }
        Err(err) => Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: format!(
                "Not found: {}\n{}\nThe web pages were not found in {}.\nBuild them with: npm run build",
                url,
                err,
                web_dir.display()
            )
            .into_bytes(),
            set_cookie: None,
            extra_headers: Vec::new(),
        },
    }
}

pub(crate) fn respond(request: tiny_http::Request, reply: Reply) {
    let mut response = Response::from_data(reply.body).with_status_code(StatusCode(reply.status));
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], reply.content_type.as_bytes()) {
        response = response.with_header(header);
    }
    if let Some(cookie) = reply.set_cookie {
        if let Ok(header) = Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()) {
            response = response.with_header(header);
        }
    }
    for (name, value) in reply.extra_headers {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response = response.with_header(header);
        }
    }
    for (name, value) in [
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        (
            "Content-Security-Policy",
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'self'",
        ),
        ("Referrer-Policy", "no-referrer"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response = response.with_header(header);
        }
    }
    let _ = request.respond(response);
}

fn handle_request(app: &Arc<Ctx>, mut request: tiny_http::Request) {
    let url = request.url().to_string();
    let method = format!("{}", request.method());

    let url_preview = request.url().to_string();
    let path_preview = url_preview.split('?').next().unwrap_or(&url_preview);
    let max_body = if path_preview.starts_with("/adminPage/login") {
        16 * 1024u64
    } else {
        16 * 1024 * 1024
    };
    let mut body = Vec::new();
    {
        let reader = request.as_reader();
        let _ = reader.take(max_body).read_to_end(&mut body);
    }

    let (path, query_text) = match url.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (url.clone(), String::new()),
    };

    let mut query: HashMap<String, String> = HashMap::new();
    for pair in query_text.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => (pair.to_string(), String::new()),
        };
        query.insert(percent_decode(&key), percent_decode(&value));
    }

    let mut cookies: HashMap<String, String> = HashMap::new();
    let mut content_type = String::new();
    let mut forwarded_https = false;
    let mut forwarded_ip = String::new();
    let mut node_header = String::new();
    let mut authorization = String::new();
    let mut host = String::new();
    for header in request.headers() {
        if header.field.equiv("Cookie") {
            for part in header.value.as_str().split(';') {
                if let Some((key, value)) = part.trim().split_once('=') {
                    cookies.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        if header.field.equiv("Content-Type") {
            content_type = header.value.as_str().to_string();
        }
        if header.field.equiv("X-Forwarded-Proto") {
            forwarded_https = header
                .value
                .as_str()
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("https");
        }
        if header.field.equiv("X-NginxWebUI-Node") {
            node_header = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("Authorization") {
            authorization = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("Host") {
            host = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("X-Forwarded-For") {
            forwarded_ip = header
                .value
                .as_str()
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
        }
    }
    let remote_ip = if !forwarded_ip.is_empty() {
        forwarded_ip
    } else {
        request
            .remote_addr()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "unknown".into())
    };

    if path == "/healthz" || path == "/readyz" {
        let reply = crate::api::ok(serde_json::json!({
            "ok": true,
            "version": VERSION,
        }));
        respond(request, reply);
        return;
    }

    let proto = if app.secure_cookie || forwarded_https {
        "https"
    } else {
        "http"
    };
    let panel_base = if !host.is_empty() {
        format!("{proto}://{host}")
    } else {
        String::new()
    };

    let req = Req {
        method,
        path: path.clone(),
        query,
        body,
        cookies,
        content_type,
        remote_ip,
        secure_cookie: app.secure_cookie || forwarded_https,
        node: node_header,
        authorization,
        panel_base,
    };

    if path == "/agent/install.sh" {
        let reply = registry::install_script(&req);
        respond(request, reply);
        return;
    }
    if path == "/agent/download" {
        let reply = registry::download_agent(app, &req);
        respond(request, reply);
        return;
    }
    if path.starts_with("/agent/v1/") {
        if path == "/agent/v1/poll" {
            match app.agent_pool.submit(registry::PollTask {
                request,
                app: app.clone(),
                req,
            }) {
                Ok(()) => {}
                Err(task) => {
                    respond(task.request, crate::api::fail("tooManyAgents"));
                }
            }
            return;
        }
        let reply = registry::handle_agent(app, &req);
        respond(request, reply);
        return;
    }

    let is_api = path.starts_with("/adminPage/");
    let reply = if is_api {
        crate::api::handle(app, &req)
    } else {
        serve_static(&app.web_dir, &url)
    };
    respond(request, reply);
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = &input[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn run() {
    let options = parse_args();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let home = options.home.clone();
    let data_dir = home.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(home.join("logs"));
    crate::applog::init(&home, None);
    if !crate::nginx::unix_is_file("/etc/nginx/nginx.conf") {
        let _ = write_default_files(&home);
    }

    let mut store = Store::open(&data_dir);
    seed(&mut store.db, &home, options.reload_cmd.as_deref());
    let _ = crate::compat::adopt(&mut store.db);
    match crate::site::ensure_main_includes_sites(&store.db) {
        Ok(crate::site::SitesIncludeOutcome::NeedsConfirm { include_line, .. }) => {
            crate::applog::warn(format!(
                "nginx.conf has no active http block (only a commented one); not inserting `{include_line}` automatically"
            ));
        }
        Err(err) => crate::applog::warn(format!("ensure sites-enabled include: {err}")),
        _ => {}
    }
    let _ = store.flush();

    let web_dir = find_web_dir(&options, &exe_dir);

    crate::applog::info(format!("nginxWebUI {VERSION} starting"));
    println!("nginxWebUI {} (Rust backend)", VERSION);
    if let Some(path) = &options.config_path {
        println!("  config    : {}", path.display());
    }
    println!("  home      : {}", home.display());
    println!("  web pages : {}", web_dir.display());
    println!("  address   : http://{}:{}", options.bind, options.port);
    if options.bind == "0.0.0.0" || options.bind == "::" {
        println!("  warning   : listening on all interfaces; do not expose this port on the public internet without HTTPS");
    }
    println!("  sqlite    : {}", store.db_path.display());
    println!(
        "  memory    : no JVM, local sqlite, {} worker threads",
        options.threads
    );

    let init_token = if store.db.rows("admin").is_empty() {
        let token = std::env::var("NGINXWEBUI_INIT_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::auth::random_token(32));
        println!("============================================================");
        println!("  First-run init token: {token}");
        println!("  Enter it when creating the first administrator.");
        println!("  You can also set NGINXWEBUI_INIT_TOKEN.");
        println!("============================================================");
        Some(token)
    } else {
        None
    };

    let app = Arc::new(Ctx {
        home: home.clone(),
        web_dir,
        store: Mutex::new(store),
        sessions: Mutex::new(crate::auth::Sessions::default()),
        limits: Mutex::new(crate::auth::RateLimiter::default()),
        init_token: Mutex::new(init_token),
        secure_cookie: options.secure_cookie,
        hub: registry::Hub::new(),
        agent_pool: registry::AgentPool::new(32),
        exe_dir,
    });

    {
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2 * 60 * 60));
            let snapshot = {
                let store = app.store.lock().unwrap();
                store.db.clone()
            };
            if crate::acme::resolve_bin(&snapshot).as_os_str().is_empty() {
                continue;
            }
            if crate::acme::try_begin("cron", "").is_err() {
                continue;
            }
            let (ok, out) = crate::acme::cron(&snapshot);
            crate::acme::job_finish(ok, &out);
            if !ok {
                let mut store = app.store.lock().unwrap();
                crate::api::record_system_operate(
                    &mut store,
                    "acme.cron",
                    "",
                    false,
                    if out.trim().is_empty() {
                        "acme.sh --cron failed"
                    } else {
                        &out
                    },
                );
                let _ = store.flush();
            }
            if !out.trim().is_empty() {
                crate::applog::info(format!(
                    "acme.sh --cron (ok={}): {}",
                    ok,
                    out.chars().take(400).collect::<String>()
                ));
            }
        });
    }

    let address = format!("{}:{}", options.bind, options.port);
    let server = match tiny_http::Server::http(&address) {
        Ok(server) => Arc::new(server),
        Err(err) => {
            crate::applog::error(format!("cannot listen on {address}: {err}"));
            eprintln!("cannot listen on {}: {}", address, err);
            std::process::exit(1);
        }
    };

    let mut handles = Vec::new();
    for _ in 0..options.threads {
        let server = server.clone();
        let app = app.clone();
        handles.push(std::thread::spawn(move || loop {
            match server.recv() {
                Ok(request) => handle_request(&app, request),
                Err(_) => break,
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_directory_index_pages() {
        let dir = std::env::temp_dir().join("nginxwebui-test-static-pages");
        let server = dir.join("server");
        let edit = server.join("edit");
        let _ = fs::create_dir_all(&edit);
        fs::write(dir.join("index.html"), "home").unwrap();
        fs::write(server.join("index.html"), "list").unwrap();
        fs::write(edit.join("index.html"), "edit").unwrap();
        fs::write(dir.join("app.js"), "js").unwrap();

        assert!(resolve_static(&dir, "/").unwrap().ends_with("index.html"));
        assert!(
            resolve_static(&dir, "/server")
                .unwrap()
                .ends_with("server\\index.html")
                || resolve_static(&dir, "/server")
                    .unwrap()
                    .ends_with("server/index.html")
        );
        assert!(resolve_static(&dir, "/server/edit?id=abc")
            .unwrap()
            .display()
            .to_string()
            .contains("edit"));
        assert!(resolve_static(&dir, "/app.js").unwrap().ends_with("app.js"));
        assert!(resolve_static(&dir, "/missing").is_none());
        assert!(resolve_static(&dir, "/../secret").is_none());
    }
}
