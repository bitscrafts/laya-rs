#!/usr/bin/env bash
# Verify the REAL crates hold the pii bar: zero clippy warnings, fmt clean, tests pass.
set -uo pipefail
export PATH="/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
cd /projects/laya-rs

echo "=== fmt ==="
cargo fmt --all >/dev/null 2>&1
cargo fmt --all --check && echo "fmt: CLEAN"

echo
echo "=== clippy (target 0) ==="
CLIP=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1)
echo "warning/error lines: $(printf '%s' "$CLIP" | grep -cE '^(warning|error)')"
printf '%s' "$CLIP" | grep -E '^(warning|error)' | head -12

echo
echo "=== tests ==="
cargo test -p laya-abi -p laya-core -p xtask 2>&1 | grep -E '^test result|^error' | head -8
