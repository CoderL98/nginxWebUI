#!/usr/bin/env bash
# Build the static frontend into web/build, then copy it to web_dir from
# nginxwebui.conf. The Rust process serves that install directory.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

require_not_root

cd "$ROOT/web"
if ! command -v pnpm >/dev/null 2>&1; then
    if command -v corepack >/dev/null 2>&1; then
        corepack enable
        corepack prepare pnpm@11.24.0 --activate
    else
        echo "需要 pnpm（或 Node 自带的 corepack）" >&2
        exit 1
    fi
fi
if [[ -f pnpm-lock.yaml ]]; then
    pnpm install --frozen-lockfile
else
    pnpm install
fi
pnpm run build
sync_web_build
