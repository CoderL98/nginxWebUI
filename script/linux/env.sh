#!/usr/bin/env bash
# Shared paths for linux control scripts. Values come from nginxwebui.conf,
# then environment variables, then these defaults.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

trim() {
    local s="${1-}"
    s="${s#"${s%%[![:space:]]*}"}"
    s="${s%"${s##*[![:space:]]}"}"
    printf '%s' "$s"
}

resolve_conf_path() {
    local base="$1"
    local val="$2"
    [[ -n "$val" ]] || return 0
    case "$val" in
        /*) printf '%s' "$val" ;;
        *) printf '%s' "$base/$val" ;;
    esac
}

load_conf_file() {
    local file="$1"
    local base
    base="$(cd "$(dirname "$file")" && pwd)"
    CONF_FILE="$base/$(basename "$file")"
    local line key val
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%%#*}"
        line="$(trim "$line")"
        [[ -n "$line" ]] || continue
        [[ "$line" == *=* ]] || continue
        key="$(trim "${line%%=*}")"
        val="$(trim "${line#*=}")"
        val="${val#\"}"
        val="${val%\"}"
        val="${val#\'}"
        val="${val%\'}"
        key="$(printf '%s' "$key" | tr 'A-Z-' 'a-z_')"
        case "$key" in
            port|server.port|server_port) CONF_PORT="$val" ;;
            home|project.home|project_home) CONF_HOME="$(resolve_conf_path "$base" "$val")" ;;
            web_dir|web.dir|web) CONF_WEB="$(resolve_conf_path "$base" "$val")" ;;
            threads) CONF_THREADS="$val" ;;
            reload_cmd|reload.cmd|reloadcmd) CONF_RELOAD_CMD="$val" ;;
            bin) CONF_BIN="$(resolve_conf_path "$base" "$val")" ;;
            log) CONF_LOG="$(resolve_conf_path "$base" "$val")" ;;
            pid) CONF_PID="$(resolve_conf_path "$base" "$val")" ;;
        esac
    done < "$file"
}

CONF_FILE=""
CONF_PORT=""
CONF_HOME=""
CONF_WEB=""
CONF_THREADS=""
CONF_RELOAD_CMD=""
CONF_BIN=""
CONF_LOG=""
CONF_PID=""

if [[ -n "${NGINXWEBUI_CONFIG:-}" && -f "$NGINXWEBUI_CONFIG" ]]; then
    load_conf_file "$NGINXWEBUI_CONFIG"
elif [[ -f "$ROOT/nginxwebui.conf" ]]; then
    load_conf_file "$ROOT/nginxwebui.conf"
elif [[ -f /etc/nginxwebui.conf ]]; then
    load_conf_file /etc/nginxwebui.conf
fi

PORT="${NGINXWEBUI_PORT:-${CONF_PORT:-8080}}"
HOME_DIR="${NGINXWEBUI_HOME:-${CONF_HOME:-$ROOT/.run-home}}"
WEB_DIR="${NGINXWEBUI_WEB_DIR:-${CONF_WEB:-$ROOT/web/build}}"
BIN_DIR="${NGINXWEBUI_BIN_DIR:-$HOME_DIR/bin}"
BIN="${NGINXWEBUI_BIN:-${CONF_BIN:-$BIN_DIR/nginxwebui}}"
PID_FILE="${NGINXWEBUI_PID:-${CONF_PID:-$HOME_DIR/nginxwebui.pid}}"
LOG_FILE="${NGINXWEBUI_LOG:-${CONF_LOG:-$HOME_DIR/logs/nginxwebui.log}}"
VITE_PID_FILE="${NGINXWEBUI_VITE_PID:-$HOME_DIR/vite.pid}"
THREADS="${NGINXWEBUI_THREADS:-${CONF_THREADS:-1}}"

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
