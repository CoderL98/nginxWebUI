#!/usr/bin/env bash
# 为 Nginx 新增静态站点（HTTP 跳转 + HTTPS + 缓存策略）
# 用法: sudo add-static-site <域名> [root目录]

set -euo pipefail

SITES_AVAILABLE="/etc/nginx/sites-available"
SITES_ENABLED="/etc/nginx/sites-enabled"
SSL_PARAMS="/etc/nginx/ssl-params.conf"
DEFAULT_WEB_ROOT="/var/www"

usage() {
    cat <<EOF
用法: $0 <域名> [root目录]

示例:
  $0 www.cudadog.com
  $0 blog.cudadog.com /var/www/blog.cudadog.com

说明:
  - 证书必须已存在: /etc/nginx/ssl/<域名>.cer 和 .key（fullchain）
  - 默认 root: ${DEFAULT_WEB_ROOT}/<域名>
  - HTML / 不缓存；css/js/图 缓存 7 天（文件名无 hash，不用 immutable）
  - OCSP Stapling、sendfile、open_file_cache、gzip
  - 编译了 http_v3 时同时监听 QUIC
  - 生成后执行 nginx -t，失败自动回滚
EOF
    exit 1
}

[[ $# -ge 1 && $# -le 2 ]] || usage

DOMAIN="$1"
WEB_ROOT="${2:-${DEFAULT_WEB_ROOT}/${DOMAIN}}"

if [[ $EUID -ne 0 ]]; then
    echo "错误: 需要 root 权限，请使用 sudo 运行" >&2
    exit 1
fi

if [[ ! "$DOMAIN" =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?)+$ ]]; then
    echo "错误: 域名格式不合法: $DOMAIN" >&2
    exit 1
fi

CERT="/etc/nginx/ssl/${DOMAIN}.cer"
KEY="/etc/nginx/ssl/${DOMAIN}.key"
CONF_AVAIL="${SITES_AVAILABLE}/${DOMAIN}.conf"
CONF_ENABLED="${SITES_ENABLED}/${DOMAIN}.conf"

[[ -f "$SSL_PARAMS" ]] || { echo "错误: 缺少 ${SSL_PARAMS}" >&2; exit 1; }
[[ -f "$CERT" ]]       || { echo "错误: 证书不存在: $CERT" >&2; exit 1; }
[[ -f "$KEY" ]]        || { echo "错误: 私钥不存在: $KEY" >&2; exit 1; }

if [[ -e "$CONF_AVAIL" || -L "$CONF_ENABLED" ]]; then
    echo "错误: 配置已存在，请先手动删除:" >&2
    echo "  $CONF_AVAIL" >&2
    echo "  $CONF_ENABLED" >&2
    exit 1
fi

if [[ ! -d "$WEB_ROOT" ]]; then
    echo "警告: root 目录不存在，创建: $WEB_ROOT"
    mkdir -p "$WEB_ROOT"
    chown "${SUDO_USER:-root}":www-data "$WEB_ROOT"
    chmod 755 "$WEB_ROOT"
fi

LISTEN_QUIC=""
HTTP3_ON=""
ALT_SVC=""
if nginx -V 2>&1 | grep -q http_v3_module; then
    LISTEN_QUIC="    listen 443 quic;"
    HTTP3_ON="    http3 on;"
    ALT_SVC='        add_header Alt-Svc '\''h3=":443"; ma=86400'\'';'
fi

cat > "$CONF_AVAIL" <<EOF
server {
    listen 80;
    server_name ${DOMAIN};
    return 301 https://\$host\$request_uri;
}

server {
    listen 443 ssl;
    http2 on;
${LISTEN_QUIC}
${HTTP3_ON}
    server_name ${DOMAIN};

    ssl_certificate     ${CERT};
    ssl_certificate_key ${KEY};
    include ${SSL_PARAMS};

    ssl_stapling on;
    ssl_stapling_verify on;
    ssl_trusted_certificate ${CERT};
    resolver 223.5.5.5 8.8.8.8 valid=300s ipv6=off;
    resolver_timeout 2s;

    charset utf-8;
    sendfile on;
    tcp_nopush on;
    tcp_nodelay on;
    keepalive_timeout 65;

    open_file_cache max=2000 inactive=60s;
    open_file_cache_valid 30s;
    open_file_cache_min_uses 1;
    open_file_cache_errors on;

    root ${WEB_ROOT};
    index index.html;

    gzip on;
    gzip_vary on;
    gzip_proxied any;
    gzip_comp_level 5;
    gzip_min_length 256;
    gzip_types text/plain text/css text/xml application/javascript application/json application/xml image/svg+xml;

    # / 走 index.html，必须单独写，否则不会套上 HTML 的 Cache-Control
    location = / {
        add_header Cache-Control "no-cache";
        add_header Link "</style.css>; rel=preload; as=style, </homepage.js>; rel=preload; as=script";
${ALT_SVC}
        try_files /index.html =404;
    }

    location ~* \.html?\$ {
        add_header Cache-Control "no-cache";
        try_files \$uri =404;
    }

    location ~* \.(?:js|css|png|jpe?g|gif|ico|svg|webp|avif|woff2?|ttf|eot)\$ {
        expires 7d;
        add_header Cache-Control "public";
        access_log off;
        try_files \$uri =404;
    }

    location / {
        try_files \$uri \$uri/ =404;
    }

    access_log /var/log/nginx/${DOMAIN}.access.log;
    error_log  /var/log/nginx/${DOMAIN}.error.log;
}
EOF

echo "已生成配置: $CONF_AVAIL"
ln -s "$CONF_AVAIL" "$CONF_ENABLED"

if nginx -t; then
    systemctl reload nginx
    echo "完成: https://${DOMAIN} -> ${WEB_ROOT}"
else
    echo "错误: nginx -t 失败，回滚" >&2
    rm -f "$CONF_ENABLED" "$CONF_AVAIL"
    exit 1
fi
