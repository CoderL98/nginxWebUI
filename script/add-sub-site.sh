#!/usr/bin/env bash
# 添加二级域名网站配置（自动匹配泛域名证书）
# 用法: add-sub-site <域名> (--proxy PORT|URL | --root DIR) [选项]

set -euo pipefail

SITES_AVAILABLE="/etc/nginx/sites-available"
SITES_ENABLED="/etc/nginx/sites-enabled"
SSL_DIR="/etc/nginx/ssl"
SSL_PARAMS="/etc/nginx/ssl-params.conf"

DOMAIN=""
PROXY_TARGET=""
ROOT_DIR=""
CERT_NAME=""
ENABLE_WS=0
ENABLE_REDIRECT=1

usage() {
    cat <<EOF
用法: $0 <域名> (--proxy PORT|URL | --root DIR) [选项]

参数:
  <域名>                目标域名，如 cyhome.cdn.cudadog.com

目标（二选一）:
  --proxy PORT|URL      反代目标。PORT 自动转 127.0.0.1:PORT
                        URL 原样使用，如 http://10.0.0.1:8080
  --root DIR            静态站点根目录

选项:
  --cert NAME           指定证书文件名（不含 .cer），默认自动查找
  --ws                  反代时开启 WebSocket 支持
  --no-redirect         不生成 80 → 443 跳转

示例:
  $0 cyhome.cdn.cudadog.com --proxy 40207
  $0 cyhome.cdn.cudadog.com --proxy http://127.0.0.1:8080 --ws
  $0 cyhome.cdn.cudadog.com --root /var/www/cyhome.cdn.cudadog.com
  $0 cyhome.cdn.cudadog.com --proxy 40207 --cert cdn.cudadog.com

证书自动查找:
  按完整域名逐级去头查找 ${SSL_DIR}/<域名>.cer，
  并用 openssl 验证 SAN 是否覆盖目标域名。
  泛域名 *.cdn.cudadog.com 的证书文件名为 cdn.cudadog.com.cer
EOF
    exit 1
}

# ---------- 参数解析 ----------
[[ $# -ge 1 ]] || usage
DOMAIN="$1"; shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --proxy)       PROXY_TARGET="${2:-}"; shift 2 ;;
        --root)        ROOT_DIR="${2:-}"; shift 2 ;;
        --cert)        CERT_NAME="${2:-}"; shift 2 ;;
        --ws)          ENABLE_WS=1; shift ;;
        --no-redirect) ENABLE_REDIRECT=0; shift ;;
        -h|--help)     usage ;;
        *) echo "错误: 未知参数 $1" >&2; usage ;;
    esac
done

# ---------- 基本校验 ----------
[[ $EUID -eq 0 ]] || { echo "错误: 需要 root 权限，请使用 sudo 运行" >&2; exit 1; }

DOMAIN_RE='^(\*\.)?[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?)+$'
[[ "$DOMAIN" =~ $DOMAIN_RE && "$DOMAIN" != \** ]] \
    || { echo "错误: 域名格式不合法: $DOMAIN" >&2; exit 1; }

if [[ -n "$PROXY_TARGET" && -n "$ROOT_DIR" ]]; then
    echo "错误: --proxy 和 --root 只能选一个" >&2; exit 1
fi
if [[ -z "$PROXY_TARGET" && -z "$ROOT_DIR" ]]; then
    echo "错误: 必须指定 --proxy 或 --root" >&2; exit 1
fi

[[ -f "$SSL_PARAMS" ]] || { echo "错误: 缺少 ${SSL_PARAMS}" >&2; exit 1; }

CONF_AVAIL="${SITES_AVAILABLE}/${DOMAIN}.conf"
CONF_ENABLED="${SITES_ENABLED}/${DOMAIN}.conf"

if [[ -e "$CONF_AVAIL" || -L "$CONF_ENABLED" ]]; then
    echo "错误: 配置已存在，请先手动删除:" >&2
    echo "  $CONF_AVAIL" >&2
    echo "  $CONF_ENABLED" >&2
    exit 1
fi

# ---------- 证书覆盖判断 ----------
cert_covers() {
    local cert="$1" domain="$2"
    local sans
    sans=$(openssl x509 -in "$cert" -noout -ext subjectAltName 2>/dev/null \
        | grep -oP 'DNS:\K[^,]+' || true)
    [[ -z "$sans" ]] && return 1

    local san
    while IFS= read -r san; do
        san="$(echo "$san" | xargs)"
        [[ -z "$san" ]] && continue
        # 完全匹配
        if [[ "$san" == "$domain" ]]; then
            return 0
        fi
        # 泛域名匹配（仅一级子域）
        if [[ "$san" == \*.* ]]; then
            local suffix="${san#\*.}"
            if [[ "$domain" == *".${suffix}" ]]; then
                local prefix="${domain%.${suffix}}"
                if [[ -n "$prefix" && "$prefix" != *.* ]]; then
                    return 0
                fi
            fi
        fi
    done <<< "$sans"
    return 1
}

find_cert() {
    local domain="$1"
    local -a parts
    IFS='.' read -ra parts <<< "$domain"
    local n=${#parts[@]}

    local i j candidate cert
    for ((i=0; i<n-1; i++)); do
        candidate=""
        for ((j=i; j<n; j++)); do
            if [[ -z "$candidate" ]]; then
                candidate="${parts[j]}"
            else
                candidate="${candidate}.${parts[j]}"
            fi
        done
        cert="${SSL_DIR}/${candidate}.cer"
        if [[ -f "$cert" ]] && cert_covers "$cert" "$domain"; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

# ---------- 确定证书 ----------
if [[ -n "$CERT_NAME" ]]; then
    if [[ ! -f "${SSL_DIR}/${CERT_NAME}.cer" || ! -f "${SSL_DIR}/${CERT_NAME}.key" ]]; then
        echo "错误: 指定证书不存在: ${SSL_DIR}/${CERT_NAME}.{cer,key}" >&2
        exit 1
    fi
    if ! cert_covers "${SSL_DIR}/${CERT_NAME}.cer" "$DOMAIN"; then
        echo "错误: 证书 ${CERT_NAME}.cer 不覆盖域名 ${DOMAIN}" >&2
        echo "可用 SAN:" >&2
        openssl x509 -in "${SSL_DIR}/${CERT_NAME}.cer" -noout -ext subjectAltName >&2 || true
        exit 1
    fi
    echo "使用指定证书: ${CERT_NAME}.cer"
else
    if ! CERT_NAME=$(find_cert "$DOMAIN"); then
        echo "错误: 未找到覆盖 ${DOMAIN} 的证书" >&2
        echo "已尝试（逐级去头）:" >&2
        local_parts=( ${DOMAIN//./ } )
        n=${#local_parts[@]}
        for ((i=0; i<n-1; i++)); do
            cand=""
            for ((j=i; j<n; j++)); do
                [[ -z "$cand" ]] && cand="${local_parts[j]}" || cand="${cand}.${local_parts[j]}"
            done
            echo "  ${SSL_DIR}/${cand}.cer" >&2
        done
        echo "请先签发证书，或用 --cert 显式指定" >&2
        exit 1
    fi
    echo "自动匹配证书: ${CERT_NAME}.cer"
fi

CERT_PATH="${SSL_DIR}/${CERT_NAME}.cer"
KEY_PATH="${SSL_DIR}/${CERT_NAME}.key"

# ---------- 目标归一化 ----------
MODE=""
PROXY_URL=""
if [[ -n "$PROXY_TARGET" ]]; then
    MODE="proxy"
    if [[ "$PROXY_TARGET" =~ ^[0-9]+$ ]]; then
        PROXY_URL="http://127.0.0.1:${PROXY_TARGET}"
    elif [[ "$PROXY_TARGET" =~ ^https?:// ]]; then
        PROXY_URL="${PROXY_TARGET}"
    else
        PROXY_URL="http://${PROXY_TARGET}"
    fi
else
    MODE="static"
    [[ -d "$ROOT_DIR" ]] || {
        echo "警告: 静态目录不存在，创建: $ROOT_DIR"
        mkdir -p "$ROOT_DIR"
        chown "${SUDO_USER:-root}":www-data "$ROOT_DIR"
        chmod 755 "$ROOT_DIR"
    }
fi

# ---------- 生成配置 ----------
{
    if [[ $ENABLE_REDIRECT -eq 1 ]]; then
        cat <<EOF
server {
    listen 80;
    server_name ${DOMAIN};
    return 301 https://\$host\$request_uri;
}

EOF
    fi

    cat <<EOF
server {
    listen 443 ssl;
    http2 on;
    server_name ${DOMAIN};

    ssl_certificate     ${CERT_PATH};
    ssl_certificate_key ${KEY_PATH};
    include ${SSL_PARAMS};
EOF

    if [[ "$MODE" == "proxy" ]]; then
        cat <<EOF

    location / {
        proxy_pass ${PROXY_URL};
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
EOF
        if [[ $ENABLE_WS -eq 1 ]]; then
            cat <<EOF
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_buffering off;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
EOF
        fi
        cat <<EOF
    }

    access_log /var/log/nginx/${DOMAIN}.access.log;
    error_log  /var/log/nginx/${DOMAIN}.error.log;
}
EOF
    else
        cat <<EOF

    root ${ROOT_DIR};
    index index.html;

    location ~* \.html?\$ {
        expires -1;
        try_files \$uri =404;
    }

    location ~* \.(?:js|css|png|jpe?g|gif|ico|svg|webp|woff2?|ttf|eot)\$ {
        expires 1y;
        add_header Cache-Control "public, immutable";
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
    fi
} > "$CONF_AVAIL"

echo "已生成配置: $CONF_AVAIL"
ln -s "$CONF_AVAIL" "$CONF_ENABLED"

# ---------- 测试与回滚 ----------
if nginx -t; then
    systemctl reload nginx
    echo ""
    echo "完成: https://${DOMAIN}"
    echo "  证书: ${CERT_NAME}.cer"
    if [[ "$MODE" == "proxy" ]]; then
        echo "  反代: ${PROXY_URL}"
    else
        echo "  静态: ${ROOT_DIR}"
    fi
else
    echo "错误: nginx -t 失败，回滚" >&2
    rm -f "$CONF_ENABLED" "$CONF_AVAIL"
    exit 1
fi
