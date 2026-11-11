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

struct Options {
    port: u16,
    bind: String,
    home: PathBuf,
    web: Option<PathBuf>,
    threads: usize,
    reload_cmd: Option<String>,
    secure_cookie: bool,
    config_path: Option<PathBuf>,
    trusted_proxies: Vec<crate::net::IpNet>,
    agent_threads: usize,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
}

const DEFAULT_AGENT_THREADS: usize = 32;
/// Body limits. Login is tiny; agent endpoints are public (no session), so
/// they get a small cap; result uploads carry RPC output (log tails, site
/// lists) and get a larger but still bounded one.
const LOGIN_BODY_LIMIT: u64 = 16 * 1024;
const AGENT_BODY_LIMIT: u64 = 64 * 1024;
const AGENT_RESULT_LIMIT: u64 = 4 * 1024 * 1024;
const ADMIN_BODY_LIMIT: u64 = 16 * 1024 * 1024;

fn body_limit(path: &str) -> u64 {
    if path.starts_with("/adminPage/login") {
        LOGIN_BODY_LIMIT
    } else if path == "/agent/v1/result" {
        AGENT_RESULT_LIMIT
    } else if path.starts_with("/agent/") {
        AGENT_BODY_LIMIT
    } else {
        ADMIN_BODY_LIMIT
    }
}

fn parse_proxies_or_exit(text: &str) -> Vec<crate::net::IpNet> {
    match crate::net::parse_list(text) {
        Ok(list) => list,
        Err(err) => {
            eprintln!("trusted_proxies: {err}");
            std::process::exit(1);
        }
    }
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
        println!("  --agent-threads=N    agent 长轮询线程池（默认 32，即最多 32 台设备同时在线）");
        println!(
            "  --trusted-proxies=L  受信反代地址/网段（逗号分隔），仅这些来源的 X-Forwarded-* 生效"
        );
        println!("  --tls-cert=FILE      面板 HTTPS 证书（PEM）；与 --tls-key 同时设置");
        println!("  --tls-key=FILE       面板 HTTPS 私钥（PEM）");
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
        trusted_proxies: Vec::new(),
        agent_threads: DEFAULT_AGENT_THREADS,
        tls_cert: None,
        tls_key: None,
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
                if let Some(list) = &file.trusted_proxies {
                    options.trusted_proxies = parse_proxies_or_exit(list);
                }
                if let Some(n) = file.agent_threads {
                    options.agent_threads = n.clamp(1, 1024);
                }
                if file.tls_cert.is_some() {
                    options.tls_cert = file.tls_cert;
                }
                if file.tls_key.is_some() {
                    options.tls_key = file.tls_key;
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

    if let Ok(list) = std::env::var("NGINXWEBUI_TRUSTED_PROXIES") {
        if !list.trim().is_empty() {
            options.trusted_proxies = parse_proxies_or_exit(&list);
        }
    }
    if let Some(n) = std::env::var("NGINXWEBUI_AGENT_THREADS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
    {
        options.agent_threads = n.clamp(1, 1024);
    }
    if let Some(path) = env_path("NGINXWEBUI_TLS_CERT") {
        options.tls_cert = Some(path);
    }
    if let Some(path) = env_path("NGINXWEBUI_TLS_KEY") {
        options.tls_key = Some(path);
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
            "--trusted-proxies" => {
                if let Some(value) = take(inline) {
                    options.trusted_proxies = parse_proxies_or_exit(&value);
                }
            }
            "--agent-threads" => {
                if let Some(value) = take(inline) {
                    options.agent_threads = value
                        .trim()
                        .parse()
                        .unwrap_or(DEFAULT_AGENT_THREADS)
                        .clamp(1, 1024);
                }
            }
            "--tls-cert" => {
                if let Some(value) = take(inline) {
                    if !value.trim().is_empty() {
                        options.tls_cert = Some(PathBuf::from(value.trim()));
                    }
                }
            }
            "--tls-key" => {
                if let Some(value) = take(inline) {
                    if !value.trim().is_empty() {
                        options.tls_key = Some(PathBuf::from(value.trim()));
                    }
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
    let path_preview = url_preview
        .split('?')
        .next()
        .unwrap_or(&url_preview)
        .to_string();
    let max_body = body_limit(&path_preview);
    let mut body = Vec::new();
    {
        let reader = request.as_reader();
        let _ = reader.take(max_body + 1).read_to_end(&mut body);
    }
    if body.len() as u64 > max_body {
        let mut reply = crate::api::json_reply(
            413,
            &serde_json::json!({ "success": false, "msg": "bodyTooLarge" }),
        );
        reply
            .extra_headers
            .push(("Connection".into(), "close".into()));
        respond(request, reply);
        return;
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
    let mut requested_with = String::new();
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
        if header.field.equiv("X-Requested-With") {
            requested_with = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("Authorization") {
            authorization = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("Host") {
            host = header.value.as_str().trim().to_string();
        }
        if header.field.equiv("X-Forwarded-For") {
            if !forwarded_ip.is_empty() {
                forwarded_ip.push(',');
            }
            forwarded_ip.push_str(header.value.as_str());
        }
    }
    // X-Forwarded-* only count when the TCP peer is a configured proxy
    let peer = request.remote_addr().map(|addr| addr.ip());
    let behind_trusted_proxy = crate::net::is_trusted(peer, &app.trusted_proxies);
    let forwarded_https = forwarded_https && behind_trusted_proxy;
    let remote_ip = crate::net::client_ip(peer, &forwarded_ip, &app.trusted_proxies);

    if path == "/healthz" || path == "/readyz" {
        let reply = crate::api::ok(serde_json::json!({
            "ok": true,
            "version": VERSION,
        }));
        respond(request, reply);
        return;
    }
    if let Some(rest) = path.strip_prefix("/healthz/group/") {
        let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        let reply = match parts.as_slice() {
            [group_id] => crate::groups::healthz(app, group_id, "local"),
            [group_id, "node", node_id] => crate::groups::healthz(app, group_id, node_id),
            _ => {
                crate::api::json_reply(404, &serde_json::json!({ "ok": false, "msg": "not found" }))
            }
        };
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
        requested_with,
    };

    // unauthenticated agent endpoints: per-IP fixed windows
    let bucket = match path.as_str() {
        "/agent/install.sh" | "/agent/install.ps1" => Some(("install", 30u32)),
        "/agent/download" => Some(("download", 10)),
        "/agent/v1/join" => Some(("join", 10)),
        _ => None,
    };
    if let Some((name, max)) = bucket {
        let key = format!("{name}:{}", req.remote_ip);
        if app
            .limits
            .lock()
            .unwrap()
            .allow_bucket(&key, 60, max)
            .is_err()
        {
            let reply = crate::api::json_reply(
                429,
                &serde_json::json!({ "success": false, "msg": "tooManyAttempts" }),
            );
            respond(request, reply);
            return;
        }
    }

    if path == "/agent/install.sh" {
        let reply = registry::install_script(&req);
        respond(request, reply);
        return;
    }
    if path == "/agent/install.ps1" {
        let reply = registry::install_script_ps1(&req);
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

fn load_ssl_config(options: &Options) -> Option<tiny_http::SslConfig> {
    match (&options.tls_cert, &options.tls_key) {
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            eprintln!("tls_cert and tls_key must be set together (config, NGINXWEBUI_TLS_CERT / NGINXWEBUI_TLS_KEY, or --tls-cert / --tls-key)");
            std::process::exit(1);
        }
        (Some(cert), Some(key)) => {
            let certificate = std::fs::read(cert).unwrap_or_else(|err| {
                eprintln!("cannot read tls_cert {}: {err}", cert.display());
                std::process::exit(1);
            });
            let private_key = std::fs::read(key).unwrap_or_else(|err| {
                eprintln!("cannot read tls_key {}: {err}", key.display());
                std::process::exit(1);
            });
            if certificate.is_empty() || private_key.is_empty() {
                eprintln!("tls_cert / tls_key files are empty");
                std::process::exit(1);
            }
            Some(tiny_http::SslConfig {
                certificate,
                private_key,
            })
        }
    }
}

fn bind_listener(
    address: &str,
    tls: Option<tiny_http::SslConfig>,
) -> Result<tiny_http::Server, String> {
    let result = if let Some(config) = tls {
        tiny_http::Server::https(address, config)
    } else {
        tiny_http::Server::http(address)
    };
    result.map_err(|err| err.to_string())
}

pub fn run() {
    let mut options = parse_args();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let home = options.home.clone();
    let data_dir = home.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(home.join("logs"));
    crate::applog::init(&home, None);
    if !crate::seed::has_system_nginx() {
        let _ = crate::seed::write_default_files(&home);
    }

    let mut store = Store::open(&data_dir);
    crate::seed::seed(&mut store.db, &home, options.reload_cmd.as_deref());
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
    let tls = load_ssl_config(&options);
    if tls.is_some() {
        options.secure_cookie = true;
    }
    let scheme = if tls.is_some() { "https" } else { "http" };
    println!("  address   : {scheme}://{}:{}", options.bind, options.port);
    if options.bind == "0.0.0.0" || options.bind == "::" {
        if tls.is_some() {
            println!(
                "  warning   : listening on all interfaces with built-in TLS; a reverse proxy is still the usual production choice"
            );
        } else {
            println!("  warning   : listening on all interfaces; do not expose this port on the public internet without HTTPS");
        }
    }
    println!("  sqlite    : {}", store.db_path.display());
    println!(
        "  memory    : no JVM, local sqlite, {} worker threads, {} agent poll threads",
        options.threads, options.agent_threads
    );
    if options.trusted_proxies.is_empty() {
        println!("  proxies   : none trusted (X-Forwarded-For / -Proto ignored)");
    } else {
        println!(
            "  proxies   : trusting X-Forwarded-* from {} address range(s)",
            options.trusted_proxies.len()
        );
    }

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

    let mut ctx = Ctx::standalone(home.clone(), store);
    ctx.web_dir = web_dir;
    ctx.exe_dir = exe_dir;
    ctx.init_token = Mutex::new(init_token);
    ctx.secure_cookie = options.secure_cookie;
    ctx.agent_pool = registry::AgentPool::new(options.agent_threads);
    ctx.trusted_proxies = options.trusted_proxies.clone();
    let app = Arc::new(ctx);

    crate::api::spawn_acme_cron(app.clone());
    crate::groups::spawn_failover(app.clone());

    let address = format!("{}:{}", options.bind, options.port);
    let server = match bind_listener(&address, tls) {
        Ok(server) => Arc::new(server),
        Err(err) => {
            crate::applog::error(format!("cannot listen on {address}: {err}"));
            eprintln!("cannot listen on {address}: {err}");
            std::process::exit(1);
        }
    };

    let mut handles = Vec::new();
    for _ in 0..options.threads {
        let server = server.clone();
        let app = app.clone();
        handles.push(std::thread::spawn(move || {
            while let Ok(request) = server.recv() {
                handle_request(&app, request);
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
