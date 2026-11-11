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
AGENT_DIR="$(dirname "$BIN")"
AGENT_BIN="$AGENT_DIR/nginxwebui-agent"
cp "$ROOT/server/target/release/nginxwebui-agent" "$AGENT_BIN"
chmod 755 "$AGENT_BIN"
OS="$(uname -s | tr 'A-Z' 'a-z')"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
esac
AGENT_SLOT="$AGENT_DIR/agents/${OS}-${ARCH}"
mkdir -p "$AGENT_SLOT"
cp "$AGENT_BIN" "$AGENT_SLOT/nginxwebui-agent"
chmod 755 "$AGENT_SLOT/nginxwebui-agent"
echo "后端二进制: $BIN"
echo "agent 二进制: $AGENT_BIN"
echo "agent 多架构目录: $AGENT_SLOT/nginxwebui-agent"
echo "其它架构请放到 $AGENT_DIR/agents/<os>-<arch>/nginxwebui-agent"
"$(cd "$(dirname "$0")" && pwd)/restart-server.sh"
