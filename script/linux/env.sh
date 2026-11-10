#!/usr/bin/env bash
# Shared paths for linux control scripts. Override with environment variables.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${NGINXWEBUI_PORT:-8080}"
HOME_DIR="${NGINXWEBUI_HOME:-$ROOT/.run-home}"
WEB_DIR="${NGINXWEBUI_WEB_DIR:-$ROOT/web/build}"
BIN_DIR="${NGINXWEBUI_BIN_DIR:-$HOME_DIR/bin}"
BIN="${NGINXWEBUI_BIN:-$BIN_DIR/nginxwebui}"
PID_FILE="${NGINXWEBUI_PID:-$HOME_DIR/nginxwebui.pid}"
LOG_FILE="${NGINXWEBUI_LOG:-$HOME_DIR/logs/nginxwebui.log}"
VITE_PID_FILE="${NGINXWEBUI_VITE_PID:-$HOME_DIR/vite.pid}"

mkdir -p "$HOME_DIR/logs" "$BIN_DIR"

is_running() {
    local pid_file="$1"
    [[ -f "$pid_file" ]] || return 1
    local pid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    [[ -n "$pid" ]] || return 1
    kill -0 "$pid" 2>/dev/null
}

stop_pidfile() {
    local pid_file="$1"
    local name="$2"
    if ! is_running "$pid_file"; then
        rm -f "$pid_file"
        echo "$name 未在运行"
        return 0
    fi
    local pid
    pid="$(cat "$pid_file")"
    echo "停止 $name pid=$pid"
    kill "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.3
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "进程未退出，发送 SIGKILL"
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$pid_file"
}
