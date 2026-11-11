//! Install nginx and acme.sh when they are missing.
//! Linux uses the distro package manager and a *pinned* acme.sh GitHub
//! archive (see `ACME_SH_*` and `script/acme.sh.pin`). On Windows the agent
//! discovers nginx and installs the same pinned acme.sh through Git Bash or WSL.

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

/// Pinned acme.sh release. Keep in sync with `script/acme.sh.pin`.
pub const ACME_SH_VERSION: &str = "3.1.6";
pub const ACME_SH_URL: &str =
    "https://github.com/acmesh-official/acme.sh/archive/refs/tags/3.1.6.tar.gz";
pub const ACME_SH_SHA256: &str = "0d3f9000ac44a6331314742a88c475f79134e24fc991997883652adc59efc486";

pub fn auto_install_enabled(db: &Db) -> bool {
    let v = db.setting("autoInstall");
    if v.is_empty() {
        false
    } else {
        v != "false" && v != "0"
    }
}

pub fn can_install() -> bool {
    cfg!(unix) || cfg!(windows)
}

pub fn can_install_acme() -> bool {
    cfg!(unix) || crate::acme::find_bash().is_some()
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
    !path.as_os_str().is_empty() && path.is_file()
}

pub fn status(db: &Db) -> Value {
    let nginx_ok = nginx_present(db);
    json!({
        "os": std::env::consts::OS,
        "autoInstall": auto_install_enabled(db),
        "linux": cfg!(unix),
        "windows": cfg!(windows),
        "nginx": {
            "found": nginx_ok,
            "exe": nginx::nginx_exe(db),
            "path": nginx::nginx_path(db).display().to_string(),
        },
        "acme": acme::summarize(db),
        "canInstall": can_install(),
        "canInstallAcme": can_install_acme(),
        "acmeNeedsBash": cfg!(windows) && crate::acme::find_bash().is_none(),
        "acmeShVersion": ACME_SH_VERSION,
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
    if cfg!(windows) {
        return install_nginx_windows(db);
    }
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

    let _ = crate::privfs::run_helper("systemctl", &["enable"]);
    let (start_ok, start_out) = match crate::privfs::run_helper("systemctl", &["start"]) {
        Ok(text) => (true, text),
        Err(err) => (false, err),
    };
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

fn install_nginx_windows(db: &mut Db) -> Result<String, String> {
    if nginx_present(db) {
        layout(db)?;
        return Ok("nginx already installed".into());
    }
    if let Some(prefix) = crate::seed::discover_windows_prefix() {
        let home = PathBuf::from(db.setting("home"));
        crate::seed::seed_with_layout(
            db,
            &home,
            None,
            prefix.parent().unwrap_or(&prefix),
            crate::seed::Layout::Windows,
        );
        // force paths from the discovered prefix even if earlier defaults exist
        if prefix.join("nginx.exe").is_file() {
            db.set_setting("nginxExe", &prefix.join("nginx.exe").display().to_string());
        }
        if prefix.join("conf").join("nginx.conf").is_file() {
            db.set_setting(
                "nginxPath",
                &prefix.join("conf").join("nginx.conf").display().to_string(),
            );
            db.set_setting("nginxDir", &prefix.display().to_string());
        }
        layout(db)?;
        if nginx_present(db) {
            return Ok(format!("nginx discovered at {}", prefix.display()));
        }
    }
    Err("nginxWindowsNotFound".into())
}

pub fn install_acme(db: &mut Db) -> Result<String, String> {
    if cfg!(windows) {
        if crate::acme::find_bash().is_none() {
            return Err("acmeNeedsBash".into());
        }
    } else if !cfg!(unix) {
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

    let (ok, out) = install_acme_from_pin(&home, &email);
    let log = out;
    if !ok {
        return Err(format!("acme.sh install failed\n{}", log));
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
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home).join(".acme.sh");
    }
    let project = db.setting("home");
    if !project.trim().is_empty() {
        return PathBuf::from(project).join(".acme.sh");
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
    match site::ensure_main_includes_sites(db) {
        Ok(site::SitesIncludeOutcome::NeedsConfirm { include_line, .. }) => {
            crate::applog::warn(format!(
                "nginx.conf has no active http block (only a commented one); not inserting `{include_line}` automatically"
            ));
        }
        Err(err) => crate::applog::warn(format!("ensure sites-enabled include: {err}")),
        _ => {}
    }
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
            let mut full = vec!["-n", program];
            full.extend_from_slice(args);
            return run_env_cmd("sudo", &full, &[]);
        }
        let mut full: Vec<String> = vec!["-n".into(), "env".into()];
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
        libc_geteuid() == 0
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

/// Download the pinned acme.sh tag, verify SHA-256, extract, and run
/// `./acme.sh --install` from that tree. Never pipes a moving URL to sh.
pub fn install_acme_from_pin(home: &Path, email: &str) -> (bool, String) {
    let work = std::env::temp_dir().join(format!(
        "nginxwebui-acme-{}-{}",
        std::process::id(),
        crate::store::new_id()
    ));
    if let Err(err) = std::fs::create_dir_all(&work) {
        return (false, format!("cannot create temp dir: {err}"));
    }
    let archive = work.join("acme.sh.tar.gz");
    let (fetched, fetch_log) = download_file(ACME_SH_URL, &archive);
    if !fetched {
        let _ = std::fs::remove_dir_all(&work);
        return (false, fetch_log);
    }
    let bytes = match std::fs::read(&archive) {
        Ok(b) => b,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&work);
            return (false, format!("cannot read archive: {err}"));
        }
    };
    if let Err(err) = verify_sha256(&bytes, ACME_SH_SHA256) {
        let _ = std::fs::remove_dir_all(&work);
        return (false, format!("acme.sh archive hash mismatch: {err}"));
    }
    let (extracted, extract_log) = extract_tar_gz(&archive, &work);
    if !extracted {
        let _ = std::fs::remove_dir_all(&work);
        return (false, format!("cannot extract acme.sh\n{extract_log}"));
    }
    let script = work
        .join(format!("acme.sh-{ACME_SH_VERSION}"))
        .join("acme.sh");
    if !script.is_file() {
        let _ = std::fs::remove_dir_all(&work);
        return (
            false,
            format!("pinned archive did not contain {}", script.display()),
        );
    }
    let _ = std::fs::create_dir_all(home);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
    }
    let (ok, out) = run_acme_script(
        &script,
        &[
            "--install",
            "--home",
            &home.display().to_string(),
            "-m",
            email,
        ],
    );
    let mut log = fetch_log;
    log.push('\n');
    log.push_str(&extract_log);
    log.push('\n');
    log.push_str(&out);
    log.push_str(&format!("\ninstalled acme.sh {ACME_SH_VERSION}\n"));
    let _ = std::fs::remove_dir_all(&work);
    (ok, log)
}

fn extract_tar_gz(archive: &Path, work: &Path) -> (bool, String) {
    let archive_s = archive.display().to_string();
    let work_s = work.display().to_string();
    if which("tar").is_some() || which("tar.exe").is_some() {
        let bin = if which("tar").is_some() {
            "tar"
        } else {
            "tar.exe"
        };
        return run_cmd(bin, &["-xzf", &archive_s, "-C", &work_s]);
    }
    if let Some(bash) = crate::acme::find_bash() {
        let cmd = format!("tar -xzf '{}' -C '{}'", archive.display(), work.display());
        return run_cmd(&bash.display().to_string(), &["-lc", &cmd]);
    }
    (false, "tarNotFound".into())
}

fn run_acme_script(script: &Path, args: &[&str]) -> (bool, String) {
    if cfg!(windows) {
        let Some(bash) = crate::acme::find_bash() else {
            return (false, "acmeNeedsBash".into());
        };
        let name = bash.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.eq_ignore_ascii_case("wsl.exe") {
            let mut full = vec![
                "-e".to_string(),
                "bash".into(),
                crate::acme::to_wsl_path(script),
            ];
            full.extend(args.iter().map(|a| a.to_string()));
            let refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
            return run_cmd(&bash.display().to_string(), &refs);
        }
        let mut full = vec![script.display().to_string()];
        full.extend(args.iter().map(|a| a.to_string()));
        let refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
        return run_cmd(&bash.display().to_string(), &refs);
    }
    run_cmd(&script.display().to_string(), args)
}

pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let got = Sha256::digest(bytes);
    let want = parse_hex32(expected_hex)?;
    if got.as_slice() != want.as_slice() {
        return Err(format!(
            "expected {expected_hex}, got {}",
            crate::tls::fingerprint_hex(&got)
        ));
    }
    Ok(())
}

fn parse_hex32(hex: &str) -> Result<[u8; 32], String> {
    crate::tls::parse_fingerprint(hex)
}

fn download_file(url: &str, dest: &Path) -> (bool, String) {
    if let Some(curl) = which("curl") {
        return run_cmd(
            &curl.display().to_string(),
            &["-fsSL", "-o", &dest.display().to_string(), url],
        );
    }
    if let Some(wget) = which("wget") {
        return run_cmd(
            &wget.display().to_string(),
            &["-q", "-O", &dest.display().to_string(), url],
        );
    }
    (
        false,
        "need curl or wget to download the pinned acme.sh archive".into(),
    )
}

#[cfg(test)]
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

    #[test]
    fn auto_install_defaults_to_false() {
        let db = crate::store::Db::new();
        assert!(!auto_install_enabled(&db));
        let mut db = crate::store::Db::new();
        db.set_setting("autoInstall", "true");
        assert!(auto_install_enabled(&db));
        db.set_setting("autoInstall", "false");
        assert!(!auto_install_enabled(&db));
    }

    #[test]
    fn rejects_acme_archive_on_hash_mismatch() {
        let err = verify_sha256(b"not-the-archive", ACME_SH_SHA256).unwrap_err();
        assert!(err.contains("expected"), "{err}");
    }

    #[test]
    fn accepts_matching_sha256() {
        use sha2::{Digest, Sha256};
        let data = b"nginxwebui-acme-pin-test";
        let hex = crate::tls::fingerprint_hex(&Sha256::digest(data));
        verify_sha256(data, &hex).unwrap();
    }

    #[test]
    fn pin_constants_match_script_file() {
        let text = include_str!("../../script/acme.sh.pin");
        assert!(text.contains(ACME_SH_VERSION), "version");
        assert!(text.contains(ACME_SH_SHA256), "sha256");
        assert!(text.contains(ACME_SH_URL), "url");
        assert_eq!(ACME_SH_SHA256.len(), 64);
    }
}
