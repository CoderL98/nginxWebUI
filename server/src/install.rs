//! Install nginx and acme.sh when they are missing.
//! Linux uses the distro package manager and the official acme.sh installer.
//! Windows reports that those tools must be installed by hand.

use crate::acme;
use crate::nginx;
use crate::site;
use crate::store::Db;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const SSL_PARAMS: &str = r#"ssl_protocols TLSv1.2 TLSv1.3;
ssl_prefer_server_ciphers off;
ssl_session_timeout 1d;
ssl_session_cache shared:SSL:10m;
ssl_session_tickets off;
"#;

pub fn auto_install_enabled(db: &Db) -> bool {
    let v = db.setting("autoInstall");
    if v.is_empty() {
        true
    } else {
        v != "false" && v != "0"
    }
}

pub fn can_install() -> bool {
    cfg!(unix)
}

pub fn should_ensure(db: &Db) -> bool {
    can_install() && auto_install_enabled(db)
}

pub fn nginx_present(db: &Db) -> bool {
    let exe = nginx::nginx_exe(db);
    let path = Path::new(&exe);
    if path.is_file() {
        return true;
    }
    which(&exe).is_some() || command_ok(&exe, &["-v"])
}

pub fn acme_present(db: &Db) -> bool {
    let path = acme::resolve_bin(db);
    path.as_os_str().len() > 0 && path.is_file()
}

pub fn status(db: &Db) -> Value {
    let nginx_ok = nginx_present(db);
    json!({
        "os": std::env::consts::OS,
        "autoInstall": auto_install_enabled(db),
        "linux": cfg!(unix),
        "nginx": {
            "found": nginx_ok,
            "exe": nginx::nginx_exe(db),
            "path": nginx::nginx_path(db).display().to_string(),
        },
        "acme": acme::summarize(db),
        "canInstall": can_install(),
    })
}

pub fn ensure_nginx(db: &mut Db) -> Result<String, String> {
    if nginx_present(db) {
        layout(db)?;
        return Ok("nginx already installed".into());
    }
    if !auto_install_enabled(db) {
        return Err("nginxNotFound".into());
    }
    install_nginx(db)
}

pub fn ensure_acme(db: &mut Db) -> Result<String, String> {
    if acme_present(db) {
        return Ok("acme.sh already installed".into());
    }
    if !auto_install_enabled(db) {
        return Err("acmeNotFound".into());
    }
    install_acme(db)
}

pub fn install_nginx(db: &mut Db) -> Result<String, String> {
    if !cfg!(unix) {
        return Err("installLinuxOnly".into());
    }
    if nginx_present(db) {
        layout(db)?;
        return Ok("nginx already installed".into());
    }

    let mut log = String::new();
    let (ok, out) = pkg_install("nginx");
    log.push_str(&out);
    if !ok {
        return Err(format!("nginx install failed\n{}", log));
    }

    if let Some(exe) = which("nginx").or_else(|| which("/usr/sbin/nginx")) {
        db.set_setting("nginxExe", &exe.display().to_string());
    } else {
        db.set_setting("nginxExe", "nginx");
    }
    if nginx::unix_is_file("/etc/nginx/nginx.conf") {
        db.set_setting("nginxPath", "/etc/nginx/nginx.conf");
        db.set_setting("nginxDir", "/etc/nginx");
    }
    db.set_setting("reloadCmd", "sudo -n systemctl reload nginx");
    layout(db)?;

    let _ = run_shell("systemctl enable nginx");
    let (start_ok, start_out) = run_shell("systemctl start nginx");
    log.push('\n');
    log.push_str(&start_out);
    if !start_ok {
        let (alt_ok, alt_out) = run_cmd("nginx", &["-v"]);
        log.push('\n');
        log.push_str(&alt_out);
        if !alt_ok && !nginx_present(db) {
            return Err(format!("nginx installed but not runnable\n{}", log));
        }
    }
    Ok(log)
}

pub fn install_acme(db: &mut Db) -> Result<String, String> {
    if !cfg!(unix) {
        return Err("installLinuxOnly".into());
    }
    if acme_present(db) {
        return Ok("acme.sh already installed".into());
    }

    let curl = which("curl");
    let wget = which("wget");
    if curl.is_none() && wget.is_none() {
        let (ok, out) = pkg_install("curl");
        if !ok {
            return Err(format!("need curl to install acme.sh\n{}", out));
        }
    }

    let email = {
        let v = db.setting("acmeEmail");
        if v.trim().is_empty() {
            "nginxwebui@localhost".to_string()
        } else {
            v.trim().to_string()
        }
    };
    let home = acme_home_target(db);
    let _ = std::fs::create_dir_all(&home);

    let script = format!(
        "curl -fsSL https://get.acme.sh | sh -s -- --home {} -m {}",
        sh_single(&home.display().to_string()),
        sh_single(&email)
    );
    let (ok, out) = run_user_shell(&script);
    let mut log = out;
    if !ok {
        let fallback = format!(
            "wget -O - https://get.acme.sh | sh -s -- --home {} -m {}",
            sh_single(&home.display().to_string()),
            sh_single(&email)
        );
        let (ok2, out2) = run_user_shell(&fallback);
        log.push('\n');
        log.push_str(&out2);
        if !ok2 {
            return Err(format!("acme.sh install failed\n{}", log));
        }
    }

    let bin = home.join("acme.sh");
    if bin.is_file() {
        db.set_setting("acmeSh", &bin.display().to_string());
        db.set_setting("acmeHome", &home.display().to_string());
    } else {
        let found = acme::resolve_bin(db);
        if found.is_file() {
            db.set_setting("acmeSh", &found.display().to_string());
        } else {
            return Err(format!("acme.sh installed but binary missing\n{}", log));
        }
    }
    Ok(log)
}

fn acme_home_target(db: &Db) -> PathBuf {
    let setting = db.setting("acmeHome");
    if !setting.trim().is_empty() {
        return PathBuf::from(setting.trim());
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".acme.sh");
    }
    PathBuf::from("/root/.acme.sh")
}

fn layout(db: &mut Db) -> Result<String, String> {
    let available = site::sites_available(db);
    let enabled = site::sites_enabled(db);
    let ssl = site::ssl_dir(db);
    let www = site::web_root_base(db);
    crate::privfs::mkdir_all(&available)?;
    crate::privfs::mkdir_all(&enabled)?;
    crate::privfs::mkdir_all(&ssl)?;
    crate::privfs::mkdir_www(&www)?;

    let params = site::ssl_params(db);
    if !params.exists() {
        crate::privfs::write(&params, SSL_PARAMS)?;
    }

    if db.setting("sitesAvailable").is_empty() && nginx::unix_is_dir("/etc/nginx/sites-available") {
        db.set_setting("sitesAvailable", "/etc/nginx/sites-available");
        db.set_setting("sitesEnabled", "/etc/nginx/sites-enabled");
    }
    if db.setting("sslDir").is_empty() && nginx::unix_is_dir("/etc/nginx/ssl") {
        db.set_setting("sslDir", "/etc/nginx/ssl");
    }
    if db.setting("webRoot").is_empty() && nginx::unix_is_dir("/var/www") {
        db.set_setting("webRoot", "/var/www");
    }

    let log_dir = site::access_log_dir(db);
    let _ = crate::privfs::mkdir_all(&log_dir);
    let _ = site::ensure_main_includes_sites(db);
    Ok("layout ready".into())
}

fn pkg_install(pkg: &str) -> (bool, String) {
    if which("apt-get").is_some() {
        let env = [("DEBIAN_FRONTEND", "noninteractive")];
        let _ = maybe_sudo_env("apt-get", &["update", "-y"], &env);
        return maybe_sudo_env("apt-get", &["install", "-y", pkg], &env);
    }
    if which("dnf").is_some() {
        return maybe_sudo("dnf", &["install", "-y", pkg]);
    }
    if which("yum").is_some() {
        return maybe_sudo("yum", &["install", "-y", pkg]);
    }
    if which("pacman").is_some() {
        return maybe_sudo("pacman", &["-Sy", "--noconfirm", pkg]);
    }
    if which("apk").is_some() {
        return maybe_sudo("apk", &["add", "--no-cache", pkg]);
    }
    (false, "no package manager (apt/dnf/yum/pacman/apk)".into())
}

fn maybe_sudo(program: &str, args: &[&str]) -> (bool, String) {
    maybe_sudo_env(program, args, &[])
}

fn maybe_sudo_env(program: &str, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    if running_as_root() {
        return run_env_cmd(program, args, env);
    }
    if which("sudo").is_some() {
        if env.is_empty() {
            let mut full = vec![program];
            full.extend_from_slice(args);
            return run_env_cmd("sudo", &full, &[]);
        }
        let mut full: Vec<String> = vec!["env".into()];
        for (k, v) in env {
            full.push(format!("{k}={v}"));
        }
        full.push(program.to_string());
        for a in args {
            full.push((*a).to_string());
        }
        let refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
        return run_env_cmd("sudo", &refs, &[]);
    }
    run_env_cmd(program, args, env)
}

fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        return unsafe { libc_geteuid() == 0 };
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

fn command_ok(program: &str, args: &[&str]) -> bool {
    run_cmd(program, args).0
}

fn run_cmd(program: &str, args: &[&str]) -> (bool, String) {
    run_env_cmd(program, args, &[])
}

fn run_env_cmd(program: &str, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let mut command = Command::new(program);
    command.args(args);
    command.stdin(Stdio::null());
    for (k, v) in env {
        command.env(k, v);
    }
    match command.output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.success(), text)
        }
        Err(err) => (false, format!("{}: {}", program, err)),
    }
}

fn run_user_shell(script: &str) -> (bool, String) {
    #[cfg(unix)]
    {
        run_env_cmd("sh", &["-c", script], &[])
    }
    #[cfg(not(unix))]
    {
        let _ = script;
        (false, "installLinuxOnly".into())
    }
}

fn run_shell(script: &str) -> (bool, String) {
    #[cfg(unix)]
    {
        if running_as_root() || which("sudo").is_none() {
            return run_env_cmd("sh", &["-c", script], &[]);
        }
        return run_env_cmd("sudo", &["sh", "-c", script], &[]);
    }
    #[cfg(not(unix))]
    {
        let _ = script;
        (false, "installLinuxOnly".into())
    }
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{}.exe", name));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn sh_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_shell() {
        assert_eq!(sh_single("/root/.acme.sh"), "'/root/.acme.sh'");
        assert_eq!(sh_single("a'b"), "'a'\"'\"'b'");
    }
}
