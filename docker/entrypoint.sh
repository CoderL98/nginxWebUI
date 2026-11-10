#!/bin/sh
set -eu

PORT="${NGINXWEBUI_PORT:-8080}"
HOME_DIR="${NGINXWEBUI_HOME:-/home/nginxWebUI}"
WEB_DIR="${NGINXWEBUI_WEB_DIR:-/opt/nginxwebui/web}"
START_NGINX="${NGINXWEBUI_START_NGINX:-1}"
export NGINXWEBUI_RELOAD_CMD="${NGINXWEBUI_RELOAD_CMD:-nginx -s reload}"

mkdir -p \
  "$HOME_DIR/data" \
  "$HOME_DIR/logs" \
  /etc/nginx/sites-available \
  /etc/nginx/sites-enabled \
  /etc/nginx/ssl \
  /etc/nginx/conf.d \
  /var/www \
  /var/log/nginx

if [ ! -f /etc/nginx/ssl-params.conf ]; then
  cat >/etc/nginx/ssl-params.conf <<'EOF'
ssl_protocols TLSv1.2 TLSv1.3;
ssl_prefer_server_ciphers off;
ssl_session_timeout 1d;
ssl_session_cache shared:SSL:10m;
ssl_session_tickets off;
EOF
fi

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

/opt/nginxwebui/nginxwebui \
  --server.port="$PORT" \
  --project.home="$HOME_DIR" \
  --web.dir="$WEB_DIR" &
child=$!
wait "$child"
