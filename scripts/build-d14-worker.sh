#!/usr/bin/env bash
# Build the Linux runtime input from this checkout in the selected environment.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/backend/target-linux}"
mkdir -p "$CARGO_TARGET_DIR"
export CARGO_TARGET_DIR="$(cd "$CARGO_TARGET_DIR" && pwd)"
cargo build --locked --release --manifest-path "$repo/backend/Cargo.toml" --bin agent-workspace-backend
mkdir -p "$repo/desktop/assets"
cp "$CARGO_TARGET_DIR/release/agent-workspace-backend" "$repo/desktop/assets/worker-linux-x86_64"
