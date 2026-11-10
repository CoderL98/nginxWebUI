#!/usr/bin/env bash
# Build the Rust backend and install the binary, then restart it.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

require_not_root

cd "$ROOT/server"
cargo build --release
cp "$ROOT/server/target/release/nginxwebui" "$BIN"
chmod 755 "$BIN"
echo "后端二进制: $BIN"
"$(cd "$(dirname "$0")" && pwd)/restart-server.sh"
