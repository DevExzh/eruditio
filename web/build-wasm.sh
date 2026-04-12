#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODE="${1:---dev}"

echo "Building eruditio-wasm ($MODE)..."
wasm-pack build "$REPO_ROOT/crates/eruditio-wasm" \
  --target web \
  "$MODE" \
  --out-dir "$SCRIPT_DIR/src/lib/wasm"

echo "Done. WASM output in web/src/lib/wasm/"
