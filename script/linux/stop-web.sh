#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"
stop_pidfile "$VITE_PID_FILE" "前端开发服务"
# pnpm run dev may leave a child on 5173
if command -v fuser >/dev/null 2>&1; then
    fuser -k 5173/tcp 2>/dev/null || true
elif command -v lsof >/dev/null 2>&1; then
    pid="$(lsof -t -iTCP:5173 -sTCP:LISTEN 2>/dev/null || true)"
    if [[ -n "${pid:-}" ]]; then
        kill $pid 2>/dev/null || true
    fi
fi
