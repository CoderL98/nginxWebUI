#!/usr/bin/env bash
# Optional: Vite dev server (port 5173). Production does not need this —
# the Rust process already serves web/build.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

if is_running "$VITE_PID_FILE"; then
    echo "前端开发服务已在运行 pid=$(cat "$VITE_PID_FILE")  http://127.0.0.1:5173"
    exit 0
fi
cd "$ROOT/web"
if ! command -v pnpm >/dev/null 2>&1; then
    if command -v corepack >/dev/null 2>&1; then
        corepack enable
        corepack prepare pnpm@11.24.0 --activate
    else
        echo "需要 pnpm（或 Node 自带的 corepack）" >&2
        exit 1
    fi
fi
if [[ ! -d node_modules ]]; then
    pnpm install
fi
nohup pnpm run dev >>"$HOME_DIR/logs/vite.log" 2>&1 &
echo $! >"$VITE_PID_FILE"
sleep 0.8
echo "前端开发服务已启动 pid=$(cat "$VITE_PID_FILE")"
echo "  http://127.0.0.1:5173  (API 代理到 :$PORT，请同时 start-server.sh)"
echo "  log=$HOME_DIR/logs/vite.log"
