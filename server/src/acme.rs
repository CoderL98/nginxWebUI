//! Call the local acme.sh the same way `script/issue-cert.sh` and
//! `script/issue-gcore-cert.sh` do: DNS-01, Let's Encrypt, install into
//! `/etc/nginx/ssl/<name>.cer` + `.key`, then reload nginx.

use crate::site;
use crate::store::{bool_of, now_ms, str_of, Db};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

const JOB_LOG_MAX: usize = 512 * 1024;

#[derive(Clone, Debug)]
pub struct Job {
    pub running: bool,
    pub kind: String,
    pub cert_id: String,
    pub log: String,
    pub success: Option<bool>,
    pub msg: String,
}

impl Job {
    fn idle() -> Job {
        Job {
            running: false,
            kind: String::new(),
            cert_id: String::new(),
            log: String::new(),
            success: None,
            msg: String::new(),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "running": self.running,
            "kind": self.kind,
            "certId": self.cert_id,
            "output": self.log,
            "success": self.success,
            "msg": self.msg,
        })
    }
}

fn job_slot() -> &'static Mutex<Job> {
    static JOB: OnceLock<Mutex<Job>> = OnceLock::new();
    JOB.get_or_init(|| Mutex::new(Job::idle()))
}

fn job_lock() -> std::sync::MutexGuard<'static, Job> {
    job_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn job_snapshot() -> Job {
    job_lock().clone()
}

/// Start a new acme job. Fails with the current snapshot if one is already running.
pub fn try_begin(kind: &str, cert_id: &str) -> Result<(), Job> {
    let mut job = job_lock();
    if job.running {
        return Err(job.clone());
    }
    *job = Job {
        running: true,
        kind: kind.to_string(),
        cert_id: cert_id.to_string(),
        log: String::new(),
        success: None,
        msg: String::new(),
    };
    Ok(())
}

pub fn job_finish(success: bool, log: &str) {
    let mut job = job_lock();
    job.running = false;
    job.success = Some(success);
    if !log.is_empty() {
        job.log = log.to_string();
        if job.log.len() > JOB_LOG_MAX {
            let extra = job.log.len() - JOB_LOG_MAX;
            job.log.drain(0..extra);
        }
    }
    if !success && job.msg.is_empty() {
        job.msg = log.chars().take(400).collect();
    }
}

fn job_append(text: &str) {
    if text.is_empty() {
        return;
    }
    let mut job = job_lock();
    if !job.running {
        return;
    }
    job.log.push_str(text);
    if job.log.len() > JOB_LOG_MAX {
        let extra = job.log.len() - JOB_LOG_MAX;
        job.log.drain(0..extra);
    }
}

pub struct AcmeStatus {
    pub found: bool,
    pub path: String,
    pub version: String,
}

pub fn status(db: &Db) -> AcmeStatus {
    let path = resolve_bin(db);
    if path.as_os_str().is_empty() {
        return AcmeStatus {
            found: false,
            path: String::new(),
            version: String::new(),
        };
    }
    let (ok, out) = run_acme(db, &["--version"], &[]);
    let version = out
        .lines()
        .find(|l| l.to_lowercase().contains("acme.sh") || l.contains("v"))
        .unwrap_or(out.trim())
        .trim()
        .to_string();
    AcmeStatus {
        found: ok || path.exists(),
        path: path.display().to_string(),
        version,
    }
}

pub fn resolve_bin(db: &Db) -> PathBuf {
    let setting = db.setting("acmeSh");
    if !setting.trim().is_empty() {
        let path = PathBuf::from(setting.trim());
        if path.is_file() {
            return path;
        }
    }
    for home in acme_home_candidates(db) {
        let bin = home.join("acme.sh");
        if bin.is_file() {
            return bin;
        }
    }
    which("acme.sh").unwrap_or_default()
}

pub fn acme_home_candidates(db: &Db) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };
    let setting = db.setting("acmeHome");
    if !setting.trim().is_empty() {
        push(PathBuf::from(setting.trim()));
    }
    let project = PathBuf::from(db.setting("home"));
    if !project.as_os_str().is_empty() {
        push(project.join(".acme.sh"));
    }
    if let Ok(home) = std::env::var("HOME") {
        push(PathBuf::from(home).join(".acme.sh"));
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        push(PathBuf::from(userprofile).join(".acme.sh"));
    }
    push(PathBuf::from("/root/.acme.sh"));
    push(PathBuf::from("/home/nginxWebUI/.acme.sh"));
    push(PathBuf::from("/opt/nginxWebUI/.acme.sh"));
    out
}

pub fn acme_home(db: &Db) -> PathBuf {
    let setting = db.setting("acmeHome");
    if !setting.trim().is_empty() {
        return PathBuf::from(setting.trim());
    }
    let bin = resolve_bin(db);
    if bin.is_file() {
        if let Some(parent) = bin.parent() {
            return parent.to_path_buf();
        }
    }
    acme_home_candidates(db)
        .into_iter()
        .find(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from(db.setting("home")).join(".acme.sh"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let cmd = dir.join(format!("{}.cmd", name));
            if cmd.is_file() {
                return Some(cmd);
            }
        }
    }
    None
}

fn dns_plugin(row: &Value) -> String {
    let kind = crate::compat::normalize_cert_kind(row);
    if kind == "dns_cf" || kind == "dns_gcore" || kind.starts_with("dns_") {
        return kind;
    }
    let dns = str_of(row, "dns");
    if dns == "dns_cf" || dns == "dns_gcore" {
        return dns;
    }
    db_default_plugin()
}

fn db_default_plugin() -> String {
    "dns_cf".into()
}

fn domains_of(row: &Value) -> Vec<String> {
    str_of(row, "domain")
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains("..") && !s.contains('/'))
        .collect()
}

/// `*.cdn.example.com` → issue for `cdn.example.com` + `*.cdn.example.com`,
/// file name `cdn.example.com` (issue-gcore-cert.sh).
pub fn issue_names(domain: &str) -> (String, Vec<String>) {
    if let Some(base) = domain.strip_prefix("*.") {
        (base.to_string(), vec![base.to_string(), domain.to_string()])
    } else {
        (domain.to_string(), vec![domain.to_string()])
    }
}

fn dns_env(db: &Db, plugin: &str, row: &Value) -> Vec<(String, String)> {
    let mut env = parse_env(&str_of(row, "env"));
    let push = |env: &mut Vec<(String, String)>, key: &str| {
        if env.iter().any(|(k, _)| k == key) {
            return;
        }
        let from_settings = db.setting(key);
        if !from_settings.is_empty() {
            env.push((key.to_string(), from_settings));
        } else if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                env.push((key.to_string(), v));
            }
        }
    };
    match plugin {
        "dns_cf" => {
            push(&mut env, "CF_Token");
            push(&mut env, "CF_Account_ID");
            if !str_of(row, "cfToken").is_empty() {
                env.retain(|(k, _)| k != "CF_Token");
                env.push(("CF_Token".into(), str_of(row, "cfToken")));
            }
            if !str_of(row, "cfAccountId").is_empty() {
                env.retain(|(k, _)| k != "CF_Account_ID");
                env.push(("CF_Account_ID".into(), str_of(row, "cfAccountId")));
            }
        }
        "dns_gcore" => {
            push(&mut env, "GCORE_Key");
        }
        _ => {}
    }
    env
}

fn credentials_ok(db: &Db, plugin: &str, row: &Value) -> bool {
    let env = dns_env(db, plugin, row);
    let account = acme_home(db).join("account.conf");
    let saved = fs_contains(&account);
    match plugin {
        "dns_cf" => {
            env.iter().any(|(k, _)| k == "CF_Token")
                || saved.contains("SAVED_CF_Token")
                || !str_of(row, "cfToken").is_empty()
        }
        "dns_gcore" => {
            env.iter().any(|(k, _)| k == "GCORE_Key") || saved.contains("SAVED_GCORE_Key")
        }
        _ => true,
    }
}

fn fs_contains(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

pub fn apply(db: &Db, row: &Value, force: bool) -> (bool, String, Value) {
    let domains = domains_of(row);
    if domains.is_empty() {
        return (false, "badDomain".into(), row.clone());
    }
    let kind = crate::compat::normalize_cert_kind(row);
    if kind == "import" && crate::compat::existing_acme_cert(db, &domains[0]).is_none() {
        return (false, "importCertHasNoIssue".into(), row.clone());
    }

    let bin = resolve_bin(db);
    if bin.as_os_str().is_empty() || !bin.exists() {
        return (false, "acmeNotFound".into(), row.clone());
    }

    let plugin = dns_plugin(row);
    let mut cert_name = String::new();
    let mut issue_domains: Vec<String> = Vec::new();
    for (i, domain) in domains.iter().enumerate() {
        let (name, extra) = issue_names(domain);
        if i == 0 {
            cert_name = name;
        }
        for item in extra {
            if !issue_domains.iter().any(|d| d == &item) {
                issue_domains.push(item);
            }
        }
    }
    let local = crate::compat::existing_acme_cert(db, &cert_name)
        .or_else(|| crate::compat::existing_acme_cert(db, &domains[0]));
    let ecc = str_of(row, "encryption").eq_ignore_ascii_case("ECC")
        || local.as_ref().map(|p| p.ecc).unwrap_or(false);

    if local.is_none() {
        if plugin != "dns_cf" && plugin != "dns_gcore" && !plugin.starts_with("dns_") {
            return (false, "dnsProviderRequired".into(), row.clone());
        }
        if !credentials_ok(db, &plugin, row) {
            let hint = if plugin == "dns_gcore" {
                "missing GCORE_Key (settings or ~/.acme.sh/account.conf)"
            } else {
                "missing CF_Token / CF_Account_ID (settings or ~/.acme.sh/account.conf)"
            };
            return (false, hint.into(), row.clone());
        }
    }

    let ssl = site::ssl_dir(db);
    if let Err(err) = crate::privfs::mkdir_all(&ssl) {
        return (
            false,
            format!("cannot create {}: {}", ssl.display(), err),
            row.clone(),
        );
    }

    let (cer, key) = {
        let existing_pem = str_of(row, "pem");
        let existing_key = str_of(row, "key");
        if !existing_pem.is_empty()
            && Path::new(&existing_pem).is_file()
            && !existing_key.is_empty()
            && Path::new(&existing_key).is_file()
            && crate::compat::cert_file_owned(db, &existing_pem)
        {
            (PathBuf::from(existing_pem), PathBuf::from(existing_key))
        } else if let Some(pair) = crate::compat::cert_pairs(db, &cert_name)
            .into_iter()
            .find(|p| p.source == "sslDir")
        {
            (pair.cer, pair.key)
        } else {
            (
                ssl.join(format!("{}.cer", cert_name)),
                ssl.join(format!("{}.key", cert_name)),
            )
        }
    };

    let home = acme_home(db).display().to_string();
    let mut args: Vec<String> = Vec::new();
    if local.is_some() {
        args.push("--renew".into());
        args.push("-d".into());
        args.push(cert_name.clone());
        if ecc {
            args.push("--ecc".into());
        }
        if force || bool_of(row, "force", false) {
            args.push("--force".into());
        }
        args.push("--home".into());
        args.push(home.clone());
    } else {
        args.push("--issue".into());
        args.push("--dns".into());
        args.push(plugin.clone());
        args.push("--server".into());
        args.push({
            let ca = str_of(row, "ca");
            if ca.is_empty() {
                "letsencrypt".into()
            } else {
                ca
            }
        });
        args.push("--home".into());
        args.push(home.clone());
        for d in &issue_domains {
            args.push("-d".into());
            args.push(d.clone());
        }
        if force || bool_of(row, "force", false) {
            args.push("--force".into());
        }
        if ecc {
            args.push("--ecc".into());
            args.push("--keylength".into());
            args.push("ec-256".into());
        }
        let email = {
            let from_row = str_of(row, "email");
            if from_row.is_empty() {
                db.setting("acmeEmail")
            } else {
                from_row
            }
        };
        if !email.is_empty() {
            args.push("--accountemail".into());
            args.push(email);
        }
    }

    let env_pairs = dns_env(db, &plugin, row);
    let mut log = format!("$ acme.sh {}\n", args.join(" "));
    job_append(&log);
    let (ok, out) = run_acme_env(db, &args, &env_pairs, true);
    log.push_str(&out);
    if !ok {
        return (false, log, row.clone());
    }

    let stage = acme_home(db).join(".nginxwebui-stage").join(&cert_name);
    if let Err(err) = std::fs::create_dir_all(&stage) {
        return (
            false,
            format!("cannot create {}: {}", stage.display(), err),
            row.clone(),
        );
    }
    let stage_cer = stage.join("fullchain.cer");
    let stage_key = stage.join("cert.key");
    let mut install = vec![
        "--install-cert".into(),
        "-d".into(),
        cert_name.clone(),
        "--fullchain-file".into(),
        stage_cer.display().to_string(),
        "--key-file".into(),
        stage_key.display().to_string(),
        "--reloadcmd".into(),
        "true".into(),
        "--home".into(),
        acme_home(db).display().to_string(),
    ];
    if ecc {
        install.push("--ecc".into());
    }
    let install_header = format!("\n$ acme.sh {}\n", install.join(" "));
    job_append(&install_header);
    log.push_str(&install_header);
    let (ok_install, out_install) = run_acme_env(db, &install, &env_pairs, true);
    log.push_str(&out_install);
    if !ok_install {
        return (false, log, row.clone());
    }

    let cer_bytes = match std::fs::read(&stage_cer) {
        Ok(b) => b,
        Err(err) => {
            return (
                false,
                format!("read {}: {}", stage_cer.display(), err),
                row.clone(),
            )
        }
    };
    let key_bytes = match std::fs::read(&stage_key) {
        Ok(b) => b,
        Err(err) => {
            return (
                false,
                format!("read {}: {}", stage_key.display(), err),
                row.clone(),
            )
        }
    };
    if let Err(err) = crate::privfs::write(&cer, &cer_bytes) {
        return (false, format!("install cert: {err}"), row.clone());
    }
    if let Err(err) = crate::privfs::write(&key, &key_bytes) {
        return (false, format!("install key: {err}"), row.clone());
    }
    let _ = crate::privfs::chmod(&cer, "644");
    let _ = crate::privfs::chmod(&key, "600");
    let (rok, rout) = crate::nginx::reload(db);
    let reload_line = format!("\nreload ok={} {}\n", rok, rout.trim());
    job_append(&reload_line);
    log.push_str(&reload_line);
    let info = format!("\n{}", site::openssl_info(&cer));
    job_append(&info);
    log.push_str(&info);

    let mut updated = row.clone();
    if let Value::Object(map) = &mut updated {
        map.insert("domain".into(), domains[0].clone().into());
        map.insert("certName".into(), cert_name.into());
        map.insert("pem".into(), cer.display().to_string().into());
        map.insert("cer".into(), cer.display().to_string().into());
        map.insert("key".into(), key.display().to_string().into());
        map.insert("kind".into(), plugin.into());
        map.insert("makeTime".into(), now_ms().into());
        map.insert("lastLog".into(), log.clone().into());
    }
    (true, log, updated)
}

pub fn cron(db: &Db) -> (bool, String) {
    if resolve_bin(db).as_os_str().is_empty() {
        return (false, "acmeNotFound".into());
    }
    let home = acme_home(db).display().to_string();
    run_acme_env(db, &["--cron".into(), "--home".into(), home], &[], true)
}

fn parse_env(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
        }
    }
    out
}

fn run_acme(db: &Db, args: &[&str], extra_env: &[(String, String)]) -> (bool, String) {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    run_acme_env(db, &owned, extra_env, false)
}

fn acme_command(bin: &Path) -> Command {
    #[cfg(unix)]
    {
        let stdbuf = Path::new("/usr/bin/stdbuf");
        if stdbuf.is_file() {
            let mut command = Command::new(stdbuf);
            command.args(["-oL", "-eL"]);
            command.arg(bin);
            return command;
        }
    }
    Command::new(bin)
}

fn pump_pipe<R: Read + Send + 'static>(
    mut reader: R,
    collected: Arc<Mutex<String>>,
    live: bool,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).replace('\r', "\n");
                    if let Ok(mut text) = collected.lock() {
                        text.push_str(&chunk);
                    }
                    if live {
                        job_append(&chunk);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn run_acme_env(
    db: &Db,
    args: &[String],
    extra_env: &[(String, String)],
    live: bool,
) -> (bool, String) {
    let bin = resolve_bin(db);
    if bin.as_os_str().is_empty() {
        let msg = "acmeNotFound";
        if live {
            job_append(msg);
        }
        return (false, msg.into());
    }
    let mut command = acme_command(&bin);
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let home_setting = db.setting("home");
    let home = Path::new(&home_setting);
    if home.exists() {
        command.current_dir(home);
    }
    let acme_parent = acme_home(db)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| home_setting.clone());
    if !acme_parent.is_empty() {
        command.env("HOME", acme_parent);
    } else if let Ok(user_home) = std::env::var("HOME") {
        command.env("HOME", user_home);
    } else {
        command.env("HOME", &home_setting);
    }
    for (k, v) in extra_env {
        command.env(k, v);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let msg = format!("{}: {}", bin.display(), err);
            if live {
                job_append(&msg);
            }
            return (false, msg);
        }
    };
    let collected = Arc::new(Mutex::new(String::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(pump_pipe(stdout, collected.clone(), live));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(pump_pipe(stderr, collected.clone(), live));
    }
    let status = child.wait();
    for handle in readers {
        let _ = handle.join();
    }
    let text = collected.lock().map(|g| g.clone()).unwrap_or_default();
    match status {
        Ok(status) => (status.success(), text),
        Err(err) => {
            let msg = format!("{text}{err}");
            if live {
                job_append(&format!("{err}"));
            }
            (false, msg)
        }
    }
}

pub fn summarize(db: &Db) -> Value {
    let s = status(db);
    json!({
        "found": s.found,
        "path": s.path,
        "version": s.version,
        "home": acme_home(db).display().to_string(),
        "email": db.setting("acmeEmail"),
        "sslDir": site::ssl_dir(db).display().to_string(),
        "hasCf": credentials_ok(db, "dns_cf", &Value::Null),
        "hasGcore": credentials_ok(db, "dns_gcore", &Value::Null),
    })
}

#[allow(dead_code)]
pub fn as_object(row: Value) -> Map<String, Value> {
    match row {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_issue_names() {
        let (name, domains) = issue_names("*.cdn.example.com");
        assert_eq!(name, "cdn.example.com");
        assert_eq!(domains, vec!["cdn.example.com", "*.cdn.example.com"]);
        let (name, domains) = issue_names("www.example.com");
        assert_eq!(name, "www.example.com");
        assert_eq!(domains, vec!["www.example.com"]);
    }

    #[test]
    fn apply_collects_all_issue_names() {
        let (name, extra) = issue_names("*.cdn.example.com");
        assert_eq!(name, "cdn.example.com");
        let mut all = extra;
        let (_, more) = issue_names("www.example.com");
        for item in more {
            if !all.contains(&item) {
                all.push(item);
            }
        }
        assert!(all.contains(&"cdn.example.com".to_string()));
        assert!(all.contains(&"*.cdn.example.com".to_string()));
        assert!(all.contains(&"www.example.com".to_string()));
    }

    #[test]
    fn job_refuses_a_second_start() {
        job_finish(true, "");
        try_begin("apply", "cert-1").expect("first job starts");
        let busy = try_begin("renew", "cert-2").expect_err("second job is rejected");
        assert!(busy.running);
        assert_eq!(busy.cert_id, "cert-1");
        job_append("hello\n");
        job_finish(true, "hello\ndone\n");
        let done = job_snapshot();
        assert!(!done.running);
        assert_eq!(done.success, Some(true));
        assert!(done.log.contains("done"));
        try_begin("cron", "").expect("after finish a new job can start");
        job_finish(true, "");
    }
}
