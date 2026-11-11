# nginxWebUI

A small Rust process (no JVM, no separate database server) that hosts a SvelteKit static UI and talks to the machine’s `nginx` and `acme.sh`. Site files go to Debian-style `sites-available` / `sites-enabled`; certificates land in `/etc/nginx/ssl`.

This is a simplified rewrite of the original Java/Solon nginxWebUI. See [DIFF.md](DIFF.md) for what was intentionally left out (SSO, mail alerts, extra DNS plugins). Multi-machine management uses an outbound-agent registry ([docs/remote-registry.md](docs/remote-registry.md)); agents need no inbound ports. Agents need nginx and acme.sh preinstalled; set the panel URL on the Devices page, `trusted_proxies` when behind a reverse proxy, and send `X-Requested-With: nginxwebui` when scripting admin write APIs. `--insecure` was removed. Keep panel and agent on the same version.

## Run

Edit `nginxwebui.conf` (port, data directory, frontend path). The panel listens on `127.0.0.1:8080` by default. Set `bind = 0.0.0.0` only if you need remote access, and put HTTPS in front of it on the public internet.

On first start with an empty admin table, the process prints a one-time init token (or use `NGINXWEBUI_INIT_TOKEN`). Creating the first administrator requires that token and a captcha.

Linux:

```bash
chmod +x script/linux/*.sh
sudo script/linux/setup-sudo.sh   # writes nginx as your user; does not grant apt/dnf
script/linux/deploy.sh
```

The panel UI can run on Windows; nginx paths, reload, and acme.sh still follow Linux conventions and are not installed or managed on Windows itself.

Docker: `docker compose up -d --build`. The compose file publishes the panel as `127.0.0.1:8080:8080`. The image still binds `0.0.0.0` inside the container so the mapping works. The container runs as root because it also starts nginx (80/443) and acme.sh under `/root/.acme.sh`.

License: [Mulan Permissive Software License, Version 1](LICENSE) (SPDX `MulanPSL-1.0`).

The full Chinese README is in [README.md](README.md).
