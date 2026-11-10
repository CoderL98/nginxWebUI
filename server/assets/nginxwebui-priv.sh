#!/usr/bin/env bash
# Privileged helper for nginxWebUI. Mutates only nginx / www / log paths.
# Installed root-owned at /usr/local/sbin/nginxwebui-priv by setup-sudo.sh.
set -euo pipefail

allowed_path() {
    local p="${1-}"
    [[ "$p" == /* ]] || return 1
    [[ "$p" != *..* ]] || return 1
    case "$p" in
        /etc/nginx|/etc/nginx/*) return 0 ;;
        /var/www|/var/www/*) return 0 ;;
        /var/log/nginx|/var/log/nginx/*) return 0 ;;
        *) return 1 ;;
    esac
}

cmd="${1-}"
shift || true

case "$cmd" in
    mkdir)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        mkdir -p "$1"
        ;;
    write)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        cat > "$1"
        ;;
    rm)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        rm -f "$1"
        ;;
    ln)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        allowed_path "${2-}" || { echo "path not allowed: ${2-}" >&2; exit 1; }
        ln -sfn "$1" "$2"
        ;;
    cp)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        allowed_path "${2-}" || { echo "path not allowed: ${2-}" >&2; exit 1; }
        cp -a "$1" "$2"
        ;;
    chown-www)
        allowed_path "${1-}" || { echo "path not allowed: ${1-}" >&2; exit 1; }
        user="${2-}"
        [[ "$user" =~ ^[a-zA-Z0-9._-]+$ ]] || { echo "bad user" >&2; exit 1; }
        if getent group www-data >/dev/null 2>&1; then
            chown "$user:www-data" "$1"
        else
            chown "$user" "$1"
        fi
        chmod 755 "$1"
        ;;
    *)
        echo "usage: nginxwebui-priv mkdir|write|rm|ln|cp|chown-www ..." >&2
        exit 1
        ;;
esac
