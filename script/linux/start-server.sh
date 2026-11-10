#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

require_not_root

if is_running "$PID_FILE"; then
    echo "后端已在运行 pid=$(cat "$PID_FILE")  http://127.0.0.1:$PORT"
    exit 0
fi
rm -f "$PID_FILE"

orphans="$(our_bin_pids || true)"
if [[ -n "$orphans" ]] || port_busy; then
    echo "发现残留后端或端口 $PORT 仍被占用，先停止"
    stop_backend
fi
if port_busy; then
    echo "无法绑定 0.0.0.0:$PORT，地址已被占用。" >&2
    echo "stop.sh 只靠 pid 文件时会漏掉这类进程。" >&2
    port_hint
    exit 1
fi

if [[ ! -x "$BIN" ]]; then
    echo "找不到可执行文件: $BIN" >&2
    echo "请先运行: $(dirname "$0")/deploy-server.sh" >&2
    exit 1
fi
if [[ ! -f "$WEB_DIR/index.html" ]]; then
    echo "找不到前端页面: $WEB_DIR/index.html" >&2
    echo "请先运行: $(dirname "$0")/deploy-web.sh" >&2
    echo "（会把 $ROOT/web/build 同步到配置里的 web_dir）" >&2
    exit 1
fi
require_nginx_write

ARGS=(--server.port="$PORT" --project.home="$HOME_DIR" --web.dir="$WEB_DIR")
if [[ -n "${CONF_FILE:-}" ]]; then
    ARGS=(--config="$CONF_FILE" "${ARGS[@]}")
fi
if [[ -n "${THREADS:-}" ]]; then
    ARGS+=(--threads="$THREADS")
fi
nohup "$BIN" "${ARGS[@]}" >>"$LOG_FILE" 2>&1 &
echo $! >"$PID_FILE"
sleep 0.4
if is_running "$PID_FILE"; then
    echo "后端已启动 pid=$(cat "$PID_FILE")"
    echo "  http://127.0.0.1:$PORT"
    [[ -n "${CONF_FILE:-}" ]] && echo "  config=$CONF_FILE"
    echo "  home=$HOME_DIR"
    echo "  log=$LOG_FILE"
else
    echo "启动失败，查看日志: $LOG_FILE" >&2
    rm -f "$PID_FILE"
    exit 1
fi
