//! Outbound agent: join a panel, then long-poll for whitelist RPC jobs.
//!
//! The agent never listens on a port. It keeps its own SQLite under its home
//! directory, seeds it with the same system nginx layout the panel uses, and
//! runs each job through the panel's own API handlers (see `rpc.rs`).

use crate::api::Ctx;
use crate::store::{str_of, Store};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
struct AgentConfig {
    url: String,
    node_id: String,
    credential: String,
    ca_file: String,
    fingerprint: String,
    no_upgrade: bool,
    home: PathBuf,
}

impl AgentConfig {
    fn path(home: &Path) -> PathBuf {
        home.join("agent.json")
    }

    fn load(home: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(Self::path(home))
            .map_err(|err| format!("cannot read agent.json: {err}"))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|err| format!("bad agent.json: {err}"))?;
        Ok(Self {
            url: str_of(&value, "url"),
            node_id: str_of(&value, "nodeId"),
            credential: str_of(&value, "credential"),
            ca_file: str_of(&value, "ca"),
            fingerprint: str_of(&value, "fingerprint"),
            no_upgrade: value.get("upgrade").and_then(|v| v.as_bool()) == Some(false)
                || str_of(&value, "upgrade") == "false",
            home: home.to_path_buf(),
        })
    }

    /// Write agent.json atomically with mode 0600: it holds the long-lived
    /// credential that lets this host take nginx jobs from the panel.
    fn save(&self) -> Result<(), String> {
        create_private_dir(&self.home)?;
        let value = json!({
            "url": self.url,
            "nodeId": self.node_id,
            "credential": self.credential,
            "ca": self.ca_file,
            "fingerprint": self.fingerprint,
            "upgrade": !self.no_upgrade,
        });
        let path = Self::path(&self.home);
        let tmp = self.home.join(".agent.json.tmp");
        let bytes = serde_json::to_vec_pretty(&value).unwrap_or_default();
        write_private(&tmp, &bytes).map_err(|err| format!("cannot write agent.json: {err}"))?;
        std::fs::rename(&tmp, &path).map_err(|err| format!("cannot write agent.json: {err}"))?;
        restrict(&path).map_err(|err| format!("cannot chmod agent.json: {err}"))
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    let existed = dir.is_dir();
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let _ = existed;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn restrict(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .map(|s| matches!(s.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn default_home() -> PathBuf {
    if let Ok(dir) = std::env::var("NGINXWEBUI_AGENT_HOME") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("nginxwebui-agent");
    }
    PathBuf::from(".nginxwebui-agent")
}

fn parse_args() -> Result<(String, AgentConfig), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || args.is_empty() {
        print_help();
        std::process::exit(0);
    }
    let cmd = args[0].clone();
    let mut cfg = AgentConfig {
        url: String::new(),
        node_id: String::new(),
        credential: String::new(),
        ca_file: String::new(),
        fingerprint: String::new(),
        no_upgrade: false,
        home: default_home(),
    };
    let mut token = String::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--url" => {
                cfg.url = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--token" => {
                token = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--home" => {
                cfg.home = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
                i += 2;
            }
            "--insecure" => {
                return Err(
                    "--insecure was removed: TLS certificates are always verified. Use an HTTPS \
                     reverse proxy with a trusted certificate, add your CA to the system trust \
                     store, or use http:// only on a trusted network."
                        .into(),
                );
            }
            "--tls-ca" => {
                cfg.ca_file = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--tls-fingerprint" => {
                cfg.fingerprint = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--no-upgrade" => {
                cfg.no_upgrade = true;
                i += 1;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    if cmd == "join" {
        if cfg.url.is_empty() || token.is_empty() {
            return Err("join requires --url and --token".into());
        }
        // stash token in credential temporarily
        cfg.credential = token;
    }
    Ok((cmd, cfg))
}

fn print_help() {
    println!("nginxwebui-agent");
    println!("  join --url https://panel.example --token <join-token>");
    println!("  run  [--url https://panel.example]");
    println!("  --home DIR          config/data directory");
    println!("  --tls-ca FILE       trust this PEM CA (in addition to webpki roots)");
    println!("  --tls-fingerprint   pin SHA-256 of the panel leaf cert (hex, optional colons)");
    println!("  --no-upgrade        do not auto-download a newer agent from the panel");
    println!();
    println!("environment:");
    println!("  NGINXWEBUI_AGENT_HOME        config/data directory");
    println!("  NGINXWEBUI_AGENT_CREDENTIAL  credential to use instead of agent.json");
    println!("  NGINXWEBUI_AGENT_NO_UPGRADE  set to 1 to disable auto-upgrade");
    println!("  NGINXWEBUI_RELOAD_CMD        reload command default (allow-listed values only)");
}

fn build_agent(cfg: &AgentConfig) -> Result<ureq::Agent, String> {
    crate::tls::build_ureq_agent(&cfg.ca_file, &cfg.fingerprint, &cfg.url)
}

fn post_json(
    agent: &ureq::Agent,
    url: &str,
    credential: &str,
    body: Value,
) -> Result<Value, String> {
    let mut request = agent
        .post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    if !credential.is_empty() {
        request = request.set("Authorization", &format!("Bearer {credential}"));
    }
    let encoded = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
    let response = request
        .send_string(&encoded)
        .map_err(|err| format!("request failed: {err}"))?;
    let status = response.status();
    let mut text = String::new();
    let _ = response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut text);
    if status == 401 {
        return Err("unauthorizedAgent".into());
    }
    let value: Value = serde_json::from_str(&text).unwrap_or(json!({ "raw": text }));
    if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return Err(str_of(&value, "msg"));
    }
    Ok(value)
}

fn do_join(agent: &ureq::Agent, cfg: &mut AgentConfig) -> Result<(), String> {
    let hostname = hostname();
    let url = format!("{}/agent/v1/join", cfg.url.trim_end_matches('/'));
    let value = post_json(
        agent,
        &url,
        "",
        json!({
            "token": cfg.credential,
            "hostname": hostname,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "version": crate::VERSION,
        }),
    )?;
    let obj = value.get("obj").cloned().unwrap_or(value);
    cfg.node_id = str_of(&obj, "nodeId");
    cfg.credential = str_of(&obj, "credential");
    if cfg.node_id.is_empty() || cfg.credential.is_empty() {
        return Err("join did not return a credential".into());
    }
    cfg.save()?;
    println!("joined node {} ({})", str_of(&obj, "name"), cfg.node_id);
    Ok(())
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// Open the agent's store and give it the same defaults the panel would
/// have on this host (Debian `/etc/nginx` layout, reload command, ...).
pub fn open_node_context(home: &Path) -> Arc<Ctx> {
    let data = home.join("data");
    let _ = std::fs::create_dir_all(&data);
    crate::applog::init(home, None);
    if !crate::seed::has_system_nginx() {
        let _ = crate::seed::write_default_files(home);
    }
    let mut store = Store::open(&data);
    let reload = std::env::var("NGINXWEBUI_RELOAD_CMD").ok();
    crate::seed::seed(&mut store.db, home, reload.as_deref());
    let _ = crate::compat::adopt(&mut store.db);
    if let Err(err) = crate::site::ensure_main_includes_sites(&store.db) {
        crate::applog::warn(format!("ensure sites-enabled include: {err}"));
    }
    if let Err(err) = store.flush() {
        eprintln!("warning: cannot save agent database: {err}");
    }
    println!(
        "  nginx.conf : {}",
        crate::nginx::nginx_path(&store.db).display()
    );
    println!(
        "  sites      : {}",
        crate::site::sites_available(&store.db).display()
    );
    println!("  reload     : {}", store.db.setting("reloadCmd"));
    let mut ctx = Ctx::standalone(home.to_path_buf(), store);
    ctx.agent_mode = true;
    Arc::new(ctx)
}

fn do_run(agent: &ureq::Agent, cfg: &AgentConfig) -> Result<(), String> {
    if cfg.credential.is_empty() {
        return Err("not joined; run `nginxwebui-agent join` first".into());
    }
    println!("nginxwebui-agent {} → {}", crate::VERSION, cfg.url);
    let ctx = open_node_context(&cfg.home);
    // certificates issued on this node are renewed on this node
    crate::api::spawn_acme_cron(ctx.clone());
    let instance = crate::auth::random_token(16);
    let base = cfg.url.trim_end_matches('/').to_string();
    let hello = format!("{base}/agent/v1/hello");
    let poll = format!("{base}/agent/v1/poll");
    let result_url = format!("{base}/agent/v1/result");
    let mut backoff = 1u64;
    loop {
        let hello_body = json!({
            "hostname": hostname(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "version": crate::VERSION,
        });
        match post_json(agent, &hello, &cfg.credential, hello_body) {
            Err(err) => {
                eprintln!("hello failed: {err}");
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(30);
                continue;
            }
            Ok(hello_reply) => maybe_upgrade(agent, cfg, &hello_reply),
        }
        backoff = 1;
        match post_json(
            agent,
            &poll,
            &cfg.credential,
            json!({ "instance": instance }),
        ) {
            Ok(value) => {
                let jobs = value
                    .get("obj")
                    .and_then(|o| o.get("jobs"))
                    .cloned()
                    .unwrap_or_else(|| value.get("jobs").cloned().unwrap_or(Value::Array(vec![])));
                if let Value::Array(jobs) = jobs {
                    for job in jobs {
                        let body = run_job(&ctx, &job);
                        if let Err(err) = post_json(agent, &result_url, &cfg.credential, body) {
                            eprintln!("result upload failed: {err}");
                        }
                    }
                }
            }
            Err(err) if err == "pollSuperseded" => {
                eprintln!(
                    "poll superseded by another connection with the same credential; \
                     if this repeats, agent.json was copied to a second host"
                );
                std::thread::sleep(Duration::from_secs(5));
            }
            Err(err) => {
                eprintln!("poll failed: {err}");
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

fn maybe_upgrade(agent: &ureq::Agent, cfg: &AgentConfig, hello: &Value) {
    if cfg.no_upgrade {
        return;
    }
    let advertised = hello
        .get("obj")
        .and_then(|o| o.get("agent"))
        .cloned()
        .or_else(|| hello.get("agent").cloned())
        .unwrap_or(Value::Null);
    let current_hash = crate::upgrade::current_binary_sha256();
    match crate::upgrade::decide(
        true,
        &cfg.url,
        crate::VERSION,
        current_hash.as_deref(),
        &advertised,
    ) {
        crate::upgrade::Decision::Need {
            version,
            sha256,
            url,
        } => {
            eprintln!("upgrading agent to {version} from the panel...");
            match crate::upgrade::download(agent, &url) {
                Ok(bytes) => {
                    if let Err(err) = crate::upgrade::verify_bytes(&bytes, &sha256) {
                        eprintln!("upgrade refused: {err}");
                        return;
                    }
                    let dest = match std::env::current_exe() {
                        Ok(p) => p,
                        Err(err) => {
                            eprintln!("upgrade: cannot locate current binary: {err}");
                            return;
                        }
                    };
                    if let Err(err) = crate::upgrade::replace_binary(&dest, &bytes) {
                        eprintln!("upgrade install failed: {err}");
                        return;
                    }
                    eprintln!("upgrade installed; re-executing");
                    if let Err(err) = crate::upgrade::reexec(&dest) {
                        eprintln!("{err}");
                    }
                }
                Err(err) => eprintln!("{err}"),
            }
        }
        crate::upgrade::Decision::SkipInsecureTransport => {
            use std::sync::atomic::{AtomicBool, Ordering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "agent auto-upgrade skipped: panel URL is not verified TLS or loopback http (use --no-upgrade to hide this)"
                );
            }
        }
        _ => {}
    }
}

/// Largest result the panel accepts on /agent/v1/result (minus envelope).
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024 - 4096;

fn run_job(ctx: &Arc<Ctx>, job: &Value) -> Value {
    let job_id = str_of(job, "id");
    let method = str_of(job, "method");
    let params = job.get("params").cloned().unwrap_or(Value::Null);
    match crate::rpc::execute(ctx, &method, &params) {
        Ok(result) => {
            let size = serde_json::to_vec(&result).map(|v| v.len()).unwrap_or(0);
            if size > MAX_RESULT_BYTES {
                json!({ "jobId": job_id, "ok": false, "error": "resultTooLarge" })
            } else {
                json!({ "jobId": job_id, "ok": true, "result": result })
            }
        }
        Err(err) => json!({ "jobId": job_id, "ok": false, "error": err }),
    }
}

pub fn run() -> Result<(), String> {
    let (cmd, mut cfg) = parse_args()?;
    if cmd != "join" {
        if let Ok(saved) = AgentConfig::load(&cfg.home) {
            if cfg.url.is_empty() {
                cfg.url = saved.url;
            }
            if cfg.credential.is_empty() {
                cfg.credential = saved.credential;
            }
            if cfg.node_id.is_empty() {
                cfg.node_id = saved.node_id;
            }
            if cfg.ca_file.is_empty() {
                cfg.ca_file = saved.ca_file;
            }
            if cfg.fingerprint.is_empty() {
                cfg.fingerprint = saved.fingerprint;
            }
            if saved.no_upgrade {
                cfg.no_upgrade = true;
            }
        }
    }
    if env_flag("NGINXWEBUI_AGENT_NO_UPGRADE") {
        cfg.no_upgrade = true;
    }
    if let Ok(cred) = std::env::var("NGINXWEBUI_AGENT_CREDENTIAL") {
        if cmd != "join" && !cred.trim().is_empty() {
            cfg.credential = cred.trim().to_string();
        }
    }
    if cfg.url.is_empty() {
        return Err("--url is required (or save it via join)".into());
    }
    if !cfg.url.starts_with("https://") && !cfg.url.starts_with("http://") {
        return Err("url must start with https:// (or http:// on a trusted network)".into());
    }
    let agent = build_agent(&cfg)?;
    match cmd.as_str() {
        "join" => do_join(&agent, &mut cfg),
        "run" => do_run(&agent, &cfg),
        other => Err(format!("unknown command: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn agent_json_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!(
            "nginxwebui-agentcfg-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let cfg = AgentConfig {
            url: "https://panel.example".into(),
            node_id: "n1".into(),
            credential: "nwk_secret".into(),
            ca_file: String::new(),
            fingerprint: String::new(),
            no_upgrade: false,
            home: home.clone(),
        };
        cfg.save().unwrap();
        // saving again over an existing (wider) file still ends at 0600
        std::fs::set_permissions(
            AgentConfig::path(&home),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        cfg.save().unwrap();
        let mode = std::fs::metadata(AgentConfig::path(&home))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let loaded = AgentConfig::load(&home).unwrap();
        assert_eq!(loaded.credential, "nwk_secret");
    }

    #[test]
    fn loopback_detection() {
        assert!(crate::tls::is_loopback_url("http://127.0.0.1:8080"));
        assert!(crate::tls::is_loopback_url("http://localhost/x"));
        assert!(!crate::tls::is_loopback_url("http://10.0.0.2:8080"));
    }
}
