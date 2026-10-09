#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

echo "=== 正在启动 tongyun-dev 本地控制台 ==="
cargo run -p tongyun-dev-cli -- "$@"
