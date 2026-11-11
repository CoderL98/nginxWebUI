#!/usr/bin/env bash
# Build the Rust backend and install the binary, then restart it.
set -euo pipefail
# shellcheck source=env.sh
source "$(cd "$(dirname "$0")" && pwd)/env.sh"

require_not_root

cd "$ROOT/server"
cargo build --release --bin nginxwebui --bin nginxwebui-agent
cp "$ROOT/server/target/release/nginxwebui" "$BIN"
chmod 755 "$BIN"
# 设备页的一键安装从面板旁边读取 agent（/agent/download）
AGENT_BIN="$(dirname "$BIN")/nginxwebui-agent"
cp "$ROOT/server/target/release/nginxwebui-agent" "$AGENT_BIN"
chmod 755 "$AGENT_BIN"
echo "后端二进制: $BIN"
echo "agent 二进制: $AGENT_BIN"
"$(cd "$(dirname "$0")" && pwd)/restart-server.sh"
