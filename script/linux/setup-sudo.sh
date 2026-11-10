#!/usr/bin/env bash
# Allow the nginxWebUI process user to write site files under /etc/nginx and /var/www.
# Usage: sudo script/linux/setup-sudo.sh
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

TMP="$(mktemp)"
printf '%s ALL=(root) NOPASSWD: %s\n' "$USER_NAME" "$DEST" >"$TMP"
if ! visudo -cf "$TMP"; then
    rm -f "$TMP"
    echo "sudoers 语法检查失败" >&2
    exit 1
fi
install -o root -g root -m 440 "$TMP" /etc/sudoers.d/nginxwebui
rm -f "$TMP"

echo "已允许 $USER_NAME 免密执行 $DEST"
echo "新增站点会写入 /etc/nginx/sites-available 和 /var/www。"
echo "然后执行: $ROOT/script/linux/restart-server.sh"
