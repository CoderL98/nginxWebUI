#!/usr/bin/env bash
# Build frontend + backend and restart the server process.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
"$DIR/deploy-web.sh"
"$DIR/deploy-server.sh"
echo "部署完成"
