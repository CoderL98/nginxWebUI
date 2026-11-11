#!/bin/sh
set -eu

HOME_DIR="${NGINXWEBUI_HOME:-/home/nginxWebUI}"
DEFAULT_CONF=/opt/nginxwebui/nginxwebui.conf
USER_CONF="${NGINXWEBUI_CONFIG:-$HOME_DIR/nginxwebui.conf}"

mkdir -p \
  "$HOME_DIR/data" \
  "$HOME_DIR/logs" \
  /etc/nginx/sites-available \
  /etc/nginx/sites-enabled \
  /etc/nginx/ssl \
  /etc/nginx/conf.d \
  /var/www \
  /var/log/nginx \
  "$HOME_DIR/.acme.sh"

# Old images used a volume at /root/.acme.sh; copy it once into the data home.
if [ -d /root/.acme.sh ] && [ ! -f "$HOME_DIR/.acme.sh/acme.sh" ]; then
  if [ -e /root/.acme.sh/acme.sh ] || [ -n "$(ls -A /root/.acme.sh 2>/dev/null || true)" ]; then
    cp -a /root/.acme.sh/. "$HOME_DIR/.acme.sh/" || true
  fi
fi
if [ ! -f "$HOME_DIR/.acme.sh/acme.sh" ] && [ -x /opt/acme.sh/acme.sh ]; then
  /opt/acme.sh/acme.sh --install --home "$HOME_DIR/.acme.sh" -m nginxwebui@localhost || true
fi

if [ ! -f /etc/nginx/ssl-params.conf ]; then
  cat >/etc/nginx/ssl-params.conf <<'EOF'
ssl_protocols TLSv1.2 TLSv1.3;
ssl_prefer_server_ciphers off;
ssl_session_timeout 1d;
ssl_session_cache shared:SSL:10m;
ssl_session_tickets off;
EOF
fi

if [ ! -f "$USER_CONF" ] && [ -f "$DEFAULT_CONF" ]; then
  cp "$DEFAULT_CONF" "$USER_CONF"
fi

PORT=""
BIND=""
WEB_DIR=""
START_NGINX="${NGINXWEBUI_START_NGINX:-}"
if [ -f "$USER_CONF" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%%#*}"
    key="${line%%=*}"
    val="${line#*=}"
    key=$(printf '%s' "$key" | tr -d ' \t' | tr 'A-Z-' 'a-z_')
    val=$(printf '%s' "$val" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//;s/^"//;s/"$//')
    case "$key" in
      port|server.port|server_port) PORT="$val" ;;
      bind|listen|server.bind|server_bind) BIND="$val" ;;
      home|project.home|project_home) HOME_DIR="$val" ;;
      web_dir|web.dir|web) WEB_DIR="$val" ;;
      start_nginx) START_NGINX="$val" ;;
      reload_cmd|reload.cmd|reloadcmd) export NGINXWEBUI_RELOAD_CMD="$val" ;;
    esac
  done < "$USER_CONF"
fi

PORT="${NGINXWEBUI_PORT:-${PORT:-8080}}"
BIND="${NGINXWEBUI_BIND:-${BIND:-0.0.0.0}}"
HOME_DIR="${NGINXWEBUI_HOME:-${HOME_DIR:-/home/nginxWebUI}}"
WEB_DIR="${NGINXWEBUI_WEB_DIR:-${WEB_DIR:-/opt/nginxwebui/web}}"
START_NGINX="${NGINXWEBUI_START_NGINX:-${START_NGINX:-1}}"
export NGINXWEBUI_RELOAD_CMD="${NGINXWEBUI_RELOAD_CMD:-nginx -s reload}"

nginx_running() {
  if [ -f /run/nginx.pid ]; then
    kill -0 "$(cat /run/nginx.pid)" 2>/dev/null
  else
    return 1
  fi
}

if [ "$START_NGINX" = "1" ]; then
  if ! nginx_running; then
    nginx
  fi
fi

term() {
  if [ "$START_NGINX" = "1" ]; then
    nginx -s quit 2>/dev/null || true
  fi
  if [ -n "${child:-}" ]; then
    kill -TERM "$child" 2>/dev/null || true
    wait "$child" || true
  fi
  exit 0
}
trap term TERM INT

# Panel process runs as nginxwebui. nginx (80/443) stays a root master
# if it was started above. reload falls back to nginxwebui-priv via sudo.
if id nginxwebui >/dev/null 2>&1; then
  chown -R nginxwebui:nginxwebui "$HOME_DIR" || true
  RUNUSER="runuser"
else
  RUNUSER=""
fi

set -- /opt/nginxwebui/nginxwebui
if [ -f "$USER_CONF" ]; then
  set -- "$@" --config="$USER_CONF"
fi
set -- "$@" --server.port="$PORT" --bind="$BIND" --project.home="$HOME_DIR" --web.dir="$WEB_DIR"

if [ -n "$RUNUSER" ]; then
  runuser -u nginxwebui -- env HOME="$HOME_DIR" "$@" &
else
  "$@" &
fi
child=$!
wait "$child"
