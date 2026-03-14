#!/usr/bin/env bash
# Build the WASM client and place the bundle where mm-web serves it.
# Usage: ./scripts/build-web.sh [--release]
#
# Prerequisites: rustup target add wasm32-unknown-unknown && cargo install wasm-pack

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

PROFILE_FLAG=""
if [[ "${1:-}" == "--release" ]]; then
    PROFILE_FLAG="--release"
fi

# Ensure wasm target is available
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "Adding wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

# Ensure wasm-pack is installed
if ! command -v wasm-pack &>/dev/null; then
    echo "Installing wasm-pack..."
    cargo install wasm-pack
fi

echo "Building WASM client..."
wasm-pack build crates/mm-web/client \
    --target web \
    --out-dir ../static/pkg \
    $PROFILE_FLAG

echo "Done. Bundle at crates/mm-web/static/pkg/"
echo "Start server: cargo run -p mm-web"
