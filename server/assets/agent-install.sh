#!/bin/sh
# nginxWebUI agent installer, served by the panel at /agent/install.sh.
#
#   curl -fsSL https://panel.example/agent/install.sh | sh -s -- --url https://panel.example --token nwj_...
#
# As root on a systemd host (default): installs /usr/local/bin/nginxwebui-agent,
# keeps its config in /var/lib/nginxwebui-agent (agent.json, mode 0600) and
# enables nginxwebui-agent.service. Otherwise, or with --foreground, the agent
# is downloaded next to this script and runs in the foreground.
set -eu
PANEL="__PANEL__"
TOKEN=""
MODE="service"
RUN_USER="root"
while [ $# -gt 0 ]; do
  case "$1" in
    --url) PANEL="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --foreground) MODE="foreground"; shift ;;
    --user) RUN_USER="$2"; shift 2 ;;
    --insecure)
      echo "--insecure was removed: the agent always verifies TLS certificates." >&2
      echo "Put the panel behind HTTPS with a trusted certificate (or add your CA to the system trust store)." >&2
      exit 1 ;;
    --) shift; break ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done
if [ -z "$TOKEN" ]; then
  echo "usage: sh install.sh --url https://panel.example --token <join-token> [--foreground] [--user NAME]" >&2
  exit 1
fi
PANEL="${PANEL%/}"
case "$PANEL" in
  https://*) ;;
  http://127.*|http://localhost*|http://\[::1\]*)
    echo "warning: $PANEL is plain http on loopback (testing only)" >&2 ;;
  http://*)
    echo "WARNING: $PANEL is plain http. The agent binary, the join token and the" >&2
    echo "long-lived credential would travel unencrypted. Use an HTTPS reverse proxy." >&2
    if [ "${NGINXWEBUI_ALLOW_HTTP:-}" != "1" ]; then
      echo "Refusing to continue; set NGINXWEBUI_ALLOW_HTTP=1 only on a trusted network." >&2
      exit 1
    fi ;;
  *) echo "--url must start with https:// (or http:// on a trusted network)" >&2; exit 1 ;;
esac

OS="$(uname -s | tr 'A-Z' 'a-z')"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
esac

if [ "$MODE" = "service" ]; then
  if [ "$(id -u)" != "0" ] || ! command -v systemctl >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
    echo "Not root or no systemd: falling back to --foreground." >&2
    MODE="foreground"
  fi
fi

if [ "$MODE" = "service" ]; then
  DEST="${NGINXWEBUI_AGENT_BIN:-/usr/local/bin/nginxwebui-agent}"
  AGENT_HOME="${NGINXWEBUI_AGENT_HOME:-/var/lib/nginxwebui-agent}"
else
  DEST="${NGINXWEBUI_AGENT_BIN:-./nginxwebui-agent}"
  AGENT_HOME="${NGINXWEBUI_AGENT_HOME:-}"
fi

command -v curl >/dev/null 2>&1 || { echo "curl is required to download the agent binary" >&2; exit 1; }
TMP="$DEST.download.$$"
if curl -fsSL "$PANEL/agent/download?os=$OS&arch=$ARCH" -o "$TMP"; then
  chmod 755 "$TMP"
  mv -f "$TMP" "$DEST"
else
  rm -f "$TMP"
  echo "The panel did not serve an agent binary for $OS/$ARCH." >&2
  echo "Copy nginxwebui-agent to $DEST (cargo build --release --bin nginxwebui-agent)." >&2
  [ -x "$DEST" ] || exit 1
fi

HOME_ARGS=""
if [ -n "$AGENT_HOME" ]; then
  mkdir -p "$AGENT_HOME"
  chmod 700 "$AGENT_HOME"
  HOME_ARGS="--home $AGENT_HOME"
fi
# shellcheck disable=SC2086
"$DEST" join --url "$PANEL" --token "$TOKEN" $HOME_ARGS

if [ "$MODE" = "foreground" ]; then
  echo "Starting agent in the foreground (Ctrl-C to stop; use a service in production)..."
  # shellcheck disable=SC2086
  exec "$DEST" run $HOME_ARGS
fi

if [ "$RUN_USER" != "root" ]; then
  id "$RUN_USER" >/dev/null 2>&1 || useradd --system --home-dir "$AGENT_HOME" --shell /usr/sbin/nologin "$RUN_USER"
  chown -R "$RUN_USER" "$AGENT_HOME"
  echo "Running as $RUN_USER: writes to /etc/nginx and reloads need nginxwebui-priv + sudoers" >&2
  echo "(see script/linux/setup-sudo.sh in the nginxWebUI repository)." >&2
fi
UNIT=/etc/systemd/system/nginxwebui-agent.service
cat > "$UNIT" <<UNIT_EOF
[Unit]
Description=nginxWebUI agent (outbound connection to $PANEL)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$RUN_USER
Environment=NGINXWEBUI_AGENT_HOME=$AGENT_HOME
ExecStart=$DEST run --home $AGENT_HOME
Restart=always
RestartSec=5
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT_EOF
systemctl daemon-reload
systemctl enable --now nginxwebui-agent.service
echo "nginxwebui-agent.service enabled. Logs: journalctl -u nginxwebui-agent -f"
echo "Uninstall: systemctl disable --now nginxwebui-agent && rm $UNIT $DEST && rm -r $AGENT_HOME"
