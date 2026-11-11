//! Outbound agent: join a panel, then long-poll for whitelist RPC jobs.

use crate::store::{str_of, Store};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
struct AgentConfig {
    url: String,
    node_id: String,
    credential: String,
    insecure: bool,
    ca_file: String,
    fingerprint: String,
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
            insecure: value
                .get("insecure")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            ca_file: str_of(&value, "ca"),
            fingerprint: str_of(&value, "fingerprint"),
            home: home.to_path_buf(),
        })
    }

    fn save(&self) -> Result<(), String> {
        let _ = std::fs::create_dir_all(&self.home);
        let value = json!({
            "url": self.url,
            "nodeId": self.node_id,
            "credential": self.credential,
            "insecure": self.insecure,
            "ca": self.ca_file,
            "fingerprint": self.fingerprint,
        });
        std::fs::write(
            Self::path(&self.home),
            serde_json::to_vec_pretty(&value).unwrap_or_default(),
        )
        .map_err(|err| format!("cannot write agent.json: {err}"))
    }
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
        insecure: false,
        ca_file: String::new(),
        fingerprint: String::new(),
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
                cfg.insecure = true;
                i += 1;
            }
            "--tls-ca" => {
                cfg.ca_file = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--tls-fingerprint" => {
                cfg.fingerprint = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
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
    println!("  --tls-ca FILE       extra CA certificate (PEM)");
    println!("  --tls-fingerprint   SHA-256 hex of the panel certificate");
    println!("  --insecure          skip TLS verification (development only)");
}

fn build_agent(cfg: &AgentConfig) -> Result<ureq::Agent, String> {
    if cfg.insecure {
        eprintln!(
            "warning: --insecure disables nothing here if the URL is https://; this client verifies certificates by default. Use http:// only on a trusted network, or --tls-ca for a private CA."
        );
    }
    if !cfg.ca_file.is_empty() {
        eprintln!(
            "warning: --tls-ca is recorded for operators; rustls uses the built-in/root store. Place the custom CA in the system trust store or terminate TLS at a reverse proxy the agent already trusts."
        );
        let _ = std::fs::read(&cfg.ca_file)
            .map_err(|err| format!("cannot read {}: {err}", cfg.ca_file))?;
    }
    if !cfg.fingerprint.is_empty() {
        eprintln!(
            "warning: --tls-fingerprint cannot be enforced by this HTTP client; pin a CA at the reverse proxy or system trust store. Default TLS verification still applies."
        );
    }
    Ok(ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(40))
        .timeout_write(Duration::from_secs(15))
        .build())
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

fn do_run(agent: &ureq::Agent, cfg: &AgentConfig) -> Result<(), String> {
    if cfg.credential.is_empty() {
        return Err("not joined; run `nginxwebui-agent join` first".into());
    }
    let _ = std::fs::create_dir_all(cfg.home.join("data"));
    let mut store = Store::open(&cfg.home.join("data"));
    if store.db.setting("home").is_empty() {
        store
            .db
            .set_setting("home", &cfg.home.display().to_string());
        let _ = store.flush();
    }
    let base = cfg.url.trim_end_matches('/').to_string();
    let hello = format!("{base}/agent/v1/hello");
    let poll = format!("{base}/agent/v1/poll");
    let result_url = format!("{base}/agent/v1/result");
    let mut backoff = 1u64;
    loop {
        let hello_body = json!({
            "hostname": hostname(),
            "os": std::env::consts::OS,
            "version": crate::VERSION,
        });
        if let Err(err) = post_json(agent, &hello, &cfg.credential, hello_body) {
            eprintln!("hello failed: {err}");
            std::thread::sleep(Duration::from_secs(backoff));
            backoff = (backoff * 2).min(30);
            continue;
        }
        backoff = 1;
        match post_json(agent, &poll, &cfg.credential, json!({})) {
            Ok(value) => {
                let jobs = value
                    .get("obj")
                    .and_then(|o| o.get("jobs"))
                    .cloned()
                    .unwrap_or_else(|| value.get("jobs").cloned().unwrap_or(Value::Array(vec![])));
                if let Value::Array(jobs) = jobs {
                    for job in jobs {
                        let job_id = str_of(&job, "id");
                        let method = str_of(&job, "method");
                        let params = job.get("params").cloned().unwrap_or(Value::Null);
                        let outcome = crate::rpc::execute(&mut store.db, &method, &params);
                        let _ = store.flush();
                        let body = match outcome {
                            Ok(result) => json!({
                                "jobId": job_id,
                                "ok": true,
                                "result": result
                            }),
                            Err(err) => json!({
                                "jobId": job_id,
                                "ok": false,
                                "error": err
                            }),
                        };
                        if let Err(err) = post_json(agent, &result_url, &cfg.credential, body) {
                            eprintln!("result upload failed: {err}");
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("poll failed: {err}");
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(30);
            }
        }
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
            if !cfg.insecure {
                cfg.insecure = saved.insecure;
            }
            if cfg.ca_file.is_empty() {
                cfg.ca_file = saved.ca_file;
            }
            if cfg.fingerprint.is_empty() {
                cfg.fingerprint = saved.fingerprint;
            }
        }
    }
    if cfg.url.is_empty() {
        return Err("--url is required (or save it via join)".into());
    }
    if cfg.insecure && !cfg.url.starts_with("https://") && !cfg.url.starts_with("http://") {
        return Err("url must start with http:// or https://".into());
    }
    let agent = build_agent(&cfg)?;
    match cmd.as_str() {
        "join" => do_join(&agent, &mut cfg),
        "run" => do_run(&agent, &cfg),
        other => Err(format!("unknown command: {other}")),
    }
}
