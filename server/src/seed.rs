//! First-start defaults shared by the panel and the outbound agent.
//!
//! Both processes detect the Debian/system nginx layout the same way so a
//! remote node writes to `/etc/nginx` like the panel host does, instead of
//! falling back to a private `nginx.conf` in the agent's home directory.

use std::path::{Path, PathBuf};

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

/// Write the small standard nginx files the first time, like the Java version did
/// when it unpacked conf.zip.
pub fn write_default_files(home: &Path) -> std::io::Result<()> {
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

/// Seed settings with the system layout under `/` (or a Windows prefix).
pub fn seed(db: &mut crate::store::Db, home: &Path, reload_cmd: Option<&str>) {
    seed_with_root(db, home, reload_cmd, Path::new("/"));
}

/// Is there a system nginx (`/etc/nginx/nginx.conf`)?
pub fn has_system_nginx() -> bool {
    cfg!(unix) && Path::new("/etc/nginx/nginx.conf").is_file()
}

/// Layout used when seeding nginx paths. Tests can inject a fake Windows tree
/// on Linux by putting `nginx.exe` and `conf/nginx.conf` under `root`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Unix,
    Windows,
}

pub fn is_windows_node(db: &crate::store::Db) -> bool {
    let fam = db.setting("osFamily");
    if fam.eq_ignore_ascii_case("windows") {
        return true;
    }
    if fam.eq_ignore_ascii_case("unix")
        || fam.eq_ignore_ascii_case("linux")
        || fam.eq_ignore_ascii_case("darwin")
        || fam.eq_ignore_ascii_case("macos")
    {
        return false;
    }
    cfg!(windows)
}

/// A Windows-style nginx prefix under `root` (for tests and real installs).
pub fn windows_prefix(root: &Path) -> Option<std::path::PathBuf> {
    let candidates = [
        root.to_path_buf(),
        root.join("nginx"),
        root.join("Program Files").join("nginx"),
        root.join("Program Files (x86)").join("nginx"),
        root.join("tools").join("nginx"),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("nginx.exe").is_file() || p.join("conf").join("nginx.conf").is_file())
}

pub fn discover_windows_prefix() -> Option<std::path::PathBuf> {
    let mut cands: Vec<std::path::PathBuf> = Vec::new();
    if let Some(exe) = crate::install::which("nginx.exe").or_else(|| crate::install::which("nginx"))
    {
        if let Some(dir) = exe.parent() {
            cands.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                cands.push(parent.to_path_buf());
            }
        }
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        cands.push(Path::new(&pf).join("nginx"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles(x86)") {
        cands.push(Path::new(&pf).join("nginx"));
    }
    cands.push(PathBuf::from(
        r"C:
ginx",
    ));
    cands.push(PathBuf::from(
        r"C:	ools
ginx",
    ));
    if let Ok(cd) = std::env::var("ChocolateyInstall") {
        cands.push(Path::new(&cd).join("bin"));
        cands.push(Path::new(&cd).join("lib").join("nginx"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        cands.push(
            Path::new(&home)
                .join("scoop")
                .join("apps")
                .join("nginx")
                .join("current"),
        );
    }
    for p in cands {
        if p.join("nginx.exe").is_file() || p.join("conf").join("nginx.conf").is_file() {
            return Some(p);
        }
        if p.is_file() && p.file_name().is_some_and(|n| n == "nginx.exe") {
            return p.parent().map(|d| d.to_path_buf());
        }
    }
    None
}

pub fn detect_layout(root: &Path) -> Layout {
    if windows_prefix(root).is_some() {
        return Layout::Windows;
    }
    if cfg!(windows) {
        Layout::Windows
    } else {
        Layout::Unix
    }
}

fn seed_windows(db: &mut crate::store::Db, home: &Path, reload_cmd: Option<&str>, root: &Path) {
    db.set_setting("osFamily", "windows");
    let prefix = windows_prefix(root).or_else(discover_windows_prefix);
    let fallback = |name: &str| home.join(name).display().to_string();
    if let Some(prefix) = prefix {
        let conf = if prefix.join("conf").join("nginx.conf").is_file() {
            prefix.join("conf").join("nginx.conf")
        } else if prefix.join("nginx.conf").is_file() {
            prefix.join("nginx.conf")
        } else {
            prefix.join("conf").join("nginx.conf")
        };
        let conf_dir = conf
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| prefix.join("conf"));
        let prefix_dir = if conf_dir.file_name().is_some_and(|n| n == "conf") {
            conf_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(conf_dir.clone())
        } else {
            prefix.clone()
        };
        if db.setting("nginxPath").is_empty() {
            db.set_setting("nginxPath", &conf.display().to_string());
        }
        if db.setting("nginxDir").is_empty() {
            db.set_setting("nginxDir", &prefix_dir.display().to_string());
        }
        if db.setting("nginxExe").is_empty() {
            let exe = prefix_dir.join("nginx.exe");
            if exe.is_file() {
                db.set_setting("nginxExe", &exe.display().to_string());
            } else {
                db.set_setting("nginxExe", "nginx.exe");
            }
        }
        for (key, path) in [
            ("sitesAvailable", conf_dir.join("sites-available")),
            ("sitesEnabled", conf_dir.join("sites-enabled")),
            ("sslDir", conf_dir.join("ssl")),
            ("sslParams", conf_dir.join("ssl-params.conf")),
            ("webRoot", prefix_dir.join("html")),
        ] {
            if db.setting(key).is_empty() {
                db.set_setting(key, &path.display().to_string());
            }
        }
        if db.setting("reloadCmd").is_empty() {
            let exe = db.setting("nginxExe");
            let cmd = reload_cmd
                .and_then(crate::nginx::normalize_reload_cmd)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("{exe} -s reload"));
            let cmd = crate::nginx::normalize_reload_cmd(&cmd).unwrap_or(cmd);
            db.set_setting("reloadCmd", &cmd);
        }
    } else {
        if db.setting("nginxPath").is_empty() {
            db.set_setting("nginxPath", &home.join("nginx.conf").display().to_string());
        }
        if db.setting("nginxDir").is_empty() {
            db.set_setting("nginxDir", &home.display().to_string());
        }
        if db.setting("nginxExe").is_empty() {
            db.set_setting("nginxExe", "nginx.exe");
        }
        for (key, name) in [
            ("sitesAvailable", "sites-available"),
            ("sitesEnabled", "sites-enabled"),
            ("sslDir", "ssl"),
            ("sslParams", "ssl-params.conf"),
            ("webRoot", "html"),
        ] {
            if db.setting(key).is_empty() {
                db.set_setting(key, &fallback(name));
            }
        }
        if db.setting("reloadCmd").is_empty() {
            let cmd = reload_cmd
                .and_then(crate::nginx::normalize_reload_cmd)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "nginx.exe -s reload".into());
            db.set_setting("reloadCmd", &cmd);
        }
    }
}

/// Same as [`seed`], but detects the Debian or Windows layout below `root`
/// (tests use a temporary directory). Values that are already set are never
/// overwritten.
pub fn seed_with_root(
    db: &mut crate::store::Db,
    home: &Path,
    reload_cmd: Option<&str>,
    root: &Path,
) {
    seed_with_layout(db, home, reload_cmd, root, detect_layout(root));
}

pub fn seed_with_layout(
    db: &mut crate::store::Db,
    home: &Path,
    reload_cmd: Option<&str>,
    root: &Path,
    layout: Layout,
) {
    db.set_setting("home", &home.display().to_string());
    if db.setting("lang").is_empty() {
        db.set_setting("lang", "zh");
    }
    if layout == Layout::Windows {
        seed_windows(db, home, reload_cmd, root);
        seed_common(db, home);
        return;
    }
    if db.setting("osFamily").is_empty() {
        db.set_setting("osFamily", "unix");
    }
    let sys = |rel: &str| root.join(rel);
    let is_file = |rel: &str| sys(rel).is_file();
    let is_dir = |rel: &str| sys(rel).is_dir();
    let show = |rel: &str| sys(rel).display().to_string();
    let system_nginx = is_file("etc/nginx/nginx.conf");
    if db.setting("nginxPath").is_empty() {
        if system_nginx {
            db.set_setting("nginxPath", &show("etc/nginx/nginx.conf"));
        } else {
            db.set_setting("nginxPath", &home.join("nginx.conf").display().to_string());
        }
    }
    if db.setting("nginxDir").is_empty() {
        if system_nginx {
            db.set_setting("nginxDir", &show("etc/nginx"));
        } else {
            db.set_setting("nginxDir", &home.display().to_string());
        }
    }
    let fallback = |name: &str| home.join(name).display().to_string();
    for (key, rel, name) in [
        (
            "sitesAvailable",
            "etc/nginx/sites-available",
            "sites-available",
        ),
        ("sitesEnabled", "etc/nginx/sites-enabled", "sites-enabled"),
        ("sslDir", "etc/nginx/ssl", "ssl"),
    ] {
        if db.setting(key).is_empty() {
            let value = if is_dir(rel) {
                show(rel)
            } else {
                fallback(name)
            };
            db.set_setting(key, &value);
        }
    }
    if db.setting("sslParams").is_empty() {
        let value = if is_file("etc/nginx/ssl-params.conf") || is_dir("etc/nginx") {
            show("etc/nginx/ssl-params.conf")
        } else {
            fallback("ssl-params.conf")
        };
        db.set_setting("sslParams", &value);
    }
    if db.setting("webRoot").is_empty() {
        let value = if is_dir("var/www") {
            show("var/www")
        } else {
            fallback("www")
        };
        db.set_setting("webRoot", &value);
    }
    if db.setting("reloadCmd").is_empty() {
        let cmd = reload_cmd
            .and_then(crate::nginx::normalize_reload_cmd)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                if sys(".dockerenv").exists() {
                    "nginx -s reload".into()
                } else {
                    "sudo -n systemctl reload nginx".into()
                }
            });
        db.set_setting("reloadCmd", &cmd);
    } else if db.setting("reloadCmd") == "sudo systemctl reload nginx" {
        db.set_setting("reloadCmd", "sudo -n systemctl reload nginx");
    }
    seed_common(db, home);
}

fn seed_common(db: &mut crate::store::Db, home: &Path) {
    let _ = home;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    fn temp(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-seed-{label}-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn agent_seed_uses_debian_layout_when_present() {
        let root = temp("root");
        for d in [
            "etc/nginx/sites-available",
            "etc/nginx/sites-enabled",
            "etc/nginx/ssl",
            "var/www",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("etc/nginx/nginx.conf"), "events {}\nhttp {}\n").unwrap();
        let home = temp("home");
        let mut db = Db::new();
        seed_with_root(&mut db, &home, None, &root);
        assert_eq!(
            db.setting("nginxPath"),
            root.join("etc/nginx/nginx.conf").display().to_string()
        );
        assert_eq!(
            db.setting("nginxDir"),
            root.join("etc/nginx").display().to_string()
        );
        assert_eq!(
            db.setting("sitesAvailable"),
            root.join("etc/nginx/sites-available").display().to_string()
        );
        assert_eq!(
            db.setting("sslDir"),
            root.join("etc/nginx/ssl").display().to_string()
        );
        assert_eq!(
            db.setting("webRoot"),
            root.join("var/www").display().to_string()
        );
        assert_eq!(db.setting("reloadCmd"), "sudo -n systemctl reload nginx");
        assert_eq!(db.setting("autoInstall"), "false");
        // nginx_path must not fall back to the agent home any more
        assert!(!crate::nginx::nginx_path(&db).starts_with(&home));
    }

    #[test]
    fn seed_uses_windows_prefix_when_present() {
        let root = temp("winroot");
        let prefix = root.join("nginx");
        for d in [
            "conf/sites-available",
            "conf/sites-enabled",
            "conf/ssl",
            "html",
            "logs",
        ] {
            std::fs::create_dir_all(prefix.join(d)).unwrap();
        }
        std::fs::write(prefix.join("nginx.exe"), b"").unwrap();
        std::fs::write(prefix.join("conf/nginx.conf"), "events {}\nhttp {}\n").unwrap();
        let home = temp("winhome");
        let mut db = Db::new();
        seed_with_root(&mut db, &home, None, &root);
        assert_eq!(db.setting("osFamily"), "windows");
        assert_eq!(
            db.setting("nginxPath"),
            prefix.join("conf").join("nginx.conf").display().to_string()
        );
        assert_eq!(db.setting("nginxDir"), prefix.display().to_string());
        assert_eq!(
            db.setting("sslDir"),
            prefix.join("conf").join("ssl").display().to_string()
        );
        assert_eq!(
            db.setting("sitesAvailable"),
            prefix
                .join("conf")
                .join("sites-available")
                .display()
                .to_string()
        );
        assert_eq!(
            db.setting("webRoot"),
            prefix.join("html").display().to_string()
        );
        assert!(
            db.setting("reloadCmd").contains("nginx"),
            "{}",
            db.setting("reloadCmd")
        );
        assert!(crate::seed::is_windows_node(&db));
        // path_or must not invent Unix paths when osFamily is windows
        assert!(
            !crate::site::ssl_dir(&db)
                .display()
                .to_string()
                .replace('\\', "/")
                .contains("/etc/nginx"),
            "{:?}",
            crate::site::ssl_dir(&db)
        );
        assert!(
            !crate::site::web_root_base(&db)
                .display()
                .to_string()
                .replace('\\', "/")
                .contains("/var/www"),
            "{:?}",
            crate::site::web_root_base(&db)
        );
    }

    #[test]
    fn seed_falls_back_to_home_and_keeps_existing_values() {
        let root = temp("empty-root");
        let home = temp("home2");
        let mut db = Db::new();
        db.set_setting("reloadCmd", "nginx -s reload");
        seed_with_root(&mut db, &home, Some("curl http://evil"), &root);
        assert_eq!(
            db.setting("nginxPath"),
            home.join("nginx.conf").display().to_string()
        );
        assert_eq!(db.setting("reloadCmd"), "nginx -s reload");
        let mut db2 = Db::new();
        seed_with_root(&mut db2, &home, Some("curl http://evil"), &root);
        // non-whitelisted reload commands are ignored
        assert_eq!(db2.setting("reloadCmd"), "sudo -n systemctl reload nginx");
    }
}
