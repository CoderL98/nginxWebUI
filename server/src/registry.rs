//! Outbound-agent registry: join tokens, long-lived credentials, job hub.

use crate::api::{fail, json_reply, ok, Ctx, Reply, Req};
use crate::store::{now_ms, str_of, Db};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use subtle::ConstantTimeEq;

pub const JOIN_TTL_MS: u64 = 24 * 60 * 60 * 1000;
pub const POLL_WAIT: Duration = Duration::from_secs(25);
pub const RPC_WAIT: Duration = Duration::from_secs(30);
pub const ONLINE_AFTER_MS: u64 = 90_000;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    queues: HashMap<String, VecDeque<JobWire>>,
    waiters: HashMap<String, VecDeque<mpsc::Sender<Option<JobWire>>>>,
    inflight: HashMap<String, mpsc::Sender<Result<Value, String>>>,
    last_seen: HashMap<String, u64>,
    revoked: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct JobWire {
    pub id: String,
    pub method: String,
    pub params: Value,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                queues: HashMap::new(),
                waiters: HashMap::new(),
                inflight: HashMap::new(),
                last_seen: HashMap::new(),
                revoked: HashSet::new(),
            })),
        }
    }

    pub fn touch(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_seen.insert(node_id.to_string(), now_ms());
    }

    pub fn last_seen(&self, node_id: &str) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .last_seen
            .get(node_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn is_online(&self, node_id: &str) -> bool {
        let seen = self.last_seen(node_id);
        seen > 0 && now_ms().saturating_sub(seen) <= ONLINE_AFTER_MS
    }

    pub fn revoke(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.revoked.insert(node_id.to_string());
        if let Some(waiters) = inner.waiters.remove(node_id) {
            for waiter in waiters {
                let _ = waiter.send(None);
            }
        }
        inner.queues.remove(node_id);
        inner.inflight.retain(|_, _| true);
        let stale: Vec<String> = inner
            .inflight
            .keys()
            .filter(|id| id.starts_with(&format!("{node_id}:")))
            .cloned()
            .collect();
        for id in stale {
            if let Some(tx) = inner.inflight.remove(&id) {
                let _ = tx.send(Err("nodeRevoked".into()));
            }
        }
    }

    pub fn is_revoked(&self, node_id: &str) -> bool {
        self.inner.lock().unwrap().revoked.contains(node_id)
    }

    pub fn wait_job(&self, node_id: &str, timeout: Duration) -> Option<JobWire> {
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            if inner.revoked.contains(node_id) {
                return None;
            }
            inner.last_seen.insert(node_id.to_string(), now_ms());
            if let Some(queue) = inner.queues.get_mut(node_id) {
                if let Some(job) = queue.pop_front() {
                    return Some(job);
                }
            }
            let (tx, rx) = mpsc::channel();
            inner
                .waiters
                .entry(node_id.to_string())
                .or_default()
                .push_back(tx);
            rx
        };
        match rx.recv_timeout(timeout) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    pub fn dispatch(
        &self,
        node_id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if self.is_revoked(node_id) {
            return Err("nodeRevoked".into());
        }
        let job_id = format!("{}:{}", node_id, crate::store::new_id());
        let (tx, rx) = mpsc::channel();
        let wire = JobWire {
            id: job_id.clone(),
            method: method.to_string(),
            params,
        };
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.revoked.contains(node_id) {
                return Err("nodeRevoked".into());
            }
            inner.inflight.insert(job_id.clone(), tx);
            if let Some(waiters) = inner.waiters.get_mut(node_id) {
                while let Some(waiter) = waiters.pop_front() {
                    if waiter.send(Some(wire.clone())).is_ok() {
                        // delivered to a waiting poll
                        drop(inner);
                        return recv_result(rx, timeout, &self.inner, &job_id);
                    }
                }
            }
            inner
                .queues
                .entry(node_id.to_string())
                .or_default()
                .push_back(wire);
        }
        recv_result(rx, timeout, &self.inner, &job_id)
    }

    pub fn complete(&self, job_id: &str, result: Result<Value, String>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(tx) = inner.inflight.remove(job_id) {
            let _ = tx.send(result);
            true
        } else {
            false
        }
    }
}

fn recv_result(
    rx: Receiver<Result<Value, String>>,
    timeout: Duration,
    inner: &Mutex<Inner>,
    job_id: &str,
) -> Result<Value, String> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            let mut inner = inner.lock().unwrap();
            inner.inflight.remove(job_id);
            for queue in inner.queues.values_mut() {
                queue.retain(|job| job.id != job_id);
            }
            Err("nodeTimeout".into())
        }
        Err(RecvTimeoutError::Disconnected) => Err("nodeOffline".into()),
    }
}

pub struct PollTask {
    pub request: tiny_http::Request,
    pub app: Arc<Ctx>,
    pub req: Req,
}

pub struct AgentPool {
    tx: SyncSender<PollTask>,
}

impl AgentPool {
    pub fn new(workers: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::sync_channel::<PollTask>(workers.saturating_mul(2).max(8));
        let rx = Arc::new(Mutex::new(rx));
        let n = workers.clamp(1, 128);
        for _ in 0..n {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name("agent-poll".into())
                .spawn(move || loop {
                    let task = {
                        let locked = rx.lock().unwrap();
                        locked.recv()
                    };
                    match task {
                        Ok(task) => {
                            let reply = handle_agent(&task.app, &task.req);
                            crate::server::respond(task.request, reply);
                        }
                        Err(_) => break,
                    }
                });
        }
        Arc::new(Self { tx })
    }

    pub fn submit(&self, task: PollTask) -> Result<(), PollTask> {
        self.tx.try_send(task).map_err(|err| match err {
            TrySendError::Full(task) | TrySendError::Disconnected(task) => task,
        })
    }
}

pub fn hash_secret(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"nginxwebui-node-v1:");
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn secrets_match(provided: &str, stored_hash: &str) -> bool {
    if stored_hash.is_empty() || provided.is_empty() {
        return false;
    }
    let hashed = hash_secret(provided);
    hashed.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

pub fn new_secret(prefix: &str) -> String {
    format!("{prefix}{}", crate::auth::random_token(32))
}

pub fn public_node(row: &Value, hub: &Hub) -> Value {
    let id = str_of(row, "id");
    let revoked = str_of(row, "revoked") == "true" || bool_flag(row, "revoked");
    let pending = str_of(row, "joinTokenHash").is_empty() == false
        && str_of(row, "credHash").is_empty()
        && !revoked;
    let online = !revoked && hub.is_online(&id);
    let status = if revoked {
        "revoked"
    } else if online {
        "online"
    } else if pending {
        "pending"
    } else {
        "offline"
    };
    json!({
        "id": id,
        "name": str_of(row, "name"),
        "status": status,
        "kind": "agent",
        "lastSeen": hub.last_seen(&id),
        "hostname": str_of(row, "hostname"),
        "os": str_of(row, "os"),
        "agentVersion": str_of(row, "agentVersion"),
        "joined": !str_of(row, "credHash").is_empty(),
    })
}

pub fn local_node() -> Value {
    json!({
        "id": "local",
        "name": "local",
        "status": "online",
        "kind": "local",
        "lastSeen": now_ms(),
        "hostname": "",
        "os": std::env::consts::OS,
        "agentVersion": crate::VERSION,
        "joined": true,
    })
}

fn bool_flag(row: &Value, key: &str) -> bool {
    match row.get(key) {
        Some(Value::Bool(v)) => *v,
        Some(Value::String(s)) => s == "true" || s == "1",
        _ => false,
    }
}

pub fn strip_secrets(row: &mut Value) {
    if let Value::Object(map) = row {
        map.remove("joinTokenHash");
        map.remove("credHash");
        map.remove("joinToken");
        map.remove("credential");
    }
}

pub fn issue_join_token(db: &mut Db, node_id: &str) -> Result<(String, u64), String> {
    let Some(mut row) = db.get("node", node_id) else {
        return Err("not found".into());
    };
    if bool_flag(&row, "revoked") {
        return Err("nodeRevoked".into());
    }
    let token = new_secret("nwj_");
    let expires = now_ms().saturating_add(JOIN_TTL_MS);
    if let Value::Object(map) = &mut row {
        map.insert("joinTokenHash".into(), hash_secret(&token).into());
        map.insert("joinExpires".into(), expires.to_string().into());
    }
    db.upsert("node", row);
    Ok((token, expires))
}

pub fn create_node(db: &mut Db, name: &str) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("nameRequired".into());
    }
    if name.eq_ignore_ascii_case("local") {
        return Err("reservedName".into());
    }
    let row = json!({
        "name": name,
        "revoked": false,
        "created": now_ms().to_string(),
        "hostname": "",
        "os": "",
        "agentVersion": "",
        "joinTokenHash": "",
        "joinExpires": "0",
        "credHash": "",
    });
    let id = db.upsert("node", row);
    db.get("node", &id).ok_or_else(|| "saveFailed".into())
}

pub fn rename_node(db: &mut Db, id: &str, name: &str) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("nameRequired".into());
    }
    let Some(mut row) = db.get("node", id) else {
        return Err("not found".into());
    };
    if let Value::Object(map) = &mut row {
        map.insert("name".into(), name.into());
    }
    db.upsert("node", row);
    db.get("node", id).ok_or_else(|| "saveFailed".into())
}

pub fn revoke_node(db: &mut Db, hub: &Hub, id: &str) -> Result<(), String> {
    let Some(mut row) = db.get("node", id) else {
        return Err("not found".into());
    };
    if let Value::Object(map) = &mut row {
        map.insert("revoked".into(), true.into());
        map.insert("joinTokenHash".into(), "".into());
        map.insert("credHash".into(), "".into());
    }
    db.upsert("node", row);
    hub.revoke(id);
    Ok(())
}

pub fn join_with_token(db: &mut Db, token: &str, meta: &Value) -> Result<(Value, String), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("badToken".into());
    }
    let now = now_ms();
    let mut found: Option<Value> = None;
    for row in db.rows("node") {
        if bool_flag(row, "revoked") {
            continue;
        }
        let hash = str_of(row, "joinTokenHash");
        if hash.is_empty() {
            continue;
        }
        if secrets_match(token, &hash) {
            found = Some(row.clone());
            break;
        }
    }
    let Some(mut row) = found else {
        return Err("badToken".into());
    };
    let expires = str_of(&row, "joinExpires").parse::<u64>().unwrap_or(0);
    if expires > 0 && now > expires {
        return Err("tokenExpired".into());
    }
    let cred = new_secret("nwk_");
    if let Value::Object(map) = &mut row {
        map.insert("credHash".into(), hash_secret(&cred).into());
        map.insert("joinTokenHash".into(), "".into());
        map.insert("joinExpires".into(), "0".into());
        map.insert("hostname".into(), str_of(meta, "hostname").into());
        map.insert("os".into(), str_of(meta, "os").into());
        map.insert("agentVersion".into(), str_of(meta, "version").into());
        map.insert("joinedAt".into(), now.to_string().into());
    }
    let id = db.upsert("node", row);
    let saved = db.get("node", &id).unwrap();
    Ok((saved, cred))
}

pub fn node_by_credential<'a>(db: &'a Db, credential: &str) -> Option<Value> {
    if credential.is_empty() {
        return None;
    }
    for row in db.rows("node") {
        if bool_flag(row, "revoked") {
            continue;
        }
        let hash = str_of(row, "credHash");
        if secrets_match(credential, &hash) {
            return Some(row.clone());
        }
    }
    None
}

fn bearer(req: &Req) -> String {
    let header = req.authorization.trim();
    if let Some(rest) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    {
        return rest.trim().to_string();
    }
    let body = parse_body(req);
    let from_body = str_of(&body, "credential");
    if !from_body.is_empty() {
        return from_body;
    }
    req.query.get("credential").cloned().unwrap_or_default()
}

fn parse_body(req: &Req) -> Value {
    if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    }
}

pub fn handle_agent(ctx: &Ctx, req: &Req) -> Reply {
    let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 || parts[0] != "agent" || parts[1] != "v1" {
        return fail("not found");
    }
    match parts[2] {
        "join" => {
            let body = parse_body(req);
            let token = str_of(&body, "token");
            let mut store = ctx.store.lock().unwrap();
            match join_with_token(&mut store.db, &token, &body) {
                Ok((row, cred)) => {
                    let _ = store.flush();
                    ok(json!({
                        "nodeId": str_of(&row, "id"),
                        "name": str_of(&row, "name"),
                        "credential": cred,
                    }))
                }
                Err(err) => fail(&err),
            }
        }
        "hello" | "poll" | "result" => {
            let cred = bearer(req);
            let store = ctx.store.lock().unwrap();
            let Some(row) = node_by_credential(&store.db, &cred) else {
                return json_reply(
                    401,
                    &json!({ "success": false, "msg": "unauthorizedAgent" }),
                );
            };
            let id = str_of(&row, "id");
            if bool_flag(&row, "revoked") || ctx.hub.is_revoked(&id) {
                return fail("nodeRevoked");
            }
            drop(store);
            ctx.hub.touch(&id);
            match parts[2] {
                "hello" => {
                    let body = parse_body(req);
                    if !body.is_null() {
                        let mut store = ctx.store.lock().unwrap();
                        if let Some(mut row) = store.db.get("node", &id) {
                            if let Value::Object(map) = &mut row {
                                if !str_of(&body, "hostname").is_empty() {
                                    map.insert("hostname".into(), str_of(&body, "hostname").into());
                                }
                                if !str_of(&body, "os").is_empty() {
                                    map.insert("os".into(), str_of(&body, "os").into());
                                }
                                if !str_of(&body, "version").is_empty() {
                                    map.insert(
                                        "agentVersion".into(),
                                        str_of(&body, "version").into(),
                                    );
                                }
                            }
                            store.db.upsert("node", row);
                            let _ = store.flush();
                        }
                    }
                    ok(json!({ "nodeId": id, "ok": true }))
                }
                "poll" => {
                    let job = ctx.hub.wait_job(&id, POLL_WAIT);
                    let jobs = match job {
                        Some(job) => vec![json!({
                            "id": job.id,
                            "method": job.method,
                            "params": job.params,
                        })],
                        None => Vec::new(),
                    };
                    ok(json!({ "jobs": jobs }))
                }
                "result" => {
                    let body = parse_body(req);
                    let job_id = str_of(&body, "jobId");
                    if job_id.is_empty() || !job_id.starts_with(&format!("{id}:")) {
                        return fail("badJob");
                    }
                    let result = if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(true)
                        && body
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty()
                    {
                        Ok(body.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(str_of(&body, "error"))
                    };
                    if ctx.hub.complete(&job_id, result) {
                        ok(json!({ "ok": true }))
                    } else {
                        fail("unknownJob")
                    }
                }
                _ => fail("not found"),
            }
        }
        _ => fail("not found"),
    }
}

pub fn proxy(ctx: &Ctx, req: &Req, node_id: &str, method: &str, params: Value) -> Reply {
    if !crate::rpc::is_allowed(&method) {
        return fail("unsupportedMethod");
    }
    let (name, revoked) = {
        let store = ctx.store.lock().unwrap();
        match store.db.get("node", node_id) {
            Some(row) => (str_of(&row, "name"), bool_flag(&row, "revoked")),
            None => return fail("unknownNode"),
        }
    };
    if revoked || ctx.hub.is_revoked(node_id) {
        return fail("nodeRevoked");
    }
    let who = {
        let token = req
            .cookies
            .get("nginxwebui_token")
            .cloned()
            .unwrap_or_default();
        ctx.sessions
            .lock()
            .unwrap()
            .get(&token)
            .map(|s| s.admin_name.clone())
            .unwrap_or_else(|| "admin".into())
    };
    match ctx.hub.dispatch(node_id, method, params, RPC_WAIT) {
        Ok(value) => {
            let mut store = ctx.store.lock().unwrap();
            crate::api::record_user_operate(
                &mut store,
                &who,
                method,
                &format!("{name}/{node_id}"),
                true,
                "ok",
            );
            // record_system_operate uses adminName=system; add a proper user log:
            let _ = store.flush();
            // wrap raw RPC results into the usual envelope when they aren't already
            if value.get("success").is_some() {
                json_reply(200, &value)
            } else {
                ok(value)
            }
        }
        Err(err) => {
            let mut store = ctx.store.lock().unwrap();
            crate::api::record_user_operate(
                &mut store,
                &who,
                method,
                &format!("{name}/{node_id}"),
                false,
                &err,
            );
            let _ = store.flush();
            fail(&err)
        }
    }
}

pub fn install_script(req: &Req) -> Reply {
    let base = if req.panel_base.is_empty() {
        "http://127.0.0.1:8080".into()
    } else {
        req.panel_base.clone()
    };
    let script = format!(
        r#"#!/bin/sh
set -eu
PANEL="{base}"
TOKEN=""
INSECURE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --url) PANEL="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --insecure) INSECURE="--insecure"; shift ;;
    --) shift; break ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done
if [ -z "$TOKEN" ]; then
  echo "usage: sh install.sh --url https://panel.example --token <join-token>" >&2
  exit 1
fi
OS="$(uname -s | tr 'A-Z' 'a-z')"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
esac
DEST="${{NGINXWEBUI_AGENT_BIN:-./nginxwebui-agent}}"
URL="$PANEL/agent/download?os=$OS&arch=$ARCH"
if command -v curl >/dev/null 2>&1; then
  if curl -fsSL "$URL" -o "$DEST"; then
    chmod +x "$DEST"
  else
    echo "The panel did not serve an agent binary for $OS/$ARCH." >&2
    echo "Copy nginxwebui-agent next to this script (build with: cargo build --release --bin nginxwebui-agent)." >&2
    if [ ! -x "$DEST" ]; then
      exit 1
    fi
  fi
else
  echo "curl is required to download the agent binary" >&2
  exit 1
fi
"$DEST" join --url "$PANEL" --token "$TOKEN" $INSECURE
echo "Starting agent (run under systemd/supervisor in production)..."
exec "$DEST" run --url "$PANEL" $INSECURE
"#
    );
    Reply {
        status: 200,
        content_type: "text/x-shellscript; charset=utf-8".into(),
        body: script.into_bytes(),
        set_cookie: None,
        extra_headers: vec![("Cache-Control".into(), "no-store".into())],
    }
}

pub fn download_agent(ctx: &Ctx, req: &Req) -> Reply {
    let os = req.query.get("os").cloned().unwrap_or_default();
    let arch = req.query.get("arch").cloned().unwrap_or_default();
    let current_os = std::env::consts::OS;
    let current_arch = std::env::consts::ARCH;
    if !os.is_empty() && os != current_os || !arch.is_empty() && arch != current_arch {
        return Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".into(),
            body: format!(
                "no agent binary for {os}/{arch}; this panel built {current_os}/{current_arch}"
            )
            .into_bytes(),
            set_cookie: None,
            extra_headers: Vec::new(),
        };
    }
    let path = ctx.exe_dir.join("nginxwebui-agent");
    match std::fs::read(&path) {
        Ok(bytes) => Reply {
            status: 200,
            content_type: "application/octet-stream".into(),
            body: bytes,
            set_cookie: None,
            extra_headers: vec![(
                "Content-Disposition".into(),
                "attachment; filename=nginxwebui-agent".into(),
            )],
        },
        Err(_) => Reply {
            status: 404,
            content_type: "text/plain; charset=utf-8".into(),
            body: b"nginxwebui-agent is not installed next to the panel binary".to_vec(),
            set_cookie: None,
            extra_headers: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    #[test]
    fn join_token_is_one_time_and_can_expire() {
        let mut db = Db::new();
        let node = create_node(&mut db, "edge-1").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut db, &id).unwrap();
        let meta = json!({ "hostname": "a", "os": "linux", "version": "1.0.0" });
        let (saved, cred) = join_with_token(&mut db, &token, &meta).unwrap();
        assert_eq!(str_of(&saved, "hostname"), "a");
        assert!(str_of(&saved, "joinTokenHash").is_empty());
        assert!(!cred.is_empty());
        assert_eq!(
            join_with_token(&mut db, &token, &meta).unwrap_err(),
            "badToken"
        );

        let (token2, _) = issue_join_token(&mut db, &id).unwrap();
        if let Some(mut row) = db.get("node", &id) {
            if let Value::Object(map) = &mut row {
                map.insert("joinExpires".into(), "1".into());
            }
            db.upsert("node", row);
        }
        assert_eq!(
            join_with_token(&mut db, &token2, &meta).unwrap_err(),
            "tokenExpired"
        );
    }

    #[test]
    fn revoked_node_cannot_poll() {
        let mut db = Db::new();
        let hub = Hub::new();
        let node = create_node(&mut db, "edge-2").unwrap();
        let id = str_of(&node, "id");
        let (token, _) = issue_join_token(&mut db, &id).unwrap();
        let (_, cred) = join_with_token(
            &mut db,
            &token,
            &json!({ "hostname": "b", "os": "linux", "version": "1" }),
        )
        .unwrap();
        assert!(node_by_credential(&db, &cred).is_some());
        revoke_node(&mut db, &hub, &id).unwrap();
        assert!(node_by_credential(&db, &cred).is_none());
        assert!(hub.is_revoked(&id));
        assert!(hub.wait_job(&id, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn poll_receives_job_and_returns_result() {
        let hub = Hub::new();
        let node_id = "n1";
        let hub2 = hub.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            hub2.dispatch(node_id, "ping", json!({}), Duration::from_secs(2))
        });
        let job = hub
            .wait_job(node_id, Duration::from_secs(2))
            .expect("job should arrive");
        assert_eq!(job.method, "ping");
        hub.complete(&job.id, Ok(json!({ "pong": true })));
        let value = handle.join().unwrap().unwrap();
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn public_node_hides_hashes() {
        let hub = Hub::new();
        let row = json!({
            "id": "x",
            "name": "n",
            "joinTokenHash": "abc",
            "credHash": "",
            "revoked": false
        });
        let view = public_node(&row, &hub);
        assert!(view.get("joinTokenHash").is_none());
        assert!(view.get("credHash").is_none());
        assert_eq!(view["status"], "pending");
    }
}
