#!/usr/bin/env bash
# 为指定域名申请并安装 Let's Encrypt 证书（Gcore DNS 验证）
# 用法: issue-gcore-cert [--force] <域名> [域名2 ...]
#   域名可以是普通域名，也可以是 *.xxx 泛域名
#   *.xxx 会自动合并 xxx + *.xxx，一张证书覆盖两者

set -euo pipefail

SITES_SSL_DIR="/etc/nginx/ssl"
CA_SERVER="letsencrypt"
ACME_HOME="${HOME}/.acme.sh"
ACME="${ACME_HOME}/acme.sh"
ACCOUNT_CONF="${ACME_HOME}/account.conf"

usage() {
    cat <<EOF
用法: $0 [--force] <域名> [域名2 ...]

示例:
  $0 '*.cdn.cudadog.com'
  $0 cdn.cudadog.com
  $0 '*.cdn.cudadog.com' hbbs.cudadog.com
  $0 --force '*.cdn.cudadog.com'

说明:
  - 使用 Gcore DNS 验证，需提前 export:
      GCORE_Key="你的Gcore_API_Key"
    或 ~/.acme.sh/account.conf 已保存 SAVED_GCORE_Key
  - 普通域名独立签发一张证书
  - *.xxx 会自动合并 xxx + *.xxx，证书文件名为 xxx.cer
  - 证书安装到 ${SITES_SSL_DIR}/<证书名>.cer 和 .key
  - 安装后自动 reload nginx（nginx 未运行时容错）
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
if [[ -z "${GCORE_Key:-}" ]]; then
    if ! grep -q "SAVED_GCORE_Key" "$ACCOUNT_CONF" 2>/dev/null; then
        echo "错误: 未找到 Gcore 凭据" >&2
        echo "请先执行:" >&2
        echo "  export GCORE_Key=\"你的Gcore_API_Key\"" >&2
        exit 1
    fi
fi

# 域名格式校验（允许 *. 前缀）
DOMAIN_RE='^(\*\.)?[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?)+$'
for DOMAIN in "$@"; do
    [[ "$DOMAIN" =~ $DOMAIN_RE ]] \
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
declare -A DONE_CERTS=()

for DOMAIN in "$@"; do
    echo ""
    echo "=== $DOMAIN ==="

    # 泛域名：合并主域名，证书名去掉 *. 前缀
    if [[ "$DOMAIN" == \*.* ]]; then
        BASE_DOMAIN="${DOMAIN#\*.}"
        ISSUE_ARGS=(-d "$BASE_DOMAIN" -d "$DOMAIN")
        CERT_NAME="$BASE_DOMAIN"
        echo "泛域名模式: 一张证书覆盖 $BASE_DOMAIN 和 $DOMAIN"
    else
        ISSUE_ARGS=(-d "$DOMAIN")
        CERT_NAME="$DOMAIN"
    fi

    # 去重：同一张证书不重复签发
    if [[ -n "${DONE_CERTS[$CERT_NAME]:-}" ]]; then
        echo "跳过: $CERT_NAME 已处理过"
        continue
    fi
    DONE_CERTS[$CERT_NAME]=1

    CERT_FILE="${SITES_SSL_DIR}/${CERT_NAME}.cer"
    KEY_FILE="${SITES_SSL_DIR}/${CERT_NAME}.key"

    if ! "$ACME" --issue --dns dns_gcore --server "$CA_SERVER" \
            "${ISSUE_ARGS[@]}" "${FORCE_ARG[@]}"; then
        echo "错误: 签发失败: $DOMAIN" >&2
        FAILED+=("$DOMAIN")
        continue
    fi

    if "$ACME" --install-cert -d "$CERT_NAME" \
        --key-file "$KEY_FILE" \
        --fullchain-file "$CERT_FILE" \
        --reloadcmd "$RELOAD_CMD"; then
        chmod 644 "$CERT_FILE" 2>/dev/null || true
        chmod 600 "$KEY_FILE"  2>/dev/null || true
        SUCCESS+=("$CERT_NAME")
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
    for NAME in "${SUCCESS[@]}"; do
        echo ""
        echo "--- $NAME ---"
        openssl x509 -in "${SITES_SSL_DIR}/${NAME}.cer" \
            -noout -subject -ext subjectAltName -enddate 2>/dev/null || true
    done
fi

[[ ${#FAILED[@]} -eq 0 ]]
