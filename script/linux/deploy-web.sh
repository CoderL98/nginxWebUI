#!/usr/bin/env bash
# Build the static frontend into web/build. The Rust process reads these files
# from disk, so a running backend picks up a new build without a restart.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

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
echo "前端已部署: $WEB_DIR"
