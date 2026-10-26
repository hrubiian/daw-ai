#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --workspace --locked
python3 scripts/test-mcp-protocol.py
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
