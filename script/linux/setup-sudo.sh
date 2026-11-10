#!/usr/bin/env bash
# Allow the nginxWebUI process user to write site files under /etc/nginx and /var/www.
# Usage: sudo script/linux/setup-sudo.sh
# Does not start the UI as root. The process stays a normal user.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

if [[ ${EUID} -ne 0 ]]; then
    echo "请用 root 运行: sudo $0" >&2
    exit 1
fi

USER_NAME="${SUDO_USER:-}"
if [[ -z "$USER_NAME" || "$USER_NAME" == "root" ]]; then
    echo "请从普通用户执行: sudo $0 （需要 SUDO_USER）" >&2
    exit 1
fi

SRC="$ROOT/server/assets/nginxwebui-priv.sh"
if [[ ! -f "$SRC" ]]; then
    echo "找不到助手脚本: $SRC" >&2
    exit 1
fi

DEST="/usr/local/sbin/nginxwebui-priv"
install -o root -g root -m 755 "$SRC" "$DEST"

CMDS=("$DEST")
add_cmd() {
    local spec="$1"
    local bin="${spec%% *}"
    [[ -n "$bin" && -x "$bin" ]] || return 0
    local i
    for i in "${CMDS[@]}"; do
        [[ "$i" == "$spec" ]] && return 0
    done
    CMDS+=("$spec")
}

SYSTEMCTL="$(command -v systemctl || true)"
NGINX_BIN="$(command -v nginx || true)"
if [[ -n "$SYSTEMCTL" ]]; then
    add_cmd "$SYSTEMCTL reload nginx"
fi
if [[ -n "$NGINX_BIN" ]]; then
    add_cmd "$NGINX_BIN -t"
    add_cmd "$NGINX_BIN -s reload"
fi
# Debian /usr/bin vs /bin
[[ -x /usr/bin/systemctl ]] && add_cmd "/usr/bin/systemctl reload nginx"
[[ -x /bin/systemctl ]] && add_cmd "/bin/systemctl reload nginx"
[[ -x /usr/sbin/nginx ]] && add_cmd "/usr/sbin/nginx -t" && add_cmd "/usr/sbin/nginx -s reload"

TMP="$(mktemp)"
{
    echo "# managed by nginxWebUI setup-sudo.sh — do not edit"
    echo "Defaults!$DEST !requiretty"
    for line in "${CMDS[@]}"; do
        printf '%s ALL=(root) NOPASSWD: %s\n' "$USER_NAME" "$line"
    done
} >"$TMP"

if ! visudo -cf "$TMP"; then
    rm -f "$TMP"
    echo "sudoers 语法检查失败" >&2
    exit 1
fi
install -o root -g root -m 440 "$TMP" /etc/sudoers.d/nginxwebui
rm -f "$TMP"

echo "已允许 $USER_NAME 免密执行："
printf '  %s\n' "${CMDS[@]}"
echo "进程仍用普通用户启动: $ROOT/script/linux/start-server.sh"
echo "不要用 sudo 启动 nginxWebUI。"
