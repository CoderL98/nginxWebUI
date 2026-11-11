//! Per-site files under sites-available / sites-enabled, matching the host
//! scripts in `script/add-static-site.sh` and `script/add-sub-site.sh`.
//! The process never rewrites the main nginx.conf; it writes one file per
//! domain, tests with `nginx -t`, and reloads.

use crate::nginx;
use crate::store::{bool_of, str_of, Db};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn ssl_dir(db: &Db) -> PathBuf {
    path_or(db, "sslDir", "/etc/nginx/ssl")
}

pub fn sites_available(db: &Db) -> PathBuf {
    path_or(db, "sitesAvailable", "/etc/nginx/sites-available")
}

pub fn sites_enabled(db: &Db) -> PathBuf {
    path_or(db, "sitesEnabled", "/etc/nginx/sites-enabled")
}

pub fn ssl_params(db: &Db) -> PathBuf {
    path_or(db, "sslParams", "/etc/nginx/ssl-params.conf")
}

pub fn ensure_ssl_params(db: &Db) -> Result<(), String> {
    let path = ssl_params(db);
    if path.is_file() {
        return Ok(());
    }
    crate::privfs::write(&path, crate::install::SSL_PARAMS)
}

pub fn web_root_base(db: &Db) -> PathBuf {
    path_or(db, "webRoot", "/var/www")
}

pub fn access_log_dir(db: &Db) -> PathBuf {
    let setting = db.setting("nginxLogDir");
    if !setting.trim().is_empty() {
        return PathBuf::from(setting.trim());
    }
    if cfg!(unix) {
        PathBuf::from("/var/log/nginx")
    } else {
        nginx::nginx_dir(db).join("logs")
    }
}

fn nginx_path_text(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Outcome of ensuring `include …/sites-enabled/*.conf;` is present in nginx.conf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SitesIncludeOutcome {
    AlreadyPresent,
    Written,
    NeedsConfirm {
        include_line: String,
        preview: String,
        reason: String,
    },
}

/// Comment-aware scan: ignore `#` outside quotes.
fn strip_hash_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            i += 1;
            continue;
        }
        if c == b'#' {
            return &line[..i];
        }
        i += 1;
    }
    line
}

/// Byte offset just after the `{` of the first real `http` block (`http {`, `http{`, or `http` then `{` on the next line).
fn find_real_http_block(text: &str) -> Option<usize> {
    let mut code = String::new();
    let mut map: Vec<usize> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let stripped = strip_hash_comment(line);
        for (i, ch) in stripped.char_indices() {
            let start = code.len();
            code.push(ch);
            for _ in start..code.len() {
                map.push(offset + i);
            }
        }
        offset += line.len();
    }
    let lower = code.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"http") {
            let before_ok =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if before_ok {
                let mut j = i + 4;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    return Some(map[j] + 1);
                }
            }
        }
        i += 1;
    }
    None
}

fn text_has_commented_http_block(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            return false;
        }
        let body = trimmed.trim_start_matches('#').to_ascii_lowercase();
        body.contains("http {") || body.contains("http{")
    })
}

/// Active `include` directives whose argument mentions `marker`.
fn has_active_include(text: &str, marker: &str) -> bool {
    if marker.is_empty() {
        return false;
    }
    let marker_norm = marker.replace('\\', "/");
    for line in text.split_inclusive('\n') {
        let code = strip_hash_comment(line);
        let lower = code.to_ascii_lowercase();
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find("include") {
            let at = from + rel;
            let before_ok = at == 0
                || (!code.as_bytes()[at - 1].is_ascii_alphanumeric()
                    && code.as_bytes()[at - 1] != b'_');
            let after = at + "include".len();
            if before_ok && after <= code.len() {
                let next = code[after..].chars().next();
                if next
                    .map(|c| c.is_whitespace() || c == '\'' || c == '"')
                    .unwrap_or(false)
                    && code[after..].replace('\\', "/").contains(&marker_norm)
                {
                    return true;
                }
            }
            from = after;
        }
    }
    false
}

fn sites_include_line(db: &Db) -> String {
    let glob = format!("{}/*.conf", nginx_path_text(&sites_enabled(db)));
    format!("include {};", glob)
}

enum InsertPlan {
    AlreadyPresent,
    Ready(String),
    NeedsConfirm(SitesIncludeOutcome),
}

/// Plan how (or whether) to insert the sites-enabled include into `text`.
pub fn plan_sites_include_text(text: &str, include_line: &str) -> InsertPlanView {
    match plan_insert(text, include_line) {
        InsertPlan::AlreadyPresent => InsertPlanView::AlreadyPresent,
        InsertPlan::Ready(new_text) => InsertPlanView::Ready { new_text },
        InsertPlan::NeedsConfirm(out) => match out {
            SitesIncludeOutcome::NeedsConfirm {
                include_line,
                preview,
                reason,
            } => InsertPlanView::NeedsConfirm {
                include_line,
                preview,
                reason,
            },
            other => InsertPlanView::NeedsConfirm {
                include_line: include_line.to_string(),
                preview: String::new(),
                reason: format!("{:?}", other),
            },
        },
    }
}

/// Public view of [`plan_insert`] used by tests and the confirm API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertPlanView {
    AlreadyPresent,
    Ready {
        new_text: String,
    },
    NeedsConfirm {
        include_line: String,
        preview: String,
        reason: String,
    },
}

fn plan_insert(text: &str, include_line: &str) -> InsertPlan {
    let arg = include_line
        .trim()
        .trim_start_matches("include")
        .trim()
        .trim_end_matches(';')
        .trim();
    let path_marker = if arg.is_empty() { "sites-enabled" } else { arg };
    if has_active_include(text, path_marker) || has_active_include(text, "sites-enabled") {
        return InsertPlan::AlreadyPresent;
    }
    if let Some(at) = find_real_http_block(text) {
        let new_text = format!("{}    {}\n{}", &text[..at], include_line, &text[at..]);
        return InsertPlan::Ready(new_text);
    }
    if text_has_commented_http_block(text) {
        // Preview the whole file that would be written after an explicit confirm:
        // append a real http block rather than inserting into the comment.
        let preview = format!(
            "{}\n\nhttp {{\n    include mime.types;\n    {}\n}}\n",
            text.trim_end(),
            include_line
        );
        return InsertPlan::NeedsConfirm(SitesIncludeOutcome::NeedsConfirm {
            include_line: include_line.to_string(),
            preview,
            reason: "commentedHttpBlock".into(),
        });
    }
    let new_text = format!(
        "{}\n\nhttp {{\n    include mime.types;\n    {}\n}}\n",
        text.trim_end(),
        include_line
    );
    InsertPlan::Ready(new_text)
}

fn inspect_sites_include_file(db: &Db) -> Result<InsertPlan, String> {
    let conf_path = nginx::nginx_path(db);
    if !conf_path.exists() {
        return Ok(InsertPlan::AlreadyPresent);
    }
    let text = crate::privfs::read_to_string(&conf_path).unwrap_or_default();
    Ok(plan_insert(&text, &sites_include_line(db)))
}

/// If the main nginx.conf has no sites-enabled include, add one so apply() works
/// on a home-prefix install. Debian's stock file already includes it.
///
/// Insertion is comment-aware: a `# http {` in a comment is not treated as the
/// real http block. When the only match is commented out, the file is left
/// unchanged and [`SitesIncludeOutcome::NeedsConfirm`] is returned.
pub fn ensure_main_includes_sites(db: &Db) -> Result<SitesIncludeOutcome, String> {
    apply_sites_include(db, false)
}

/// Apply a planned include. When `confirm` is true, a previously ambiguous
/// insert (commented-only `http {`) writes the previewed new file.
pub fn apply_sites_include(db: &Db, confirm: bool) -> Result<SitesIncludeOutcome, String> {
    let conf_path = nginx::nginx_path(db);
    match inspect_sites_include_file(db)? {
        InsertPlan::AlreadyPresent => Ok(SitesIncludeOutcome::AlreadyPresent),
        InsertPlan::Ready(new_text) => {
            crate::privfs::write(&conf_path, new_text)?;
            Ok(SitesIncludeOutcome::Written)
        }
        InsertPlan::NeedsConfirm(out) => {
            if confirm {
                if let SitesIncludeOutcome::NeedsConfirm { preview, .. } = &out {
                    crate::privfs::write(&conf_path, preview)?;
                    return Ok(SitesIncludeOutcome::Written);
                }
            }
            Ok(out)
        }
    }
}

/// Inspect the main nginx.conf without writing it.
pub fn peek_sites_include(db: &Db) -> Result<InsertPlanView, String> {
    let conf_path = nginx::nginx_path(db);
    if !conf_path.exists() {
        return Ok(InsertPlanView::AlreadyPresent);
    }
    let text = crate::privfs::read_to_string(&conf_path).unwrap_or_default();
    Ok(plan_sites_include_text(&text, &sites_include_line(db)))
}

fn path_or(db: &Db, key: &str, fallback: &str) -> PathBuf {
    let setting = db.setting(key);
    if setting.trim().is_empty() {
        PathBuf::from(fallback)
    } else {
        PathBuf::from(setting.trim())
    }
}

pub fn safe_domain(raw: &str) -> Option<String> {
    let d = raw.trim().to_lowercase();
    if d.is_empty() || d.contains("..") || d.contains('/') || d.contains('\\') {
        return None;
    }
    let ok = d
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '*');
    if !ok {
        return None;
    }
    Some(d)
}

pub fn cert_paths(db: &Db, name: &str) -> (PathBuf, PathBuf) {
    let dir = ssl_dir(db);
    (
        dir.join(format!("{}.cer", name)),
        dir.join(format!("{}.key", name)),
    )
}

/// SANs from `openssl x509 -noout -ext subjectAltName`.
pub fn cert_sans(cert: &Path) -> Vec<String> {
    let output = Command::new("openssl")
        .args([
            "x509",
            "-in",
            &cert.display().to_string(),
            "-noout",
            "-ext",
            "subjectAltName",
        ])
        .output();
    let text = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return Vec::new(),
    };
    parse_sans(&text)
}

pub fn parse_sans(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split([',', '\n']) {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("DNS:") {
            let name = rest.trim();
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Same matching rule as `add-sub-site.sh` `cert_covers`.
pub fn cert_covers_domain(sans: &[String], domain: &str) -> bool {
    for san in sans {
        if san == domain {
            return true;
        }
        if let Some(suffix) = san.strip_prefix("*.") {
            if let Some(prefix) = domain.strip_suffix(&format!(".{}", suffix)) {
                if !prefix.is_empty() && !prefix.contains('.') {
                    return true;
                }
            }
        }
    }
    false
}

pub fn find_cert(db: &Db, domain: &str) -> Option<String> {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    for i in 0..parts.len() - 1 {
        let candidate = parts[i..].join(".");
        let (cer, key) = crate::compat::resolve_pair(db, &candidate);
        if cer.is_file() && key.is_file() {
            let sans = cert_sans(&cer);
            if sans.is_empty() || cert_covers_domain(&sans, domain) {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn openssl_info(cert: &Path) -> String {
    let output = Command::new("openssl")
        .args([
            "x509",
            "-in",
            &cert.display().to_string(),
            "-noout",
            "-subject",
            "-ext",
            "subjectAltName",
            "-enddate",
        ])
        .output();
    match output {
        Ok(o) => {
            let mut t = String::from_utf8_lossy(&o.stdout).to_string();
            t.push_str(&String::from_utf8_lossy(&o.stderr));
            t
        }
        Err(err) => err.to_string(),
    }
}

pub fn list_ssl_files(db: &Db) -> Vec<Value> {
    let mut out = Vec::new();
    for pair in crate::compat::discover_all_certs(db) {
        out.push(json!({
            "name": pair.name,
            "cer": pair.cer.display().to_string(),
            "key": pair.key.display().to_string(),
            "hasKey": pair.key.is_file(),
            "source": pair.source,
            "ecc": pair.ecc,
        }));
    }
    if out.is_empty() {
        let dir = ssl_dir(db);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(".cer") {
                    let (cer, key) = cert_paths(db, stem);
                    out.push(json!({
                        "name": stem,
                        "cer": cer.display().to_string(),
                        "key": key.display().to_string(),
                        "hasKey": key.is_file(),
                        "source": "sslDir",
                    }));
                }
            }
        }
    }
    out.sort_by(|a, b| str_of(a, "name").cmp(&str_of(b, "name")));
    out
}

pub fn has_http3(db: &Db) -> bool {
    let exe = nginx::nginx_exe(db);
    match Command::new(&exe).arg("-V").output() {
        Ok(o) => {
            let mut t = String::from_utf8_lossy(&o.stdout).to_string();
            t.push_str(&String::from_utf8_lossy(&o.stderr));
            t.contains("http_v3_module")
        }
        Err(_) => false,
    }
}

fn proxy_url(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("proxyRequired".into());
    }
    if t.chars()
        .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '"' | '\''))
    {
        return Err("badProxy".into());
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return Ok(format!("http://127.0.0.1:{}", t));
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        return Ok(t.to_string());
    }
    Ok(format!("http://{}", t))
}

fn safe_index(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty()
        || t.chars()
            .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '"' | '\''))
    {
        return "index.html".into();
    }
    t.to_string()
}

fn safe_root(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("badDir".into());
    }
    if t.chars()
        .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '"' | '\0'))
    {
        return Err("badDir".into());
    }
    Ok(t.replace('\\', "/"))
}

fn safe_loc_path(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    let t = if t.is_empty() { "/" } else { t };
    if t.chars()
        .any(|c| c.is_control() || matches!(c, '{' | '}' | ';' | '"' | '\0'))
    {
        return Err("badPath".into());
    }
    Ok(t.to_string())
}

fn safe_upstream_name(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("upstreamRequired".into());
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("badUpstream".into());
    }
    Ok(t.to_string())
}

pub fn loc_type(loc: &Value) -> String {
    let t = str_of(loc, "type");
    match t.as_str() {
        "0" | "proxy" | "http" => "proxy".into(),
        "1" | "static" | "html" => "static".into(),
        "2" | "upstream" => "upstream".into(),
        "4" | "redirect" | "return" => "redirect".into(),
        "3" | "custom" => "custom".into(),
        _ => {
            if !str_of(loc, "proxy").is_empty() {
                "proxy".into()
            } else {
                "static".into()
            }
        }
    }
}

fn loc_path(loc: &Value) -> String {
    let p = str_of(loc, "path");
    if p.is_empty() {
        "/".into()
    } else {
        p
    }
}

fn loc_index(loc: &Value) -> String {
    let i = str_of(loc, "index");
    if i.is_empty() {
        safe_index(&str_of(loc, "rootPage"))
    } else {
        safe_index(&i)
    }
}

fn site_root(db: &Db, site: &Value, loc: Option<&Value>) -> String {
    if let Some(loc) = loc {
        let r = str_of(loc, "root");
        if !r.is_empty() {
            return r;
        }
        let r = str_of(loc, "rootPath");
        if !r.is_empty() {
            return r;
        }
    }
    let r = str_of(site, "root");
    if !r.is_empty() {
        return r;
    }
    let domain = str_of(site, "domain");
    web_root_base(db).join(domain).display().to_string()
}

pub fn site_kind(site: &Value) -> String {
    let kind = str_of(site, "kind");
    match kind.as_str() {
        "forward" | "fwd" => return "forward".into(),
        "reverse" | "proxy" => return "reverse".into(),
        "static" | "html" | "substatic" => return "static".into(),
        _ => {}
    }
    match str_of(site, "mode").as_str() {
        "forward" => return "forward".into(),
        "proxy" => return "reverse".into(),
        "static" | "substatic" => return "static".into(),
        _ => {}
    }
    if let Some(arr) = site.get("locations").and_then(|v| v.as_array()) {
        if arr
            .iter()
            .any(|l| matches!(loc_type(l).as_str(), "proxy" | "upstream"))
        {
            return "reverse".into();
        }
    }
    if !str_of(site, "proxy").is_empty() {
        return "reverse".into();
    }
    "static".into()
}

/// Old-project location list, or a single location inferred from mode/root/proxy.
pub fn locations_of(db: &Db, site: &Value) -> Vec<Value> {
    if site_kind(site) == "forward" {
        return Vec::new();
    }
    if let Some(arr) = site.get("locations").and_then(|v| v.as_array()) {
        if !arr.is_empty() {
            return arr.clone();
        }
    }
    let websocket = bool_of(site, "websocket", false);
    if site_kind(site) == "reverse" {
        vec![json!({
            "path": "/",
            "type": "proxy",
            "proxy": str_of(site, "proxy"),
            "websocket": websocket,
        })]
    } else {
        let index = {
            let i = str_of(site, "index");
            if i.is_empty() {
                "index.html".to_string()
            } else {
                i
            }
        };
        let mode = str_of(site, "mode");
        let cache = if mode == "substatic" { "sub" } else { "static" };
        vec![json!({
            "path": "/",
            "type": "static",
            "root": site_root(db, site, None),
            "index": index,
            "cache": cache,
        })]
    }
}

fn primary_static<'a>(locations: &'a [Value]) -> Option<&'a Value> {
    if locations.len() == 1 {
        let loc = &locations[0];
        if loc_type(loc) == "static" && loc_path(loc) == "/" {
            return Some(loc);
        }
    }
    None
}

fn emit_proxy_location(out: &mut String, path: &str, url: &str, websocket: bool) {
    out.push_str(&format!("    location {path} {{\n"));
    out.push_str(&format!("        proxy_pass {url};\n"));
    out.push_str("        proxy_set_header Host $host;\n");
    out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
    out.push_str("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n");
    out.push_str("        proxy_set_header X-Forwarded-Proto $scheme;\n");
    if websocket {
        out.push_str("        proxy_http_version 1.1;\n");
        out.push_str("        proxy_set_header Upgrade $http_upgrade;\n");
        out.push_str("        proxy_set_header Connection \"upgrade\";\n");
        out.push_str("        proxy_buffering off;\n");
        out.push_str("        proxy_read_timeout 86400s;\n");
        out.push_str("        proxy_send_timeout 86400s;\n");
    }
    out.push_str("    }\n");
}

fn safe_listen(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "80".into();
    }
    if t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 5 {
        if let Ok(port) = t.parse::<u32>() {
            if (1..=65535).contains(&port) {
                return t.to_string();
            }
        }
    }
    "80".into()
}

fn emit_upstream_block(db: &Db, out: &mut String, uniq: &str, name: &str) -> bool {
    let row = match db
        .rows("upstream")
        .iter()
        .find(|u| str_of(u, "name") == name)
    {
        Some(row) => row.clone(),
        None => return false,
    };
    let id = str_of(&row, "id");
    let mut nodes: Vec<Value> = db
        .rows("upstreamServer")
        .iter()
        .filter(|n| str_of(n, "upstreamId") == id && bool_of(n, "enable", true))
        .cloned()
        .collect();
    if nodes.is_empty() {
        return false;
    }
    nodes.sort_by(|a, b| str_of(a, "id").cmp(&str_of(b, "id")));
    out.push_str(&format!("upstream {uniq} {{\n"));
    for node in nodes {
        let host = str_of(&node, "server");
        let host = if host.is_empty() {
            "127.0.0.1".into()
        } else {
            host
        };
        if host
            .chars()
            .any(|c| matches!(c, ';' | '{' | '}' | '"' | '\n'))
        {
            continue;
        }
        let port = crate::store::int_of(&node, "port");
        if port > 0 {
            out.push_str(&format!("    server {host}:{port};\n"));
        } else {
            out.push_str(&format!("    server {host};\n"));
        }
    }
    out.push_str("}\n\n");
    true
}

fn emit_static_location(out: &mut String, path: &str, root: &str, index: &str) {
    if path == "/" {
        out.push_str(&format!("    root {root};\n    index {index};\n"));
        out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
        return;
    }
    let alias = if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{root}/")
    };
    out.push_str(&format!("    location {path} {{\n"));
    out.push_str(&format!("        alias {alias};\n"));
    out.push_str(&format!("        index {index};\n"));
    out.push_str("    }\n");
}

fn safe_resolver(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "223.5.5.5 8.8.8.8".into();
    }
    let ok = t.split_whitespace().all(|part| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '='))
    });
    if ok {
        t.to_string()
    } else {
        "223.5.5.5 8.8.8.8".into()
    }
}

fn parse_allow(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == '\n' || c == ';')
        .map(|s| s.trim().to_string())
        .filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '-'))
        })
        .collect()
}

fn render_forward(db: &Db, site: &Value) -> Result<String, String> {
    let name = safe_domain(&str_of(site, "domain")).ok_or("badDomain")?;
    let listen = {
        let t = str_of(site, "listen");
        if t.trim().is_empty() {
            "8888".into()
        } else {
            safe_listen(&t)
        }
    };
    let ipv6 = bool_of(site, "ipv6", false);
    let resolver = safe_resolver(&str_of(site, "resolver"));
    let allow = parse_allow(&str_of(site, "allow"));
    let mut out = String::from("server {\n");
    out.push_str(&format!("    listen {listen};\n"));
    if ipv6 {
        out.push_str(&format!("    listen [::]:{listen};\n"));
    }
    out.push_str("    server_name _;\n");
    for cidr in &allow {
        out.push_str(&format!("    allow {cidr};\n"));
    }
    if !allow.is_empty() {
        out.push_str("    deny all;\n");
    }
    out.push_str(&format!("    resolver {resolver} ipv6=off;\n"));
    out.push_str("    resolver_timeout 5s;\n\n");
    out.push_str("    location / {\n");
    out.push_str("        proxy_pass $scheme://$http_host$request_uri;\n");
    out.push_str("        proxy_set_header Host $http_host;\n");
    out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
    out.push_str("        proxy_buffers 256 8k;\n");
    out.push_str("        proxy_max_temp_file_size 0;\n");
    out.push_str("        proxy_connect_timeout 10s;\n");
    out.push_str("        proxy_read_timeout 300s;\n");
    out.push_str("        proxy_send_timeout 300s;\n");
    out.push_str("    }\n");
    let log_dir = nginx_path_text(&access_log_dir(db));
    out.push_str(&format!(
        "\n    access_log {log_dir}/{name}.access.log;\n    error_log  {log_dir}/{name}.error.log;\n}}\n"
    ));
    Ok(out)
}

/// Render one site file, same shape as the shell scripts.
pub fn render(db: &Db, site: &Value) -> Result<String, String> {
    if site_kind(site) == "forward" {
        return render_forward(db, site);
    }
    let domain = safe_domain(&str_of(site, "domain")).ok_or("badDomain")?;
    let ssl = bool_of(site, "ssl", true);
    let http2 = bool_of(site, "http2", true);
    let redirect = bool_of(site, "redirect", ssl);
    let ipv6 = bool_of(site, "ipv6", false);
    let mode = str_of(site, "mode");
    let locations = locations_of(db, site);
    if locations.is_empty() {
        return Err("locationRequired".into());
    }

    let mut cert_name = str_of(site, "certName");
    if cert_name.is_empty() {
        cert_name = find_cert(db, &domain).unwrap_or_else(|| domain.clone());
    }
    let (cer, key) = crate::compat::resolve_pair(db, &cert_name);
    if ssl && (!cer.is_file() || !key.is_file()) {
        // preview still emits paths; apply() checks files exist
    }

    let params = ssl_params(db);
    let include_params = params.is_file();
    let http3 = has_http3(db);
    let primary = primary_static(&locations);
    let sub_cache = mode == "substatic"
        || primary
            .map(|l| str_of(l, "cache") == "sub")
            .unwrap_or(false);

    let mut prefix = String::new();
    for loc in &locations {
        if loc_type(loc) == "upstream" {
            if let Ok(name) = safe_upstream_name(&str_of(loc, "upstream")) {
                let uniq = format!(
                    "site_{}_{}",
                    domain.replace(['.', '-'], "_"),
                    name.replace('-', "_")
                );
                let _ = emit_upstream_block(db, &mut prefix, &uniq, &name);
            }
        }
    }

    let mut out = prefix;
    if ssl && redirect {
        out.push_str("server {\n    listen 80;\n");
        if ipv6 {
            out.push_str("    listen [::]:80;\n");
        }
        out.push_str(&format!(
            "    server_name {domain};\n    return 301 https://$host$request_uri;\n}}\n\n"
        ));
    }

    out.push_str("server {\n");
    if ssl {
        out.push_str("    listen 443 ssl;\n");
        if ipv6 {
            out.push_str("    listen [::]:443 ssl;\n");
        }
        if http2 {
            out.push_str("    http2 on;\n");
        }
        if http3 && primary.is_some() && !sub_cache {
            out.push_str("    listen 443 quic;\n");
            out.push_str("    http3 on;\n");
        }
    } else {
        let listen = safe_listen(&str_of(site, "listen"));
        out.push_str(&format!("    listen {listen};\n"));
        if ipv6 {
            out.push_str(&format!("    listen [::]:{listen};\n"));
        }
    }
    out.push_str(&format!("    server_name {domain};\n"));
    if ssl {
        out.push_str(&format!(
            "\n    ssl_certificate     {};\n",
            nginx_path_text(&cer)
        ));
        out.push_str(&format!(
            "    ssl_certificate_key {};\n",
            nginx_path_text(&key)
        ));
        if include_params {
            out.push_str(&format!("    include {};\n", nginx_path_text(&params)));
        }
    }

    if let Some(loc) = primary {
        let root = safe_root(&site_root(db, site, Some(loc)))?;
        let index = loc_index(loc);
        if ssl && !sub_cache {
            out.push_str("\n    ssl_stapling on;\n    ssl_stapling_verify on;\n");
            out.push_str(&format!(
                "    ssl_trusted_certificate {};\n",
                nginx_path_text(&cer)
            ));
            out.push_str("    resolver 223.5.5.5 8.8.8.8 valid=300s ipv6=off;\n");
            out.push_str("    resolver_timeout 2s;\n\n");
            out.push_str("    charset utf-8;\n    sendfile on;\n    tcp_nopush on;\n    tcp_nodelay on;\n    keepalive_timeout 65;\n\n");
            out.push_str("    open_file_cache max=2000 inactive=60s;\n    open_file_cache_valid 30s;\n    open_file_cache_min_uses 1;\n    open_file_cache_errors on;\n\n");
            out.push_str(&format!("    root {root};\n    index {index};\n\n"));
            out.push_str("    gzip on;\n    gzip_vary on;\n    gzip_proxied any;\n    gzip_comp_level 5;\n    gzip_min_length 256;\n");
            out.push_str("    gzip_types text/plain text/css text/xml application/javascript application/json application/xml image/svg+xml;\n\n");
            out.push_str("    location = / {\n        add_header Cache-Control \"no-cache\";\n");
            if http3 {
                out.push_str("        add_header Alt-Svc 'h3=\":443\"; ma=86400';\n");
            }
            out.push_str(&format!("        try_files /{index} =404;\n    }}\n\n"));
            out.push_str("    location ~* \\.html?$ {\n        add_header Cache-Control \"no-cache\";\n        try_files $uri =404;\n    }\n\n");
            out.push_str("    location ~* \\.(?:js|css|png|jpe?g|gif|ico|svg|webp|avif|woff2?|ttf|eot)$ {\n        expires 7d;\n        add_header Cache-Control \"public\";\n        access_log off;\n        try_files $uri =404;\n    }\n\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
        } else {
            out.push_str(&format!("\n    root {root};\n    index {index};\n\n"));
            out.push_str("    location ~* \\.html?$ {\n        expires -1;\n        try_files $uri =404;\n    }\n\n");
            out.push_str("    location ~* \\.(?:js|css|png|jpe?g|gif|ico|svg|webp|woff2?|ttf|eot)$ {\n        expires 1y;\n        add_header Cache-Control \"public, immutable\";\n        access_log off;\n        try_files $uri =404;\n    }\n\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
        }
    } else {
        out.push('\n');
        for loc in &locations {
            let path = safe_loc_path(&loc_path(loc))?;
            match loc_type(loc).as_str() {
                "proxy" => {
                    let url = proxy_url(&str_of(loc, "proxy"))?;
                    emit_proxy_location(&mut out, &path, &url, bool_of(loc, "websocket", false));
                }
                "upstream" => {
                    let name = safe_upstream_name(&str_of(loc, "upstream"))?;
                    let uniq = format!(
                        "site_{}_{}",
                        domain.replace(['.', '-'], "_"),
                        name.replace('-', "_")
                    );
                    let target = if out.contains(&format!("upstream {uniq} {{")) {
                        uniq
                    } else {
                        name
                    };
                    emit_proxy_location(
                        &mut out,
                        &path,
                        &format!("http://{}", target),
                        bool_of(loc, "websocket", false),
                    );
                }
                "redirect" => {
                    let url = str_of(loc, "returnUrl");
                    let url = if url.trim().is_empty() {
                        "https://$host$request_uri".to_string()
                    } else if url
                        .chars()
                        .any(|c| matches!(c, ';' | '{' | '}' | '"' | '\n'))
                    {
                        return Err("badRedirect".into());
                    } else {
                        url
                    };
                    out.push_str(&format!(
                        "    location {path} {{\n        return 301 {url};\n    }}\n"
                    ));
                }
                "custom" => {
                    let body = str_of(loc, "custom");
                    out.push_str(&format!("    location {path} {{\n"));
                    for line in body.lines() {
                        out.push_str("        ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push_str("    }\n");
                }
                _ => {
                    let root = safe_root(&site_root(db, site, Some(loc)))?;
                    emit_static_location(&mut out, &path, &root, &loc_index(loc));
                }
            }
            out.push('\n');
        }
    }

    let log_dir = nginx_path_text(&access_log_dir(db));
    out.push_str(&format!(
        "\n    access_log {log_dir}/{domain}.access.log;\n    error_log  {log_dir}/{domain}.error.log;\n}}\n"
    ));
    Ok(out)
}

fn require_sites_include(db: &Db, confirm: bool) -> Result<(), String> {
    match apply_sites_include(db, confirm)? {
        SitesIncludeOutcome::NeedsConfirm { .. } => Err("needsSitesIncludeConfirm".into()),
        _ => Ok(()),
    }
}

pub fn apply(db: &Db, site: &Value) -> Result<(String, String), String> {
    let domain = safe_domain(&str_of(site, "domain")).ok_or("badDomain")?;
    if bool_of(site, "fromDisk", false) {
        if let Some(kept) = keep_existing_site(db, site) {
            return kept;
        }
    }
    if site_kind(site) != "forward" && bool_of(site, "ssl", true) {
        let mut cert_name = str_of(site, "certName");
        if cert_name.is_empty() {
            cert_name = find_cert(db, &domain).unwrap_or_default();
        }
        if cert_name.is_empty() {
            return Err(format!("no certificate covers {}", domain));
        }
        let (cer, key) = crate::compat::resolve_pair(db, &cert_name);
        if !cer.is_file() {
            return Err(format!("missing {}", cer.display()));
        }
        if !key.is_file() {
            return Err(format!("missing {}", key.display()));
        }
    }
    let text = render(db, site)?;
    if site_kind(site) != "forward" && bool_of(site, "ssl", true) {
        ensure_ssl_params(db)?;
    }
    let available = sites_available(db);
    let enabled = sites_enabled(db);
    crate::privfs::mkdir_all(&available)?;
    crate::privfs::mkdir_all(&enabled)?;

    let (avail_path, enabled_path, in_place) = site_write_paths(db, site, &domain);
    let previous = crate::privfs::read_to_string(&avail_path).ok();
    let had_link = enabled_path.exists();

    let locations = locations_of(db, site);
    for loc in &locations {
        if loc_type(loc) == "static" {
            let root = PathBuf::from(site_root(db, site, Some(loc)));
            if !root.exists() {
                crate::privfs::mkdir_www(&root)?;
            }
        }
    }
    let _ = crate::privfs::mkdir_all(&access_log_dir(db));
    require_sites_include(db, bool_of(site, "confirmInclude", false))?;

    crate::privfs::write(&avail_path, &text)?;
    if !in_place && !enabled_path.exists() {
        crate::privfs::link_or_copy(&avail_path, &enabled_path)?;
    }

    let (ok, output) = nginx::test_conf(db);
    if !ok {
        if let Some(prev) = previous {
            let _ = crate::privfs::write(&avail_path, prev);
        } else {
            let _ = crate::privfs::remove_file(&avail_path);
            if !had_link {
                let _ = crate::privfs::remove_file(&enabled_path);
            }
        }
        return Err(format!("nginx -t failed, rolled back\n{}", output));
    }

    let (rok, rout) = nginx::reload(db);
    Ok((
        avail_path.display().to_string(),
        format!("{}\nreload ok={} {}", output, rok, rout),
    ))
}

pub fn existing_conf_text(_db: &Db, site: &Value) -> Option<String> {
    let raw = str_of(site, "rawConf");
    if !raw.trim().is_empty() {
        return Some(raw);
    }
    let conf_path = str_of(site, "confPath");
    if conf_path.is_empty() {
        return None;
    }
    crate::privfs::read_to_string(Path::new(&conf_path))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn keep_existing_site(db: &Db, site: &Value) -> Option<Result<(String, String), String>> {
    let conf_path = str_of(site, "confPath");
    if conf_path.is_empty() {
        return None;
    }
    let path = PathBuf::from(&conf_path);
    if !path.is_file() {
        return None;
    }
    if let Err(err) = require_sites_include(db, bool_of(site, "confirmInclude", false)) {
        return Some(Err(err));
    }
    let (ok, output) = nginx::test_conf(db);
    if !ok {
        return Some(Err(format!("nginx -t failed\n{}", output)));
    }
    let (rok, rout) = nginx::reload(db);
    Some(Ok((
        path.display().to_string(),
        format!("kept existing file\n{}\nreload ok={} {}", output, rok, rout),
    )))
}

fn site_write_paths(db: &Db, site: &Value, domain: &str) -> (PathBuf, PathBuf, bool) {
    let available = sites_available(db);
    let enabled = sites_enabled(db);
    let avail_path = available.join(format!("{}.conf", domain));
    let enabled_path = enabled.join(format!("{}.conf", domain));
    let conf_path = str_of(site, "confPath");
    if bool_of(site, "fromDisk", false) && !conf_path.is_empty() {
        let p = PathBuf::from(&conf_path);
        if p != nginx::nginx_path(db) && p.is_file() {
            return (p.clone(), p, true);
        }
    }
    (avail_path, enabled_path, false)
}

pub fn remove_files(db: &Db, site: &Value) {
    let domain = str_of(site, "domain");
    if let Some(d) = safe_domain(&domain) {
        let _ = crate::privfs::remove_file(&sites_enabled(db).join(format!("{}.conf", d)));
        let _ = crate::privfs::remove_file(&sites_available(db).join(format!("{}.conf", d)));
    }
    let conf_path = str_of(site, "confPath");
    if conf_path.is_empty() {
        return;
    }
    let p = PathBuf::from(&conf_path);
    if p == nginx::nginx_path(db) {
        return;
    }
    let _ = crate::privfs::remove_file(&p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_san_matches_one_label() {
        let sans = vec!["*.cdn.example.com".into(), "cdn.example.com".into()];
        assert!(cert_covers_domain(&sans, "app.cdn.example.com"));
        assert!(cert_covers_domain(&sans, "cdn.example.com"));
        assert!(!cert_covers_domain(&sans, "a.b.cdn.example.com"));
        assert!(!cert_covers_domain(&sans, "other.com"));
    }

    #[test]
    fn parses_openssl_sans() {
        let text =
            "X509v3 Subject Alternative Name:\n    DNS:cdn.example.com, DNS:*.cdn.example.com\n";
        let sans = parse_sans(text);
        assert_eq!(sans, vec!["cdn.example.com", "*.cdn.example.com"]);
    }

    #[test]
    fn renders_old_form_proxy_location() {
        let db = Db::new();
        let site = json!({
            "domain": "app.example.com",
            "ssl": true,
            "http2": true,
            "redirect": true,
            "certName": "example.com",
            "locations": [{ "path": "/", "type": "proxy", "proxy": "8080", "websocket": true }]
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("listen 80;"));
        assert!(text.contains("listen 443 ssl;"));
        assert!(text.contains("http2 on;"));
        assert!(text.contains("ssl_certificate"));
        assert!(text.contains("proxy_pass http://127.0.0.1:8080;"));
        assert!(text.contains("proxy_set_header Upgrade"));
        let cer = nginx_path_text(&cert_paths(&db, "example.com").0);
        assert!(text.contains(&cer), "missing cert path in:\n{text}");
    }

    #[test]
    fn renders_static_like_add_static_site() {
        let db = Db::new();
        let site = json!({
            "domain": "www.example.com",
            "ssl": true,
            "redirect": true,
            "certName": "www.example.com",
            "locations": [{ "path": "/", "type": "static", "root": "/var/www/www.example.com" }]
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("root /var/www/www.example.com;"));
        assert!(text.contains("gzip on;"));
        assert!(text.contains("Cache-Control \"no-cache\""));
        let log = nginx_path_text(&access_log_dir(&db).join("www.example.com.access.log"));
        assert!(
            text.contains(&format!("access_log {log};")),
            "missing log in:\n{text}"
        );
    }

    #[test]
    fn mixed_locations_do_not_nest_alias_and_root() {
        let db = Db::new();
        let site = json!({
            "domain": "mix.example.com",
            "ssl": true,
            "certName": "mix.example.com",
            "locations": [
                { "path": "/", "type": "static", "root": "/var/www/mix" },
                { "path": "/api", "type": "proxy", "proxy": "8080" }
            ]
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("root /var/www/mix;"));
        assert!(!text.contains("alias /var/www/mix/;"));
        assert!(text.contains("location /api {"));
        assert!(text.contains("proxy_pass http://127.0.0.1:8080;"));
    }

    #[test]
    fn preview_payload_with_cert_keeps_ssl() {
        let db = Db::new();
        let site = json!({
            "kind": "reverse",
            "mode": "proxy",
            "domain": "app.example.com",
            "listen": "80",
            "ssl": true,
            "http2": true,
            "redirect": true,
            "certName": "app.example.com",
            "proxy": "8080",
            "apply": false
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("listen 443 ssl;"), "missing 443 in:\n{text}");
        assert!(text.contains("ssl_certificate"), "missing cert in:\n{text}");
        assert!(
            text.contains("ssl_certificate_key"),
            "missing key in:\n{text}"
        );
        assert!(text.contains("return 301 https://$host$request_uri;"));
    }

    #[test]
    fn renders_reverse_from_kind_and_proxy_field() {
        let db = Db::new();
        let site = json!({
            "kind": "reverse",
            "domain": "api.example.com",
            "ssl": true,
            "certName": "api.example.com",
            "proxy": "127.0.0.1:3000",
            "websocket": true
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("proxy_pass http://127.0.0.1:3000;"));
        assert!(text.contains("proxy_set_header Upgrade"));
        assert_eq!(site_kind(&site), "reverse");
    }

    #[test]
    fn renders_forward_proxy() {
        let db = Db::new();
        let site = json!({
            "kind": "forward",
            "domain": "forward-8888",
            "listen": "8888",
            "resolver": "223.5.5.5 8.8.8.8",
            "allow": "10.0.0.0/8\n127.0.0.1"
        });
        let text = render(&db, &site).unwrap();
        assert!(text.contains("listen 8888;"));
        assert!(text.contains("server_name _;"));
        assert!(text.contains("allow 10.0.0.0/8;"));
        assert!(text.contains("deny all;"));
        assert!(text.contains("proxy_pass $scheme://$http_host$request_uri;"));
        assert!(!text.contains("ssl_certificate"));
    }

    #[test]
    fn finds_legacy_acme_fullchain() {
        let dir = std::env::temp_dir().join("nginxwebui-test-acme-cert");
        let domain_dir = dir.join(".acme.sh").join("example.com");
        let _ = std::fs::create_dir_all(&domain_dir);
        std::fs::write(domain_dir.join("fullchain.cer"), "cer").unwrap();
        std::fs::write(domain_dir.join("example.com.key"), "key").unwrap();
        let mut db = Db::new();
        db.set_setting("home", &dir.display().to_string());
        db.set_setting("acmeHome", &dir.join(".acme.sh").display().to_string());
        db.set_setting("sslDir", &dir.join("ssl").display().to_string());
        let found = find_cert(&db, "www.example.com");
        assert_eq!(found.as_deref(), Some("example.com"));
        let (cer, key) = crate::compat::resolve_pair(&db, "example.com");
        assert!(cer.ends_with("fullchain.cer"));
        assert!(key.ends_with("example.com.key"));
    }

    #[test]
    fn existing_conf_prefers_raw_block() {
        let db = Db::new();
        let site = json!({
            "fromDisk": true,
            "rawConf": "server { server_name kept.example.com; }",
            "confPath": "/no/such/file.conf"
        });
        assert!(existing_conf_text(&db, &site)
            .unwrap()
            .contains("kept.example.com"));
    }

    #[test]
    fn listen_ignores_injection() {
        assert_eq!(safe_listen("8080"), "8080");
        assert_eq!(safe_listen("80; return 200"), "80");
        assert_eq!(safe_listen("99999"), "80");
    }

    #[test]
    fn proxy_rejects_injection() {
        assert!(proxy_url("8080; return 200").is_err());
        assert_eq!(proxy_url("8080").unwrap(), "http://127.0.0.1:8080");
    }

    #[test]
    fn plans_insert_after_real_http_block() {
        let text = "events { worker_connections 64; }\nhttp {\n    include mime.types;\n}\n";
        let include = "include /tmp/sites-enabled/*.conf;";
        match plan_sites_include_text(text, include) {
            InsertPlanView::Ready { new_text } => {
                assert!(new_text.contains(include));
                let http_at = new_text.find("http {").unwrap();
                let inc_at = new_text.find(include).unwrap();
                assert!(inc_at > http_at);
                assert!(
                    inc_at < new_text.find("include mime.types").unwrap() + 30
                        || inc_at < new_text.find("}").unwrap()
                );
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn ignores_http_token_inside_comment() {
        let text = "# example: http {\nevents { worker_connections 64; }\nhttp {\n    include mime.types;\n}\n";
        let include = "include /tmp/sites-enabled/*.conf;";
        match plan_sites_include_text(text, include) {
            InsertPlanView::Ready { new_text } => {
                // Must not splice into the leading comment.
                let first_line = new_text.lines().next().unwrap();
                assert!(first_line.starts_with("# example"));
                assert!(!first_line.contains("sites-enabled"));
                let real = new_text.find("http {").unwrap();
                // skip the commented one
                let real = new_text[real + 1..]
                    .find("http {")
                    .map(|n| n + real + 1)
                    .unwrap();
                assert!(new_text[real..].contains(include));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn commented_only_http_block_needs_confirm_and_does_not_write() {
        let dir = std::env::temp_dir().join("nginxwebui-test-include-comment");
        let _ = std::fs::create_dir_all(&dir);
        let conf = dir.join("nginx.conf");
        let original = "worker_processes 1;\n# http {\n#     include mime.types;\n# }\n";
        std::fs::write(&conf, original).unwrap();
        let mut db = Db::new();
        db.set_setting("nginxPath", &conf.display().to_string());
        db.set_setting(
            "sitesEnabled",
            &dir.join("sites-enabled").display().to_string(),
        );
        match ensure_main_includes_sites(&db).unwrap() {
            SitesIncludeOutcome::NeedsConfirm {
                include_line,
                preview,
                reason,
            } => {
                assert!(include_line.contains("sites-enabled"));
                assert!(preview.contains("http {"));
                assert!(preview.contains(&include_line));
                assert_eq!(reason, "commentedHttpBlock");
            }
            other => panic!("expected NeedsConfirm, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), original);

        match apply_sites_include(&db, true).unwrap() {
            SitesIncludeOutcome::Written => {}
            other => panic!("expected Written after confirm, got {other:?}"),
        }
        let written = std::fs::read_to_string(&conf).unwrap();
        assert!(written.contains(original.trim_end()));
        assert!(written.contains("http {"));
        assert!(written.contains("sites-enabled"));
        // confirm write should not have mutated the comment into an http block
        assert!(written
            .lines()
            .any(|l| l.trim_start().starts_with("# http {")));
    }

    #[test]
    fn existing_include_is_not_duplicated() {
        let text = "http {\n    include /opt/nginx/sites-enabled/*.conf;\n}\n";
        let include = "include /opt/nginx/sites-enabled/*.conf;";
        assert_eq!(
            plan_sites_include_text(text, include),
            InsertPlanView::AlreadyPresent
        );
        // commented include does not count as present
        let commented =
            "# include /opt/nginx/sites-enabled/*.conf;\nhttp {\n    include mime.types;\n}\n";
        match plan_sites_include_text(commented, include) {
            InsertPlanView::Ready { new_text } => {
                assert_eq!(new_text.matches("sites-enabled").count(), 2);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn http_brace_on_following_line_is_found() {
        let text = "http\n{\n    include mime.types;\n}\n";
        let include = "include /tmp/sites-enabled/*.conf;";
        match plan_sites_include_text(text, include) {
            InsertPlanView::Ready { new_text } => {
                assert!(new_text.contains(include));
                let brace = new_text.find('{').unwrap();
                let inc = new_text.find(include).unwrap();
                assert!(inc > brace);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn adds_sites_include_when_missing() {
        let dir = std::env::temp_dir().join("nginxwebui-test-include");
        let _ = std::fs::create_dir_all(&dir);
        let conf = dir.join("nginx.conf");
        std::fs::write(
            &conf,
            "worker_processes 1;\nevents { worker_connections 64; }\n",
        )
        .unwrap();
        let mut db = Db::new();
        db.set_setting("nginxPath", &conf.display().to_string());
        db.set_setting(
            "sitesEnabled",
            &dir.join("sites-enabled").display().to_string(),
        );
        ensure_main_includes_sites(&db).unwrap();
        let text = std::fs::read_to_string(&conf).unwrap();
        assert!(text.contains("sites-enabled"));
        assert!(text.contains("http {"));
        ensure_main_includes_sites(&db).unwrap();
        let again = std::fs::read_to_string(&conf).unwrap();
        assert_eq!(
            text.matches("sites-enabled").count(),
            again.matches("sites-enabled").count()
        );
    }
}
