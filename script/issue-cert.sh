#!/usr/bin/env bash
# 为指定域名申请并安装 Let's Encrypt 证书（Cloudflare DNS 验证）
# 用法: issue-cert [--force] <域名> [域名2 ...]

set -euo pipefail

SITES_SSL_DIR="/etc/nginx/ssl"
CA_SERVER="letsencrypt"
ACME_HOME="${HOME}/.acme.sh"
ACME="${ACME_HOME}/acme.sh"

usage() {
    cat <<EOF
用法: $0 [--force] <域名> [域名2 ...]

示例:
  $0 hbbs.cudadog.com
  $0 hbbs.cudadog.com hbbr.cudadog.com
  $0 --force www.cudadog.com

说明:
  - 每个域名独立签发一张证书（不使用泛域名）
  - 使用 Cloudflare DNS 验证，需提前 export:
      CF_Token / CF_Account_ID
    或 ~/.acme.sh/account.conf 已保存凭据
  - 证书安装到 ${SITES_SSL_DIR}/<域名>.cer 和 .key
  - 安装后自动 reload nginx（nginx 未运行时用容错模式）
  - --force: 忽略缓存，强制重新签发
EOF
    exit 1
}

# ---------- 参数解析 ----------
FORCE=0
if [[ "${1:-}" == "--force" ]]; then
    FORCE=1
    shift
fi
[[ $# -ge 1 ]] || usage

FORCE_ARG=()
[[ $FORCE -eq 1 ]] && FORCE_ARG=(--force)

# ---------- 环境检查 ----------
[[ -x "$ACME" ]] || { echo "错误: 未找到 acme.sh: $ACME" >&2; exit 1; }

if [[ ! -d "$SITES_SSL_DIR" ]]; then
    echo "错误: 目录不存在: $SITES_SSL_DIR" >&2
    echo "请先执行:" >&2
    echo "  sudo mkdir -p $SITES_SSL_DIR && sudo chown \$USER:\$USER $SITES_SSL_DIR" >&2
    exit 1
fi
[[ -w "$SITES_SSL_DIR" ]] || { echo "错误: $SITES_SSL_DIR 不可写，请 chown 给当前用户" >&2; exit 1; }

# 凭据检查：环境变量或 account.conf 至少有一个
ACCOUNT_CONF="${ACME_HOME}/account.conf"
if [[ -z "${CF_Token:-}" && -z "${CF_Account_ID:-}" ]]; then
    if ! grep -q "SAVED_CF_Token" "$ACCOUNT_CONF" 2>/dev/null; then
        echo "错误: 未找到 Cloudflare 凭据" >&2
        echo "请先执行:" >&2
        echo "  export CF_Token=\"你的Token\"" >&2
        echo "  export CF_Account_ID=\"你的Account_ID\"" >&2
        exit 1
    fi
fi

# 域名格式校验
for DOMAIN in "$@"; do
    [[ "$DOMAIN" =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?)+$ ]] \
        || { echo "错误: 域名格式不合法: $DOMAIN" >&2; exit 1; }
done

# nginx 状态决定 reloadcmd
if systemctl is-active --quiet nginx 2>/dev/null; then
    RELOAD_CMD="sudo systemctl reload nginx"
else
    RELOAD_CMD="sudo systemctl reload nginx 2>/dev/null || true"
    echo "提示: nginx 未运行，reloadcmd 已设为容错模式"
fi

# ---------- 逐个处理 ----------
SUCCESS=()
FAILED=()

for DOMAIN in "$@"; do
    echo ""
    echo "=== $DOMAIN ==="

    CERT_FILE="${SITES_SSL_DIR}/${DOMAIN}.cer"
    KEY_FILE="${SITES_SSL_DIR}/${DOMAIN}.key"

    if ! "$ACME" --issue --dns dns_cf --server "$CA_SERVER" -d "$DOMAIN" "${FORCE_ARG[@]}"; then
        echo "错误: 签发失败: $DOMAIN" >&2
        FAILED+=("$DOMAIN")
        continue
    fi

    if "$ACME" --install-cert -d "$DOMAIN" \
        --key-file "$KEY_FILE" \
        --fullchain-file "$CERT_FILE" \
        --reloadcmd "$RELOAD_CMD"; then
        chmod 644 "$CERT_FILE" 2>/dev/null || true
        chmod 600 "$KEY_FILE"  2>/dev/null || true
        SUCCESS+=("$DOMAIN")
    else
        echo "错误: 安装失败: $DOMAIN" >&2
        FAILED+=("$DOMAIN")
    fi
done

# ---------- 汇总 ----------
echo ""
echo "=== 汇总 ==="
if [[ ${#SUCCESS[@]} -gt 0 ]]; then
    echo "成功: ${SUCCESS[*]}"
fi
if [[ ${#FAILED[@]} -gt 0 ]]; then
    echo "失败: ${FAILED[*]}"
fi

if [[ ${#SUCCESS[@]} -gt 0 ]]; then
    for DOMAIN in "${SUCCESS[@]}"; do
        echo ""
        echo "--- $DOMAIN ---"
        openssl x509 -in "${SITES_SSL_DIR}/${DOMAIN}.cer" -noout -subject -enddate 2>/dev/null || true
    done
fi

[[ ${#FAILED[@]} -eq 0 ]]
