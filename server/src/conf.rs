//! Turns the stored settings into nginx configuration text.
//!
//! This mirrors what `ConfService` did in the Java version: the top level holds
//! the "basic" parameters, then an `http {}` block with HTTP parameters,
//! upstreams and servers, and a `stream {}` block with TCP/UDP forwards. Each
//! server can either be written inline or into its own small file under
//! `conf.d/` that the main file includes.

use crate::store::{bool_of, int_of, ordered, str_of, Db};
use serde_json::Value;

#[derive(Clone)]
pub struct ConfFile {
    pub name: String,
    pub conf: String,
}

pub struct ConfExt {
    pub conf: String,
    pub files: Vec<ConfFile>,
}

pub enum Ent {
    P(String),
    B(Block),
}

pub struct Block {
    pub head: String,
    pub ents: Vec<Ent>,
}

impl Block {
    pub fn new(head: &str) -> Block {
        Block {
            head: head.to_string(),
            ents: Vec::new(),
        }
    }

    pub fn p(&mut self, line: impl Into<String>) {
        let line = line.into();
        if !line.trim().is_empty() {
            self.ents.push(Ent::P(line));
        }
    }

    pub fn raw(&mut self, line: impl Into<String>) {
        self.ents.push(Ent::P(line.into()));
    }

    pub fn b(&mut self, block: Block) {
        self.ents.push(Ent::B(block));
    }

    pub fn is_empty(&self) -> bool {
        self.ents.is_empty()
    }

    #[allow(clippy::only_used_in_recursion)]
    fn render(&self, indent: usize, out: &mut String) {
        if !self.head.is_empty() {
            out.push_str(&" ".repeat(indent));
            out.push_str(&self.head);
            out.push_str(" {\n");
        }
        let inner = if self.head.is_empty() {
            indent
        } else {
            indent + 4
        };
        for ent in &self.ents {
            match ent {
                Ent::P(line) => {
                    out.push_str(&" ".repeat(inner));
                    out.push_str(line);
                    let trimmed = line.trim_end();
                    // a value that is itself a block (`events { ... }`) takes no `;`
                    if !trimmed.starts_with('#') && !trimmed.ends_with('}') {
                        out.push(';');
                    }
                    out.push('\n');
                }
                Ent::B(block) => block.render(inner, out),
            }
        }
        if !self.head.is_empty() {
            out.push_str(&" ".repeat(indent));
            out.push_str("}\n");
        }
    }
}

/// Strip characters nginx file names dislike, the way `ToolUtils.handlePath` did.
pub fn safe_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '?' | '<' | '>' | '|' | '"' | '#' | '&' | ';' | '\'' | '`' | '*' | '\\'
                )
        })
        .collect::<String>()
        .replace([':', '/'], "-")
}

fn get_or(value: &Value, key: &str, fallback: &str) -> String {
    let v = str_of(value, key);
    if v.trim().is_empty() {
        fallback.to_string()
    } else {
        v.trim().to_string()
    }
}

// ---------------------------------------------------------------- parameters

/// Parameters attached to a server / location / upstream, including those of the
/// templates marked as default for that kind (the Java version did the same).
fn param_lines(db: &Db, id: &str, kind: &str) -> Vec<String> {
    let mut out = Vec::new();

    for template in db.rows("template") {
        if str_of(template, "def") == kind {
            let tid = str_of(template, "id");
            for p in db
                .rows("param")
                .iter()
                .filter(|p| str_of(p, "templateId") == tid)
            {
                out.push(format!(
                    "{} {}",
                    str_of(p, "name").trim(),
                    str_of(p, "value").trim()
                ));
            }
        }
    }

    let column = format!("{}Id", kind);
    for p in db
        .rows("param")
        .iter()
        .filter(|p| str_of(p, &column) == id && !id.is_empty())
    {
        let template_value = str_of(p, "templateValue");
        if template_value.is_empty() {
            out.push(format!(
                "{} {}",
                str_of(p, "name").trim(),
                str_of(p, "value").trim()
            ));
        } else {
            let p_name = str_of(p, "name").trim().to_string();
            let p_value = str_of(p, "value").trim().to_string();
            for q in db
                .rows("param")
                .iter()
                .filter(|q| str_of(q, "templateId") == template_value)
            {
                let name = if p_name.is_empty() {
                    str_of(q, "name").trim().to_string()
                } else {
                    p_name.clone()
                };
                let value = if p_value.is_empty() {
                    str_of(q, "value").trim().to_string()
                } else {
                    p_value.clone()
                };
                out.push(format!("{} {}", name, value));
            }
        }
    }

    out.retain(|line| !line.trim().is_empty());
    out
}

// ------------------------------------------------------------------ upsteams

fn upstream_block(db: &Db, upstream: &Value) -> Block {
    let name = get_or(upstream, "name", "upstream");
    let mut block = Block::new(&format!("upstream {}", name));

    let tactics = str_of(upstream, "tactics");
    if !tactics.trim().is_empty() {
        block.p(tactics.trim());
    }

    let id = str_of(upstream, "id");
    let mut nodes: Vec<Value> = db
        .rows("upstreamServer")
        .iter()
        .filter(|n| str_of(n, "upstreamId") == id)
        .cloned()
        .collect();
    nodes.sort_by_key(|a| str_of(a, "id"));

    for node in nodes {
        if !bool_of(&node, "enable", true) {
            continue;
        }
        let host = get_or(&node, "server", "127.0.0.1");
        let port = int_of(&node, "port");
        let mut line = if port > 0 {
            format!("server {}:{}", host, port)
        } else {
            format!("server {}", host)
        };
        let weight = int_of(&node, "weight");
        if weight > 0 {
            line.push_str(&format!(" weight={}", weight));
        }
        let max_fails = str_of(&node, "maxFails");
        if !max_fails.trim().is_empty() {
            line.push_str(&format!(" max_fails={}", max_fails.trim()));
        }
        let fail_timeout = str_of(&node, "failTimeout");
        if !fail_timeout.trim().is_empty() {
            line.push_str(&format!(" fail_timeout={}", fail_timeout.trim()));
        }
        let max_conns = str_of(&node, "maxConns");
        if !max_conns.trim().is_empty() {
            line.push_str(&format!(" max_conns={}", max_conns.trim()));
        }
        let status = str_of(&node, "status");
        if status == "down" || status == "backup" {
            line.push_str(&format!(" {}", status));
        }
        let extra = str_of(&node, "param");
        if !extra.trim().is_empty() {
            line.push_str(&format!(" {}", extra.trim()));
        }
        block.p(line);
    }

    for line in param_lines(db, &id, "upstream") {
        block.p(line);
    }

    block
}

// ------------------------------------------------------------------- servers

fn listen_tail(ssl: bool, http2: i64, proxy_protocol: bool, is_default: bool) -> String {
    let mut tail = String::new();
    if ssl {
        tail.push_str(" ssl");
    }
    if http2 == 1 {
        tail.push_str(" http2");
    }
    if proxy_protocol {
        tail.push_str(" proxy_protocol");
    }
    if is_default {
        tail.push_str(" default_server");
    }
    tail
}

fn server_http_block(db: &Db, server: &Value) -> Block {
    let listen = get_or(server, "listen", "80");
    let ssl = int_of(server, "ssl") == 1;
    let http2 = int_of(server, "http2");
    let ipv6 = int_of(server, "ipv6") == 1;
    let is_default = int_of(server, "def") == 1;
    let tail = listen_tail(ssl, http2, int_of(server, "proxyProtocol") == 1, is_default);

    let mut block = Block::new("server");
    block.p(format!("listen {}", listen) + &tail);

    if ipv6 {
        let port = listen.rsplit(':').next().unwrap_or(&listen).to_string();
        block.p(format!("listen [::]:{}", port) + &tail);
    }

    if http2 == 2 {
        block.p("http2 on");
    }

    let name = str_of(server, "serverName");
    if !name.trim().is_empty() {
        block.p(format!("server_name {}", name.trim()));
    }

    let descr = str_of(server, "descr");
    if !descr.trim().is_empty() {
        block.raw(format!("# {}", descr.trim().replace('\n', " ")));
    }

    // password protection
    let password_id = str_of(server, "passwordId");
    if !password_id.is_empty() {
        if let Some(password) = db.get("password", &password_id) {
            block.p(format!("auth_basic \"{}\"", str_of(&password, "name")));
            block.p(format!(
                "auth_basic_user_file {}",
                str_of(&password, "path")
            ));
        }
    }

    // ssl
    if ssl {
        let pem = str_of(server, "pem");
        let key = str_of(server, "key");
        if !pem.trim().is_empty() {
            block.p(format!("ssl_certificate {}", pem.trim()));
        }
        if !key.trim().is_empty() {
            block.p(format!("ssl_certificate_key {}", key.trim()));
        }
        block.p(format!(
            "ssl_protocols {}",
            get_or(server, "protocols", "TLSv1 TLSv1.1 TLSv1.2 TLSv1.3")
        ));
        block.p(
            "ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256:ECDHE:ECDH:AES:HIGH:!NULL:!aNULL:!MD5:!ADH:!RC4:!DH:!DHE",
        );
        block.p("ssl_session_timeout 10m");
        block.p("ssl_session_cache shared:SSL:10m");
        block.p("ssl_prefer_server_ciphers on");
    }

    deny_allow_lines(db, &mut block, "server", server);

    let server_id = str_of(server, "id");
    for line in param_lines(db, &server_id, "server") {
        block.p(line);
    }

    // locations
    let mut locations: Vec<Value> = db
        .rows("location")
        .iter()
        .filter(|l| str_of(l, "serverId") == server_id)
        .cloned()
        .collect();
    locations.sort_by_key(|a| str_of(a, "id"));
    for location in locations {
        if int_of(&location, "enable") == 0 {
            continue;
        }
        block.b(location_block(db, &location));
    }

    block
}

fn location_block(db: &Db, location: &Value) -> Block {
    let path = get_or(location, "path", "/");
    let kind = int_of(location, "type");
    let mut block = Block::new(&format!("location {}", path));

    let descr = str_of(location, "descr");
    if !descr.trim().is_empty() {
        block.raw(format!("# {}", descr.trim().replace('\n', " ")));
    }

    match kind {
        // static files
        1 => {
            let root_path = str_of(location, "rootPath");
            let root_type = str_of(location, "rootType");
            if !root_path.trim().is_empty() {
                if root_type == "alias" {
                    block.p(format!("alias {}", root_path.trim()));
                } else {
                    block.p(format!("root {}", root_path.trim()));
                }
            }
            let page = str_of(location, "rootPage");
            if !page.trim().is_empty() {
                block.p(format!("index {}", page.trim()));
            }
        }
        // load balancing through an upstream
        2 => {
            let upstream_id = str_of(location, "upstreamId");
            let name = db
                .get("upstream", &upstream_id)
                .map(|u| get_or(&u, "name", "upstream"))
                .unwrap_or_default();
            let scheme = get_or(location, "upstreamType", "http");
            let extra = str_of(location, "upstreamPath");
            block.p(format!(
                "proxy_pass {}://{}{}",
                scheme,
                name,
                if extra.trim().is_empty() {
                    String::new()
                } else {
                    format!("/{}", extra.trim().trim_start_matches('/'))
                }
            ));
            proxy_headers(db, &mut block, location);
        }
        // redirect elsewhere
        4 => {
            let url = str_of(location, "returnUrl");
            let target = if int_of(location, "returnPath") == 1 {
                format!("{}$request_uri", url.trim_end_matches('/'))
            } else {
                url
            };
            block.p(format!("return 301 {}", target));
        }
        // blank block: only the custom parameters below
        3 => {}
        // default: reverse proxy to a fixed address
        _ => {
            let value = str_of(location, "value");
            if !value.trim().is_empty() {
                block.p(format!("proxy_pass {}", value.trim()));
            }
            proxy_headers(db, &mut block, location);
        }
    }

    for line in param_lines(db, &str_of(location, "id"), "location") {
        block.p(line);
    }

    block
}

fn proxy_headers(_db: &Db, block: &mut Block, location: &Value) {
    let mut headers = Block::new("");
    if int_of(location, "header") == 1 {
        headers.p(format!(
            "proxy_set_header Host {}",
            get_or(location, "headerHost", "$host")
        ));
    }
    headers.p("proxy_set_header X-Real-IP $remote_addr");
    headers.p("proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for");
    headers.p("proxy_set_header X-Forwarded-Proto $scheme");

    if int_of(location, "websocket") == 1 {
        headers.p("proxy_http_version 1.1");
        headers.p("proxy_set_header Upgrade $http_upgrade");
        headers.p("proxy_set_header Connection \"upgrade\"");
    }

    if int_of(location, "cros") == 1 {
        headers.p("add_header Access-Control-Allow-Origin *");
        headers.p("add_header Access-Control-Allow-Methods *");
        headers.p("add_header Access-Control-Allow-Headers *");
        headers.p("add_header Access-Control-Allow-Credentials true");
        let mut preflight = Block::new("if ($request_method = 'OPTIONS')");
        preflight.p("add_header Access-Control-Allow-Origin *");
        preflight.p("add_header Access-Control-Allow-Methods *");
        preflight.p("add_header Access-Control-Allow-Headers *");
        preflight.p("add_header Access-Control-Allow-Credentials true");
        preflight.p("add_header Content-Length 0");
        preflight.p("add_header Content-Type 'text/plain; charset=utf-8'");
        preflight.p("return 204");
        headers.b(preflight);
    }

    for ent in headers.ents {
        block.ents.push(ent);
    }
}

fn server_stream_block(db: &Db, server: &Value) -> Block {
    let mut block = Block::new("server");
    block.p(format!("listen {}", get_or(server, "listen", "80")));

    let proxy_protocol = int_of(server, "proxyProtocol") == 1;
    if proxy_protocol {
        block.p("proxy_protocol on");
    }

    let descr = str_of(server, "descr");
    if !descr.trim().is_empty() {
        block.raw(format!("# {}", descr.trim().replace('\n', " ")));
    }

    let upstream_id = str_of(server, "proxyUpstreamId");
    if !upstream_id.is_empty() {
        if let Some(upstream) = db.get("upstream", &upstream_id) {
            block.p(format!(
                "proxy_pass {}",
                get_or(&upstream, "name", "upstream")
            ));
        }
    }

    deny_allow_lines(db, &mut block, "server", server);
    for line in param_lines(db, &str_of(server, "id"), "server") {
        block.p(line);
    }
    block
}

// ------------------------------------------------------------ deny / allow IPs

fn deny_allow_lines(db: &Db, block: &mut Block, kind: &str, server: &Value) {
    let (mode, deny_id, allow_id) = if kind == "server" {
        (
            int_of(server, "denyAllow"),
            str_of(server, "denyId"),
            str_of(server, "allowId"),
        )
    } else if kind == "http" {
        (
            db.setting("denyAllow").parse().unwrap_or(0),
            db.setting("denyId"),
            db.setting("allowId"),
        )
    } else {
        (
            db.setting("denyAllowStream").parse().unwrap_or(0),
            db.setting("denyIdStream"),
            db.setting("allowIdStream"),
        )
    };

    if mode == 0 {
        return;
    }
    let ips = |id: &str| -> Vec<String> {
        db.get("denyAllow", id)
            .map(|row| {
                str_of(&row, "ip")
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    if mode == 1 || mode == 3 {
        for ip in ips(&deny_id) {
            block.p(format!("deny {}", ip));
        }
    }
    if mode == 2 || mode == 3 {
        for ip in ips(&allow_id) {
            block.p(format!("allow {}", ip));
        }
    }
}

// --------------------------------------------------------------------- driver

pub fn build_conf(db: &Db, decompose: bool) -> ConfExt {
    let mut files: Vec<ConfFile> = Vec::new();
    let mut root = Block::new("");

    for basic in ordered(db, "basic", true) {
        let name = str_of(&basic, "name");
        if name.trim().is_empty() {
            continue;
        }
        root.p(format!(
            "{} {}",
            name.trim(),
            str_of(&basic, "value").trim()
        ));
    }

    // ---- http
    let mut http = Block::new("http");
    let mut has_http = false;

    for param in ordered(db, "http", true) {
        if !bool_of(&param, "enable", true) {
            continue;
        }
        let name = str_of(&param, "name");
        if name.trim().is_empty() {
            continue;
        }
        http.p(format!(
            "{} {}",
            name.trim(),
            str_of(&param, "value").trim()
        ));
        has_http = true;
    }

    deny_allow_lines(db, &mut http, "http", &Value::Null);

    let sites_enabled = db.setting("sitesEnabled");
    if !sites_enabled.trim().is_empty() {
        let glob = format!("{}/*.conf", sites_enabled.trim()).replace('\\', "/");
        http.p(format!("include {}", glob));
        has_http = true;
    }

    for upstream in ordered(db, "upstream", false) {
        if int_of(&upstream, "proxyType") != 0 {
            continue;
        }
        has_http = true;
        let name = str_of(&upstream, "name");
        if decompose {
            let file_name = format!(
                "upstream_{}_{}.conf",
                str_of(&upstream, "id"),
                safe_name(&name)
            );
            let mut single = Block::new("");
            single.b(upstream_block(db, &upstream));
            files.push(ConfFile {
                name: file_name.clone(),
                conf: render(&single),
            });
            http.p(format!("include conf.d/{}", file_name));
        } else {
            http.b(upstream_block(db, &upstream));
        }
    }

    for server in ordered(db, "server", false) {
        if int_of(&server, "proxyType") != 0 || !bool_of(&server, "enable", true) {
            continue;
        }
        has_http = true;

        // plain HTTP that just sends visitors to HTTPS
        if int_of(&server, "rewrite") == 1 && int_of(&server, "ssl") == 1 {
            let rewrite_listen = get_or(&server, "rewriteListen", "80");
            let mut redirect = Block::new("server");
            redirect.p(format!("listen {}", rewrite_listen));
            let name = str_of(&server, "serverName");
            if !name.trim().is_empty() {
                redirect.p(format!("server_name {}", name.trim()));
            }
            redirect.p("rewrite ^(.*)$ https://$host$request_uri permanent");
            if decompose {
                let file_name = format!(
                    "rewrite_{}_{}.conf",
                    str_of(&server, "id"),
                    safe_name(&get_or(&server, "serverName", "server"))
                );
                let mut single = Block::new("");
                single.b(redirect);
                files.push(ConfFile {
                    name: file_name.clone(),
                    conf: render(&single),
                });
                http.p(format!("include conf.d/{}", file_name));
            } else {
                http.b(redirect);
            }
        }

        if decompose {
            let file_name = format!(
                "http_{}_{}.conf",
                str_of(&server, "id"),
                safe_name(&get_or(&server, "serverName", "server"))
            );
            let mut single = Block::new("");
            single.b(server_http_block(db, &server));
            files.push(ConfFile {
                name: file_name.clone(),
                conf: render(&single),
            });
            http.p(format!("include conf.d/{}", file_name));
        } else {
            http.b(server_http_block(db, &server));
        }
    }

    if has_http {
        root.b(http);
    }

    // ---- stream (TCP / UDP forwarding)
    let mut stream = Block::new("stream");
    let mut has_stream = false;

    for param in ordered(db, "stream", true) {
        let name = str_of(&param, "name");
        if name.trim().is_empty() {
            continue;
        }
        stream.p(format!(
            "{} {}",
            name.trim(),
            str_of(&param, "value").trim()
        ));
        has_stream = true;
    }

    deny_allow_lines(db, &mut stream, "stream", &Value::Null);

    for upstream in ordered(db, "upstream", false) {
        let proxy_type = int_of(&upstream, "proxyType");
        if proxy_type == 0 {
            continue;
        }
        has_stream = true;
        if decompose {
            let file_name = format!(
                "upstream_{}_{}.conf",
                str_of(&upstream, "id"),
                safe_name(&get_or(&upstream, "name", "upstream"))
            );
            let mut single = Block::new("");
            single.b(upstream_block(db, &upstream));
            files.push(ConfFile {
                name: file_name.clone(),
                conf: render(&single),
            });
            stream.p(format!("include conf.d/{}", file_name));
        } else {
            stream.b(upstream_block(db, &upstream));
        }
    }

    for server in ordered(db, "server", false) {
        let proxy_type = int_of(&server, "proxyType");
        if proxy_type == 0 || !bool_of(&server, "enable", true) {
            continue;
        }
        has_stream = true;
        let prefix = if proxy_type == 2 { "udp" } else { "tcp" };
        if decompose {
            let file_name = format!(
                "{}_{}_{}.conf",
                prefix,
                str_of(&server, "id"),
                safe_name(&get_or(&server, "serverName", "server"))
            );
            let mut single = Block::new("");
            single.b(server_stream_block(db, &server));
            files.push(ConfFile {
                name: file_name.clone(),
                conf: render(&single),
            });
            stream.p(format!("include conf.d/{}", file_name));
        } else {
            stream.b(server_stream_block(db, &server));
        }
    }

    if has_stream {
        root.b(stream);
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    ConfExt {
        conf: format!("{}\n{}", GENERATED_MARK, render(&root).trim_start()),
        files,
    }
}

/// Written at the top of a generated nginx.conf. `write_conf` will not replace
/// an existing file that does not contain this line.
pub const GENERATED_MARK: &str = "# Generated by nginxWebUI";

pub fn render(block: &Block) -> String {
    let mut out = String::new();
    block.render(0, &mut out);
    out
}
