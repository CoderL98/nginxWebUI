#!/usr/bin/env bash
# 设置 Nginx 静态站点目录的标准权限
# 用法: sudo setup-web-permissions <站点名或域名>

set -euo pipefail

WEB_ROOT="/var/www"

usage() {
    cat <<EOF
用法: $0 <站点目录名或域名>

示例:
  $0 www.cudadog.com
  $0 blog.cudadog.com

说明:
  在 ${WEB_ROOT} 下创建目录并设置:
    - 属主: sudo 调用者（SUDO_USER），属组: www-data
    - 目录 755 + setgid（新建文件自动继承 www-data 组）
    - 文件 644
EOF
    exit 1
}

[[ $# -eq 1 ]] || usage

SITE_NAME="$1"

if [[ ! "$SITE_NAME" =~ ^[a-zA-Z0-9._-]+$ ]]; then
    echo "错误: 站点名只能包含字母、数字、点、下划线、连字符" >&2
    exit 1
fi

if [[ $EUID -ne 0 ]]; then
    echo "错误: 需要 root 权限，请使用 sudo 运行" >&2
    exit 1
fi

if ! getent group www-data >/dev/null; then
    echo "错误: 系统中不存在 www-data 组" >&2
    exit 1
fi

SITE_DIR="${WEB_ROOT}/${SITE_NAME}"
OWNER="${SUDO_USER:-root}"

# 根目录
[[ -d "$WEB_ROOT" ]] || mkdir -p "$WEB_ROOT"
chown root:www-data "$WEB_ROOT"
chmod 755 "$WEB_ROOT"

# 站点目录
if [[ -d "$SITE_DIR" ]]; then
    echo "目录已存在，修复权限: $SITE_DIR"
else
    mkdir -p "$SITE_DIR"
    echo "已创建目录: $SITE_DIR"
fi

chown -R "$OWNER":www-data "$SITE_DIR"
find "$SITE_DIR" -type d -exec chmod 755 {} \;
find "$SITE_DIR" -type f -exec chmod 644 {} \;
find "$SITE_DIR" -type d -exec chmod g+s {} \;

echo "完成: $SITE_DIR"
echo "  属主: ${OWNER}:www-data"
echo "  目录: 755 + setgid"
echo "  文件: 644"
