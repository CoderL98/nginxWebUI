# Changelog

## Unreleased

### Sync groups, complete sync, and Windows agents

- Complete sync includes certificate files and `nginx -t` + reload.
- Sync groups: active-active and primary-standby (auto-promote, healthz, loop guard).
- Windows is a first-class managed agent node (`install.ps1`, Windows Service, prefix paths, Git Bash/WSL for acme.sh).

### Remaining hardening

- A revoked device can be re-joined: "重新接入" issues a new one-time token, clears revoked state, and does not require deleting the row.
- Agent `--tls-ca` and `--tls-fingerprint` (SHA-256 of the leaf cert, hex with optional colons) are enforced. `--insecure` stays removed.
- Optional built-in panel HTTPS via `tls_cert` / `tls_key` (config, env, or `--tls-cert` / `--tls-key`). Secure cookies are enabled automatically; a reverse proxy is still the usual production choice.
- Remote nodes can install nginx and a pinned acme.sh (confirm / `autoInstall` default off). The pin is `script/acme.sh.pin` (currently acme.sh 3.1.6); hash is verified before extract. Docker uses the same pin.
- `/agent/download` serves `agents/{os}-{arch}/nginxwebui-agent` (including `nginxwebui-agent.exe` for `windows-x86_64`) with a same-arch fallback. CI also cross-compiles the Windows agent via mingw.
- Agents may auto-upgrade from the panel after hello (verified TLS or loopback http; disable with `--no-upgrade` / `NGINXWEBUI_AGENT_NO_UPGRADE`). Hash mismatches are refused. On Windows the running exe is renamed `.old` then replaced; the service/process restarts and `.old` is removed on the next start.
- Manual sync copies site and cert records, certificate files from `sslDir`, then runs `nginx -t` and reloads only if the test passes. Sync groups support active-active replication and primary-standby auto-promote, plus `/healthz/group/{id}` for load balancers. Floating IPs are not moved.
- Windows is a supported managed agent: `nginx.exe` path defaults under the nginx prefix, `/agent/install.ps1` installs a Windows Service, and acme.sh runs through Git Bash or WSL.
- Docker: panel runs as `nginxwebui`; acme home is `/home/nginxWebUI/.acme.sh`. nginx may still bind 80/443 as root.
- Repo LICENSE remains MulanPSL-1.0; GitHub's web license field may still show Other.


### Remote agents (v2 hardening)

- Remote RPC now runs the same API handlers as the local panel (`{query, body}` params). Fixes remote `site/apply` by id wiping the row, partial saves dropping fields, and `conf/check` showing failure. Legacy agent replies (`{ok}`) are normalized.
- Remote mapping covers sites (incl. save/del/findCert), certs (list/scan/save/del/apply/renew/cron), per-device DNS credentials (`cert/dns`, write-only), log names/tail, nginx status/test/reload/signal, `install/status`, and remote nginx/acme.sh install (`install.nginx` / `install.acme` / `install.auto`). Panel settings, admins, devices, export and backups always stay local; other areas return `notSupportedOnNode`.
- Agents seed nginx paths like the panel, adopt existing sites/certs, run the acme renewal cron, and print effective paths.
- Offline nodes fail fast with `nodeOfflineErr`; only mutating RPCs are written to the operate log.
- One waiter per node: a newer poll supersedes the older one, timed-out polls are cleaned up, and a shared credential is flagged as a conflict.
- Join no longer issues a credential when the database write fails; hello only writes when something changed.

### Security (v2)

- Admin writes require `X-Requested-With: nginxwebui` (403 `csrfRequired`).
- `trusted_proxies` (off by default): forwarded headers are honored only from listed CIDRs.
- Body limits (413) and per-IP rate limits on `/agent/install.sh`, `/agent/download`, `/agent/v1/join` (429).
- Agent credentials are read only from the `Authorization` header; `agent.json` is written 0600 atomically; `NGINXWEBUI_AGENT_CREDENTIAL` supported. `--insecure` removed.
- Import keeps existing device credential hashes, cert secrets and password-type settings.
- `install.sh` installs a systemd service as root, refuses non-loopback `http://` unless `NGINXWEBUI_ALLOW_HTTP=1`.
- Install commands use the configured panel URL (`agentPanelUrl`) and warn about loopback or plain-http URLs.

### Packaging & UI (v2)

- Release builds and the Docker image ship `nginxwebui-agent`; `agent_threads` is configurable (default 32).
- Device switcher on small screens, remote-mode banners, panel-only items hidden or tagged, device page with panel URL, revoke vs delete, rename, conflict flag and auto-refresh.
- Clippy is clean and CI runs `clippy -D warnings`.

### Security

- Default listen address is `127.0.0.1`; Docker images still bind `0.0.0.0` and compose publishes the panel to `127.0.0.1:8080`.
- First administrator requires a one-time init token and captcha.
- Passwords use argon2id (legacy unsalted MD5 still verifies and is upgraded on login).
- Session tokens, captchas, and htpasswd salts use the OS CSPRNG.
- Login and captcha are rate-limited; secret settings are masked; `conf/replace` refuses a foreign main config.
- `reloadCmd` is an allow-list; `runCmd` accepts only enumerated actions.

### Added

- Outbound-agent registry: remote nodes dial the panel with HTTP long polling. No inbound ports on agents. See `docs/remote-registry.md`.

### Changed

- SQLite persistence uses row-level upsert on the existing `records` table and reuses the connection.
- Default worker threads are 2.
- Operation history is written to `operateLog` and `/healthz` is a public liveness endpoint.
- Automatic package/acme install defaults to off.

### Documentation

- Cargo crate license matches the root Mulan PSL v1 `LICENSE`.
