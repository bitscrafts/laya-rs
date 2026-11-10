#!/usr/bin/env bash
# Put the pinned upstream model package's SOURCE (no torch, no dependencies) where scripts/check-doc-citations.py looks for it,
# so a clean clone can verify the citations that point into it. Usage: scripts/fetch-model-source.sh [version]
set -eu
cd "$(dirname "${BASH_SOURCE[0]}")/.."
VER="${1:-0.3.5}"
SITE=".venv-laya/lib/python3/site-packages"
if [ -f "$SITE/laya/agent.py" ]; then echo "model source already present: $SITE/laya"; exit 0; fi
TMP="$(mktemp -d)"; trap 'mv "$TMP" "$TMP.done" 2>/dev/null || true' EXIT
python3 -m pip download "laya==$VER" --no-deps --no-binary :none: -d "$TMP" -q
mkdir -p "$SITE"
f="$(ls "$TMP"/*.whl 2>/dev/null | head -1)"
if [ -n "$f" ]; then python3 -m zipfile -e "$f" "$SITE"; else tar -xzf "$(ls "$TMP"/*.tar.gz | head -1)" -C "$TMP" && cp -r "$TMP"/laya-*/laya "$SITE/"; fi
test -f "$SITE/laya/agent.py" || { echo "FAILED: laya/agent.py not found after unpacking"; exit 1; }
echo "model source ready: $SITE/laya ($(wc -l < "$SITE/laya/agent.py") lines in agent.py)"
