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
PRIV_HELPER="${NGINXWEBUI_PRIV:-/usr/local/sbin/nginxwebui-priv}"

in_docker() {
    [[ -f /.dockerenv ]] && return 0
    grep -qE '(docker|lxc|containerd|/kubepods)' /proc/1/cgroup 2>/dev/null
}

# 不要用 sudo 启动整个进程。数据目录、pid、日志会变成 root 属主；
# 管理界面一旦被打穿就是整机 root。写 nginx 配置走白名单助手。
require_not_root() {
    if [[ ${EUID} -ne 0 ]]; then
        return 0
    fi
    if [[ "${NGINXWEBUI_ALLOW_ROOT:-}" == "1" ]] || in_docker; then
        echo "以 root 运行（容器或 NGINXWEBUI_ALLOW_ROOT=1）"
        return 0
    fi
    echo "不要用 sudo 启动 nginxWebUI。" >&2
    echo "进程用普通用户跑；写 /etc/nginx 只授权助手：" >&2
    echo "  sudo $ROOT/script/linux/setup-sudo.sh" >&2
    echo "若以前用 root 启动过，先把数据目录改回当前用户：" >&2
    echo "  sudo chown -R \"\$USER:\$USER\" $HOME_DIR" >&2
    echo "容器或排障才设 NGINXWEBUI_ALLOW_ROOT=1" >&2
    exit 1
}

require_nginx_write() {
    local sites=/etc/nginx/sites-available
    if [[ ! -d /etc/nginx ]]; then
        return 0
    fi
    if [[ -d "$sites" && -w "$sites" ]]; then
        echo "站点配置可直接写入 $sites"
        return 0
    fi
    if [[ -d /etc/nginx && -w /etc/nginx ]]; then
        echo "站点配置可直接写入 /etc/nginx"
        return 0
    fi
    if [[ -x "$PRIV_HELPER" ]] && sudo -n "$PRIV_HELPER" mkdir "$sites" >/dev/null 2>&1; then
        echo "站点配置将通过 sudo -n $PRIV_HELPER 写入 /etc/nginx"
        return 0
    fi
    echo "当前用户写不了 /etc/nginx，且免密助手未就绪。" >&2
    echo "请先运行: sudo $ROOT/script/linux/setup-sudo.sh" >&2
    echo "不要用 sudo $0 启动本进程。" >&2
    exit 1
}

# setup-sudo.sh 以 root 引用本文件，不能在那时建数据目录。
if [[ ${EUID} -ne 0 ]] || [[ "${NGINXWEBUI_ALLOW_ROOT:-}" == "1" ]] || in_docker; then
    mkdir -p "$HOME_DIR/logs" "$BIN_DIR"
fi

# Vite 写到仓库 web/build；deploy 再同步到配置的 web_dir。
BUILD_DIR="$ROOT/web/build"

sync_web_build() {
    if [[ ! -f "$BUILD_DIR/index.html" ]]; then
        echo "找不到构建结果: $BUILD_DIR/index.html" >&2
        echo "请先在仓库里成功执行 pnpm run build" >&2
        return 1
    fi
    if [[ -z "$WEB_DIR" || "$WEB_DIR" == "/" ]]; then
        echo "web_dir 无效: ${WEB_DIR:-<空>}" >&2
        return 1
    fi
    local src dst src_root
    src="$(cd "$BUILD_DIR" && pwd -P)"
    src_root="$(cd "$ROOT/web" && pwd -P)"
    mkdir -p "$WEB_DIR"
    dst="$(cd "$WEB_DIR" && pwd -P)"
    if [[ "$dst" == "$src_root" ]]; then
        echo "web_dir 不能是前端源码目录 $ROOT/web" >&2
        echo "请在 nginxwebui.conf 里改成安装目录，例如 $HOME_DIR/web" >&2
        return 1
    fi
    if [[ "$src" == "$dst" ]]; then
        echo "前端已在配置路径: $dst"
        return 0
    fi
    echo "同步前端 $src -> $dst"
    if command -v rsync >/dev/null 2>&1; then
        rsync -a --delete "$src"/ "$dst"/
    else
        find "$dst" -mindepth 1 -maxdepth 1 -exec rm -rf {} + || true
        cp -a "$src"/. "$dst"/
    fi
    if [[ ! -f "$dst/index.html" ]]; then
        echo "同步失败，没有 $dst/index.html" >&2
        return 1
    fi
    echo "前端已部署: $dst"
}

pid_alive() {
    local pid="${1-}"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [[ -d "/proc/$pid" ]]
}

is_running() {
    local pid_file="$1"
    [[ -f "$pid_file" ]] || return 1
    local pid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    pid_alive "$pid"
}

port_busy() {
    local port="${1:-$PORT}"
    [[ -n "$port" ]] || return 1
    if command -v ss >/dev/null 2>&1; then
        ss -ltnH "sport = :$port" 2>/dev/null | grep -q .
        return
    fi
    if command -v fuser >/dev/null 2>&1; then
        fuser "${port}/tcp" >/dev/null 2>&1
        return
    fi
    return 1
}

our_bin_pids() {
    local want p pid exe
    [[ -e "$BIN" ]] || return 0
    want="$(readlink -f "$BIN" 2>/dev/null || printf '%s' "$BIN")"
    [[ -n "$want" ]] || return 0
    for p in /proc/[0-9]*; do
        pid="${p##*/}"
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        exe="$(readlink "$p/exe" 2>/dev/null || true)"
        [[ -n "$exe" ]] || continue
        if [[ "$exe" == "$want" || "$exe" == "$want (deleted)" ]]; then
            printf '%s\n' "$pid"
        fi
    done
}

port_hint() {
    echo "  sudo ss -tlnp | grep \":$PORT\"" >&2
    echo "  sudo fuser -k ${PORT}/tcp" >&2
}

stop_one_pid() {
    local pid="$1"
    local name="$2"
    pid_alive "$pid" || return 0
    echo "停止 $name pid=$pid"
    if ! kill "$pid" 2>/dev/null; then
        echo "当前用户杀不了 pid=$pid（多半是以前 sudo 启动留下的）。" >&2
        echo "请执行: sudo kill $pid" >&2
        port_hint
        return 1
    fi
    local _
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pid_alive "$pid" || break
        sleep 0.3
    done
    if pid_alive "$pid"; then
        echo "进程未退出，发送 SIGKILL"
        kill -9 "$pid" 2>/dev/null || true
        sleep 0.2
    fi
    if pid_alive "$pid"; then
        echo "pid=$pid 仍在运行，请: sudo kill -9 $pid" >&2
        port_hint
        return 1
    fi
    return 0
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
    if stop_one_pid "$pid" "$name"; then
        rm -f "$pid_file"
        return 0
    fi
    return 1
}

stop_backend() {
    local pid found=0 fail=0
    if [[ -f "$PID_FILE" ]]; then
        pid="$(cat "$PID_FILE" 2>/dev/null || true)"
        if pid_alive "$pid"; then
            found=1
            stop_one_pid "$pid" "后端" || fail=1
        fi
        if [[ $fail -eq 0 ]]; then
            rm -f "$PID_FILE"
        fi
    fi
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        found=1
        stop_one_pid "$pid" "后端(残留)" || fail=1
    done < <(our_bin_pids || true)
    if [[ $fail -ne 0 ]]; then
        return 1
    fi
    if port_busy; then
        echo "端口 $PORT 仍被占用。pid 文件对不上，且当前用户看不到属主（多半是 root 残留）。" >&2
        port_hint
        return 1
    fi
    if [[ $found -eq 0 ]]; then
        echo "后端 未在运行"
    fi
    return 0
}
