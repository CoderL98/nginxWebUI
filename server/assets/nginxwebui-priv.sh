#!/usr/bin/env bash
# Privileged helper for nginxWebUI. Mutates only nginx / www / log paths.
# Installed root-owned at /usr/local/sbin/nginxwebui-priv by setup-sudo.sh.
# Must run as root via sudo -n; never world-writable; paths go through realpath.
set -euo pipefail

EXPECTED="/usr/local/sbin/nginxwebui-priv"

die() {
    echo "$*" >&2
    exit 1
}

canonical() {
    local p="${1-}"
    [[ "$p" == /* ]] || return 1
    realpath -m -- "$p" 2>/dev/null
}

# Follow existing last-component symlink (write/mkdir/chown/cp).
canonical_follow() {
    canonical "${1-}"
}

# Keep last component so ln/rm can replace a symlink instead of its target.
canonical_last() {
    local p="${1-}" dir base
    [[ "$p" == /* ]] || return 1
    dir="$(canonical "$(dirname -- "$p")")" || return 1
    base="$(basename -- "$p")"
    [[ -n "$base" && "$base" != / && "$base" != . && "$base" != .. ]] || return 1
    printf '%s/%s' "$dir" "$base"
}

in_tree() {
    local p="${1-}"
    case "$p" in
        /etc/nginx|/etc/nginx/*) return 0 ;;
        /var/www|/var/www/*) return 0 ;;
        /var/log/nginx|/var/log/nginx/*) return 0 ;;
        /run/nginx.pid|/var/run/nginx.pid) return 0 ;;
        *) return 1 ;;
    esac
}

require_path() {
    local mode="${1-}" raw="${2-}" p
    [[ "$raw" == /* ]] || die "path not allowed: ${raw:-<empty>}"
    case "$mode" in
        follow) p="$(canonical_follow "$raw")" || die "path not allowed: $raw" ;;
        last) p="$(canonical_last "$raw")" || die "path not allowed: $raw" ;;
        *) die "internal: bad resolve mode" ;;
    esac
    in_tree "$p" || die "path not allowed: $raw -> $p"
    printf '%s' "$p"
}

check_self() {
    [[ ${EUID} -eq 0 ]] || die "nginxwebui-priv must run as root via sudo"
    local self uid mode
    self="$(readlink -f -- "$0" 2>/dev/null || canonical "$0")"
    [[ "$self" == "$EXPECTED" ]] || die "helper must be $EXPECTED"
    [[ ! -L "$EXPECTED" ]] || die "helper must not be a symlink"
    uid="$(stat -c '%u' "$EXPECTED")"
    mode="$(stat -c '%a' "$EXPECTED")"
    [[ "$uid" == "0" ]] || die "helper must be root-owned"
    [[ $((8#$mode & 022)) -eq 0 ]] || die "helper must not be group/world writable (mode $mode)"
}

nginx_bin() {
    if [[ -x /usr/sbin/nginx ]]; then
        printf '%s' /usr/sbin/nginx
    elif command -v nginx >/dev/null 2>&1; then
        command -v nginx
    else
        die "nginx not found"
    fi
}

check_self

cmd="${1-}"
shift || true

case "$cmd" in
    mkdir)
        p="$(require_path follow "${1-}")"
        mkdir -p "$p"
        ;;
    write)
        p="$(require_path follow "${1-}")"
        cat > "$p"
        ;;
    rm)
        p="$(require_path last "${1-}")"
        rm -f "$p"
        ;;
    ln)
        target="$(require_path follow "${1-}")"
        link="$(require_path last "${2-}")"
        ln -sfn "$target" "$link"
        ;;
    cp)
        src="$(require_path follow "${1-}")"
        dst="$(require_path follow "${2-}")"
        cp -a "$src" "$dst"
        ;;
    chown-www)
        p="$(require_path follow "${1-}")"
        user="${2-}"
        [[ "$user" =~ ^[a-zA-Z0-9._-]+$ ]] || die "bad user"
        if getent group www-data >/dev/null 2>&1; then
            chown "$user:www-data" "$p"
        else
            chown "$user" "$p"
        fi
        chmod 755 "$p"
        ;;
    nginx-t)
        bin="$(nginx_bin)"
        if [[ -n "${1-}" ]]; then
            conf="$(require_path follow "$1")"
            "$bin" -t -c "$conf"
        else
            "$bin" -t
        fi
        ;;
    nginx-s)
        sig="${1-}"
        case "$sig" in
            reload|stop|quit) ;;
            *) die "bad nginx signal: ${sig:-<empty>}" ;;
        esac
        "$(nginx_bin)" -s "$sig"
        ;;
    nginx-start)
        if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
            systemctl start nginx
        else
            "$(nginx_bin)"
        fi
        ;;
    systemctl)
        act="${1-}"
        case "$act" in
            start|stop|reload|enable|status) ;;
            *) die "bad systemctl action: ${act:-<empty>}" ;;
        esac
        systemctl "$act" nginx
        ;;
    read)
        p="$(require_path follow "${1-}")"
        [[ -f "$p" ]] || die "not a file: $p"
        cat -- "$p"
        ;;
    tail)
        p="$(require_path follow "${1-}")"
        n="${2-65536}"
        [[ "$n" =~ ^[0-9]+$ ]] || die "bad tail size"
        if (( n < 1 )); then n=1; fi
        if (( n > 1048576 )); then n=1048576; fi
        [[ -f "$p" ]] || die "not a file: $p"
        tail -c "$n" -- "$p"
        ;;
    ls)
        p="$(require_path follow "${1-}")"
        [[ -d "$p" ]] || die "not a directory: $p"
        ls -1A -- "$p"
        ;;
    empty-dir)
        p="$(require_path follow "${1-}")"
        case "$p" in
            /var/www/*) ;;
            *) die "empty-dir only under /var/www/<site>" ;;
        esac
        [[ "$p" != /var/www && "$p" != /var/www/ ]] || die "refusing to empty /var/www"
        find "$p" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
        ;;
    chmod)
        p="$(require_path follow "${1-}")"
        mode="${2-}"
        case "$mode" in
            644|640|600|755|750) ;;
            *) die "bad mode: ${mode:-<empty>}" ;;
        esac
        chmod "$mode" "$p"
        ;;
    *)
        echo "usage: nginxwebui-priv mkdir|write|rm|ln|cp|chown-www|nginx-t|nginx-s|nginx-start|systemctl|read|tail|ls|empty-dir|chmod ..." >&2
        exit 1
        ;;
esac
