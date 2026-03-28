#!/usr/bin/env bash
# Build the React SPA frontend for mm-web.
# Usage: ./scripts/build-web.sh
#
# Prerequisites: Node.js 18+

set -euo pipefail

cd "$(git rev-parse --show-toplevel)/crates/mm-web/frontend"

if [ ! -d node_modules ]; then
    echo "Installing dependencies..."
    npm ci
fi

echo "Building React frontend..."
npm run build

echo "Done. Output at crates/mm-web/frontend/dist/"
echo "Start server: cargo run -p mm-web"
