#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

if is_running "$PID_FILE"; then
    echo "后端已在运行 pid=$(cat "$PID_FILE")  http://127.0.0.1:$PORT"
    exit 0
fi
if [[ ! -x "$BIN" ]]; then
    echo "找不到可执行文件: $BIN" >&2
    echo "请先运行: $(dirname "$0")/deploy-server.sh" >&2
    exit 1
fi
if [[ ! -f "$WEB_DIR/index.html" ]]; then
    echo "找不到前端页面: $WEB_DIR/index.html" >&2
    echo "请先运行: $(dirname "$0")/deploy-web.sh" >&2
    exit 1
fi

nohup "$BIN" \
    --server.port="$PORT" \
    --project.home="$HOME_DIR" \
    --web.dir="$WEB_DIR" \
    >>"$LOG_FILE" 2>&1 &
echo $! >"$PID_FILE"
sleep 0.4
if is_running "$PID_FILE"; then
    echo "后端已启动 pid=$(cat "$PID_FILE")"
    echo "  http://127.0.0.1:$PORT"
    echo "  home=$HOME_DIR"
    echo "  log=$LOG_FILE"
else
    echo "启动失败，查看日志: $LOG_FILE" >&2
    rm -f "$PID_FILE"
    exit 1
fi
