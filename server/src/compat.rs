//! Adopt an already-running nginx, leftover nginxWebUI (Java) home, and
//! local acme.sh certificates without rewriting live files.

use crate::acme;
use crate::nginx;
use crate::site;
use crate::store::{new_id, str_of, Db};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct CertPair {
    pub name: String,
    pub cer: PathBuf,
    pub key: PathBuf,
    pub source: String,
    pub ecc: bool,
}

pub fn adopt(db: &mut Db) -> Value {
    detect_nginx(db);
    detect_acme(db);
    let certs = import_certs(db);
    let sites = import_sites(db);
    json!({
        "nginxPath": nginx::nginx_path(db).display().to_string(),
        "nginxExe": nginx::nginx_exe(db),
        "acmeSh": acme::resolve_bin(db).display().to_string(),
        "acmeHome": acme::acme_home(db).display().to_string(),
        "certs": certs,
        "sites": sites,
        "sslDir": site::ssl_dir(db).display().to_string(),
    })
}

fn detect_nginx(db: &mut Db) {
    if db.setting("nginxExe").is_empty() {
        if let Some(exe) = nginx::which_nginx() {
            db.set_setting("nginxExe", &exe.display().to_string());
        }
    }
    let exe = nginx::nginx_exe(db);
    if let Some((conf, prefix, sbin)) = nginx::probe_binary(&exe) {
        let current = db.setting("nginxPath");
        if (current.is_empty() || !Path::new(&current).is_file()) && conf.is_file() {
            db.set_setting("nginxPath", &conf.display().to_string());
        }
        if db.setting("nginxDir").is_empty() && prefix.is_dir() {
            db.set_setting("nginxDir", &prefix.display().to_string());
        }
        if (db.setting("nginxExe").is_empty() || db.setting("nginxExe") == "nginx") && sbin.is_file()
        {
            db.set_setting("nginxExe", &sbin.display().to_string());
        }
    }
    let dir = nginx::nginx_dir(db);
    let avail = db.setting("sitesAvailable");
    if avail.is_empty() || !Path::new(&avail).is_dir() {
        if nginx::unix_is_dir("/etc/nginx/sites-available") {
            db.set_setting("sitesAvailable", "/etc/nginx/sites-available");
            db.set_setting("sitesEnabled", "/etc/nginx/sites-enabled");
        } else if dir.join("sites-available").is_dir() {
            db.set_setting(
                "sitesAvailable",
                &dir.join("sites-available").display().to_string(),
            );
            db.set_setting(
                "sitesEnabled",
                &dir.join("sites-enabled").display().to_string(),
            );
        } else if conf_d_has_sites(&dir.join("conf.d")) {
            let conf_d = dir.join("conf.d").display().to_string();
            db.set_setting("sitesAvailable", &conf_d);
            db.set_setting("sitesEnabled", &conf_d);
        }
    }
}

fn conf_d_has_sites(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x == "conf")
            .unwrap_or(false)
    })
}

fn detect_acme(db: &mut Db) {
    let bin = acme::resolve_bin(db);
    if bin.is_file() {
        if db.setting("acmeSh").is_empty() {
            db.set_setting("acmeSh", &bin.display().to_string());
        }
        if db.setting("acmeHome").is_empty() {
            if let Some(parent) = bin.parent() {
                db.set_setting("acmeHome", &parent.display().to_string());
            }
        }
    }
    let account = acme::acme_home(db).join("account.conf");
    let text = fs::read_to_string(account).unwrap_or_default();
    if db.setting("acmeEmail").is_empty() {
        if let Some(email) = account_value(&text, "ACCOUNT_EMAIL") {
            db.set_setting("acmeEmail", &email);
        }
    }
    if db.setting("CF_Token").is_empty() {
        if let Some(v) = account_value(&text, "SAVED_CF_Token") {
            db.set_setting("CF_Token", &v);
        }
    }
    if db.setting("CF_Account_ID").is_empty() {
        if let Some(v) = account_value(&text, "SAVED_CF_Account_ID") {
            db.set_setting("CF_Account_ID", &v);
        }
    }
    if db.setting("GCORE_Key").is_empty() {
        if let Some(v) = account_value(&text, "SAVED_GCORE_Key") {
            db.set_setting("GCORE_Key", &v);
        }
    }
}

fn account_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key).and_then(|s| s.strip_prefix('=')) {
            let v = rest.trim().trim_matches('\'').trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn cert_pairs(db: &Db, name: &str) -> Vec<CertPair> {
    let mut out = Vec::new();
    let mut push = |cer: PathBuf, key: PathBuf, source: &str, ecc: bool| {
        if cer.is_file() && key.is_file() {
            out.push(CertPair {
                name: name.to_string(),
                cer,
                key,
                source: source.into(),
                ecc,
            });
        }
    };
    let ssl = site::ssl_dir(db);
    push(ssl.join(format!("{name}.cer")), ssl.join(format!("{name}.key")), "sslDir", false);
    push(ssl.join(format!("{name}.pem")), ssl.join(format!("{name}.key")), "sslDir", false);
    push(
        ssl.join(name).join("fullchain.pem"),
        ssl.join(name).join("privkey.pem"),
        "certbot",
        false,
    );
    let live = PathBuf::from("/etc/letsencrypt/live").join(name);
    push(
        live.join("fullchain.pem"),
        live.join("privkey.pem"),
        "certbot",
        false,
    );
    for home in acme::acme_home_candidates(db) {
        let rsa = home.join(name);
        push(
            rsa.join("fullchain.cer"),
            rsa.join(format!("{name}.key")),
            "acme.sh",
            false,
        );
        let ecc = home.join(format!("{name}_ecc"));
        push(
            ecc.join("fullchain.cer"),
            ecc.join(format!("{name}.key")),
            "acme.sh",
            true,
        );
    }
    let java_home = PathBuf::from(db.setting("home"));
    let wild = name.replace("*.", "_");
    for folder in [name.to_string(), wild] {
        let dir = java_home.join("cert").join(&folder);
        push(
            dir.join("certificate.pem"),
            dir.join("private.key"),
            "nginxWebUI",
            false,
        );
        push(dir.join("fullchain.cer"), dir.join("private.key"), "nginxWebUI", false);
    }
    out
}

pub fn resolve_pair(db: &Db, name: &str) -> (PathBuf, PathBuf) {
    if let Some(pair) = cert_pairs(db, name).into_iter().next() {
        return (pair.cer, pair.key);
    }
    site::cert_paths(db, name)
}

pub fn discover_all_certs(db: &Db) -> Vec<CertPair> {
    let mut names: Vec<String> = Vec::new();
    let mut add_name = |raw: &str| {
        let n = raw.trim().trim_end_matches(".cer").trim_end_matches(".pem");
        let n = n.trim_end_matches("_ecc");
        if n.is_empty() || n.contains("..") {
            return;
        }
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    if let Ok(entries) = fs::read_dir(site::ssl_dir(db)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".cer") || name.ends_with(".pem") || e.path().is_dir() {
                add_name(&name);
            }
        }
    }
    for home in acme::acme_home_candidates(db) {
        if let Ok(entries) = fs::read_dir(&home) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    add_name(&e.file_name().to_string_lossy());
                }
            }
        }
    }
    let java_cert = PathBuf::from(db.setting("home")).join("cert");
    if let Ok(entries) = fs::read_dir(&java_cert) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                let raw = e.file_name().to_string_lossy().to_string();
                if raw.starts_with('_') && !raw.ends_with("_ecc") {
                    add_name(&format!("*.{}", &raw[1..]));
                } else {
                    add_name(&raw);
                }
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/etc/letsencrypt/live") {
        for e in entries.flatten() {
            if e.path().is_dir() {
                add_name(&e.file_name().to_string_lossy());
            }
        }
    }
    for (cer, _, _) in ssl_paths_in_nginx(db) {
        add_name(&cert_name_from_path(&cer));
    }
    let mut out = Vec::new();
    for name in names {
        if let Some(pair) = cert_pairs(db, &name).into_iter().next() {
            out.push(pair);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn ssl_paths_in_nginx(db: &Db) -> Vec<(PathBuf, PathBuf, String)> {
    let mut files: Vec<PathBuf> = Vec::new();
    files.push(nginx::nginx_path(db));
    for dir in [
        site::sites_available(db),
        site::sites_enabled(db),
        nginx::nginx_dir(db).join("conf.d"),
    ] {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("conf") {
                    files.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap_or_default();
        out.extend(parse_ssl_blocks(&text));
    }
    out
}

pub fn parse_ssl_blocks(text: &str) -> Vec<(PathBuf, PathBuf, String)> {
    let mut out = Vec::new();
    let mut cer = String::new();
    let mut key = String::new();
    let mut names = String::new();
    let flush = |cer: &str, key: &str, names: &str, out: &mut Vec<(PathBuf, PathBuf, String)>| {
        if cer.is_empty() || key.is_empty() {
            return;
        }
        let domain = names
            .split_whitespace()
            .find(|s| !s.starts_with('_') && *s != "_" && *s != "localhost")
            .unwrap_or("")
            .to_string();
        out.push((PathBuf::from(cer), PathBuf::from(key), domain));
    };
    for line in text.lines() {
        let line = line.trim().trim_end_matches(';');
        if line.starts_with("server {") {
            flush(&cer, &key, &names, &mut out);
            cer.clear();
            key.clear();
            names.clear();
        }
        if let Some(rest) = line.strip_prefix("ssl_certificate_key") {
            key = unquote(rest);
        } else if let Some(rest) = line.strip_prefix("ssl_certificate") {
            if !line.contains("ssl_certificate_key") && !line.contains("ssl_trusted") {
                cer = unquote(rest);
            }
        } else if let Some(rest) = line.strip_prefix("server_name") {
            names = rest.trim().to_string();
        }
    }
    flush(&cer, &key, &names, &mut out);
    out
}

pub fn parse_site_file(text: &str, domain_hint: &str) -> Value {
    let mut domain = domain_hint.to_string();
    let mut root = String::new();
    let mut proxy = String::new();
    let mut cer = String::new();
    let mut listen = "80".to_string();
    let mut ssl = false;
    for line in text.lines() {
        let line = line.trim().trim_end_matches(';');
        if let Some(rest) = line.strip_prefix("server_name") {
            if let Some(name) = rest.split_whitespace().next() {
                if domain.is_empty() || domain.starts_with("disk-") {
                    domain = name.to_string();
                }
            }
        }
        if line.starts_with("ssl_certificate") && !line.contains("ssl_certificate_key") {
            ssl = true;
            cer = unquote(line.strip_prefix("ssl_certificate").unwrap_or(""));
        }
        if let Some(rest) = line.strip_prefix("root ") {
            if root.is_empty() {
                root = rest.trim().to_string();
            }
        }
        if let Some(rest) = line.strip_prefix("proxy_pass ") {
            if proxy.is_empty() {
                proxy = rest.trim().to_string();
            }
        }
        if line.starts_with("listen ") && !line.contains("443") && listen == "80" {
            listen = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("80")
                .trim_matches(|c: char| !c.is_ascii_digit())
                .to_string();
            if listen.is_empty() {
                listen = "80".into();
            }
        }
    }
    let cert_name = cert_name_from_path(Path::new(&cer));
    let loc = if !proxy.is_empty() {
        json!([{ "path": "/", "type": "proxy", "proxy": proxy }])
    } else {
        json!([{ "path": "/", "type": "static", "root": root }])
    };
    json!({
        "domain": domain,
        "ssl": ssl,
        "http2": ssl,
        "redirect": ssl,
        "listen": listen,
        "certName": cert_name,
        "imported": true,
        "fromDisk": true,
        "locations": loc,
        "mode": if proxy.is_empty() { "static" } else { "proxy" },
    })
}

fn import_certs(db: &mut Db) -> usize {
    let discovered = discover_all_certs(db);
    let mut added = 0;
    for pair in discovered {
        let exists = db.rows("cert").iter().any(|r| {
            str_of(r, "domain") == pair.name
                || str_of(r, "certName") == pair.name
                || str_of(r, "pem") == pair.cer.display().to_string()
                || str_of(r, "cer") == pair.cer.display().to_string()
        });
        if exists {
            continue;
        }
        let kind = kind_from_acme_dir(&pair);
        let mut row = Map::new();
        row.insert("id".into(), new_id().into());
        row.insert("domain".into(), pair.name.clone().into());
        row.insert("certName".into(), pair.name.clone().into());
        row.insert("pem".into(), pair.cer.display().to_string().into());
        row.insert("cer".into(), pair.cer.display().to_string().into());
        row.insert("key".into(), pair.key.display().to_string().into());
        row.insert("kind".into(), kind.into());
        row.insert("source".into(), pair.source.clone().into());
        row.insert("encryption".into(), if pair.ecc { "ECC" } else { "RSA" }.into());
        row.insert("imported".into(), true.into());
        db.upsert("cert", Value::Object(row));
        added += 1;
    }
    added
}

fn kind_from_acme_dir(pair: &CertPair) -> String {
    if pair.source == "nginxWebUI" || pair.source == "certbot" {
        return "import".into();
    }
    let conf = pair
        .cer
        .parent()
        .map(|p| p.join(format!("{}.conf", pair.name)))
        .unwrap_or_default();
    let text = fs::read_to_string(conf).unwrap_or_default();
    if text.contains("dns_gcore") || text.contains("dns_gdcore") {
        "dns_gcore".into()
    } else if text.contains("dns_cf") {
        "dns_cf".into()
    } else if let Some(line) = text.lines().find(|l| l.contains("Le_Webroot")) {
        let v = line.split('=').nth(1).unwrap_or("").trim().trim_matches('\'');
        if v.starts_with("dns_") {
            v.to_string()
        } else if pair.source == "acme.sh" {
            "acme".into()
        } else {
            "import".into()
        }
    } else if pair.source == "acme.sh" {
        "acme".into()
    } else {
        "import".into()
    }
}

fn import_sites(db: &mut Db) -> usize {
    let mut added = 0;
    let mut seen: Vec<String> = db
        .rows("site")
        .iter()
        .map(|r| str_of(r, "domain"))
        .collect();
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [
        site::sites_available(db),
        site::sites_enabled(db),
        nginx::nginx_dir(db).join("conf.d"),
    ] {
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("conf") {
                    files.push(p);
                }
            }
        }
    }
    files.push(nginx::nginx_path(db));
    for path in files {
        let text = fs::read_to_string(&path).unwrap_or_default();
        if !text.contains("server_name") {
            continue;
        }
        for block in split_server_blocks(&text) {
            let hint = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut row = parse_site_file(block, &hint);
            let domain = str_of(&row, "domain");
            if domain.is_empty()
                || domain == "_"
                || domain == "localhost"
                || domain == "default"
                || seen.contains(&domain)
            {
                continue;
            }
            if let Value::Object(map) = &mut row {
                map.insert("id".into(), new_id().into());
                map.insert("confPath".into(), path.display().to_string().into());
                map.insert("rawConf".into(), block.to_string().into());
            }
            db.upsert("site", row);
            seen.push(domain);
            added += 1;
        }
    }
    added
}

pub fn split_server_blocks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while let Some(rel) = text[i..].find("server") {
        let start = i + rel;
        if start > 0 {
            let prev = bytes[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                i = start + 6;
                continue;
            }
        }
        let after = if start + 6 <= text.len() {
            text[start + 6..].trim_start()
        } else {
            ""
        };
        if !after.starts_with('{') {
            i = start + 6;
            continue;
        }
        let rest = &text[start..];
        let brace = match rest.find('{') {
            Some(b) => b,
            None => break,
        };
        let mut depth = 0;
        let mut end = brace;
        for (idx, ch) in rest[brace..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = brace + idx + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end > brace {
            out.push(&rest[..end]);
            i = start + end;
        } else {
            break;
        }
    }
    out
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

pub fn cert_name_from_path(cer: &Path) -> String {
    let stem = cer
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let generic = matches!(
        stem.as_str(),
        "fullchain" | "certificate" | "cert" | "fullchain.pem" | "cert.pem"
    );
    if generic {
        if let Some(parent) = cer.parent().and_then(|p| p.file_name()) {
            let name = parent.to_string_lossy();
            return name.trim_end_matches("_ecc").to_string();
        }
    }
    stem.trim_end_matches("_ecc").to_string()
}

pub fn normalize_cert_kind(row: &Value) -> String {
    let kind = str_of(row, "kind");
    if !kind.is_empty() {
        return match kind.as_str() {
            "0" => "acme".into(),
            "1" => "import".into(),
            "2" => dns_type_to_kind(&str_of(row, "dnsType")),
            other => other.to_string(),
        };
    }
    match str_of(row, "type").as_str() {
        "1" => "import".into(),
        "2" => dns_type_to_kind(&str_of(row, "dnsType")),
        "0" => "acme".into(),
        _ => {
            if !str_of(row, "dnsType").is_empty() {
                dns_type_to_kind(&str_of(row, "dnsType"))
            } else {
                "import".into()
            }
        }
    }
}

fn dns_type_to_kind(dns: &str) -> String {
    match dns {
        "cf" | "cfToken" | "dns_cf" => "dns_cf".into(),
        "gcore" | "dns_gcore" => "dns_gcore".into(),
        other if other.starts_with("dns_") => other.to_string(),
        other if !other.is_empty() => format!("dns_{other}"),
        _ => "dns_cf".into(),
    }
}

pub fn cert_file_owned(db: &Db, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = PathBuf::from(path);
    for home in acme::acme_home_candidates(db) {
        if p.starts_with(&home) {
            return false;
        }
    }
    p.components().all(|c| {
        let name = c.as_os_str();
        name != ".acme.sh" && name != "letsencrypt"
    })
}

pub fn existing_acme_cert(db: &Db, name: &str) -> Option<CertPair> {
    cert_pairs(db, name)
        .into_iter()
        .find(|p| p.source == "acme.sh")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssl_certificate_lines() {
        let text = r#"
server {
    listen 443 ssl;
    server_name app.example.com;
    ssl_certificate /home/nginxWebUI/.acme.sh/example.com/fullchain.cer;
    ssl_certificate_key /home/nginxWebUI/.acme.sh/example.com/example.com.key;
}
"#;
        let blocks = parse_ssl_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].2, "app.example.com");
        assert!(blocks[0]
            .0
            .to_string_lossy()
            .ends_with("fullchain.cer"));
    }

    #[test]
    fn maps_old_dns_types() {
        let row = json!({ "type": 2, "dnsType": "cfToken" });
        assert_eq!(normalize_cert_kind(&row), "dns_cf");
        let row = json!({ "kind": "1" });
        assert_eq!(normalize_cert_kind(&row), "import");
    }

    #[test]
    fn parses_proxy_site_file() {
        let text = r#"
server {
    listen 80;
    server_name api.example.com;
    location / {
        proxy_pass http://127.0.0.1:8080;
    }
}
"#;
        let site = parse_site_file(text, "api.example.com");
        assert_eq!(str_of(&site, "domain"), "api.example.com");
        assert_eq!(str_of(&site, "mode"), "proxy");
    }

    #[test]
    fn split_ignores_server_name_and_upstream() {
        let text = r#"
http {
    upstream app {
        server 127.0.0.1:8080;
        server 127.0.0.1:8081;
    }
    server {
        listen 80;
        server_name api.example.com;
        location / { proxy_pass http://app; }
    }
    server {
        listen 443 ssl;
        server_name www.example.com;
        ssl_certificate /home/nginxWebUI/.acme.sh/example.com/fullchain.cer;
        ssl_certificate_key /home/nginxWebUI/.acme.sh/example.com/example.com.key;
    }
}
"#;
        let blocks = split_server_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("api.example.com"));
        assert!(blocks[1].contains("www.example.com"));
        let site = parse_site_file(blocks[1], "disk-www");
        assert_eq!(str_of(&site, "certName"), "example.com");
    }

    #[test]
    fn cert_name_from_acme_fullchain() {
        let path = PathBuf::from("/home/nginxWebUI/.acme.sh/cdn.example.com_ecc/fullchain.cer");
        assert_eq!(cert_name_from_path(&path), "cdn.example.com");
        let path = PathBuf::from("/etc/nginx/ssl/app.example.com.cer");
        assert_eq!(cert_name_from_path(&path), "app.example.com");
    }
}
