#!/usr/bin/env bash
# Build frontend + backend, copy web/build to web_dir, install the binary, restart.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
"$DIR/deploy-web.sh"
"$DIR/deploy-server.sh"
echo "部署完成"
