# nginxWebUI

A small Rust process (no JVM, no separate database server) that hosts a SvelteKit static UI and talks to the machine’s `nginx` and `acme.sh`. Site files go to Debian-style `sites-available` / `sites-enabled`; certificates land in `/etc/nginx/ssl`.

This is a simplified rewrite of the original Java/Solon nginxWebUI. See [DIFF.md](DIFF.md) for what was intentionally left out (SSO, mail alerts, extra DNS plugins). Multi-machine management uses an outbound-agent registry ([docs/remote-registry.md](docs/remote-registry.md)); agents need no inbound ports. Set the panel URL on the Devices page, `trusted_proxies` when behind a reverse proxy, and send `X-Requested-With: nginxwebui` when scripting admin write APIs. `--insecure` was removed; `--tls-ca` and `--tls-fingerprint` are enforced. Optional built-in panel TLS is available via `tls_cert` / `tls_key`. Agents may auto-upgrade from `/agent/download` (verified TLS or loopback http). Devices can install a pinned acme.sh release and manually copy sites; there is no automatic failover. Keep panel and agent on the same version. The repo LICENSE is MulanPSL-1.0; GitHub's license badge may still say Other.

## Run

Edit `nginxwebui.conf` (port, data directory, frontend path). The panel listens on `127.0.0.1:8080` by default. Set `bind = 0.0.0.0` only if you need remote access, and put HTTPS in front of it on the public internet.

On first start with an empty admin table, the process prints a one-time init token (or use `NGINXWEBUI_INIT_TOKEN`). Creating the first administrator requires that token and a captcha.

Linux:

```bash
chmod +x script/linux/*.sh
sudo script/linux/setup-sudo.sh   # writes nginx as your user; does not grant apt/dnf
script/linux/deploy.sh
```

The panel UI can run on Windows. A Windows machine can also be a managed agent node: install the nginxwebui-agent Windows service to manage nginx for Windows (paths under the nginx prefix). Certificate issuance uses the pinned acme.sh through Git Bash or WSL. Run the service as an account that can write the nginx directories.

Docker: `docker compose up -d --build`. The compose file publishes the panel as `127.0.0.1:8080:8080`. The image still binds `0.0.0.0` inside the container so the mapping works. The panel process runs as user `nginxwebui` with acme.sh under `/home/nginxWebUI/.acme.sh`. The nginx master that binds 80/443 may still be privileged.

License: [Mulan Permissive Software License, Version 1](LICENSE) (SPDX `MulanPSL-1.0`).

The full Chinese README is in [README.md](README.md).
