#!/usr/bin/env bash
# Deploy the freshly built binaries. Keeps the old ones as .old-<stamp>; never cp over a live binary.
set -euo pipefail
cd /projects/laya-rs
STAMP=$(date -u +%Y%m%dT%H%M%SZ)

echo "=== pre-deploy: old hashes vs the new build ==="
for b in laya laya-mcp laya-gate; do
  old=$(sha256sum /root/.local/bin/$b 2>/dev/null | cut -c1-16)
  new=$(sha256sum target/release/$b 2>/dev/null | cut -c1-16)
  printf "  %-10s old %s   new %s   %s\n" "$b" "$old" "$new" "$([ "$old" = "$new" ] && echo SAME || echo CHANGED)"
done

echo
echo "=== keeping the old binaries as .old-$STAMP ==="
for b in laya laya-mcp laya-gate; do
  [ -f "/root/.local/bin/$b" ] && cp -p "/root/.local/bin/$b" "/root/.local/bin/$b.old-$STAMP"
done
ls -1 /root/.local/bin/ | grep "old-$STAMP" | sed 's/^/  /'

echo
echo "=== installing (mv into place, never cp over a live binary) ==="
for b in laya laya-mcp laya-gate; do
  cp target/release/$b "/root/.local/bin/$b.new"
  mv -f "/root/.local/bin/$b.new" "/root/.local/bin/$b"
  chmod +x "/root/.local/bin/$b"
done

echo
echo "=== post-deploy verification ==="
for b in laya laya-mcp laya-gate; do
  dep=$(sha256sum /root/.local/bin/$b | cut -c1-16)
  src=$(sha256sum target/release/$b | cut -c1-16)
  printf "  %-10s deployed %s   tree %s   %s\n" "$b" "$dep" "$src" "$([ "$dep" = "$src" ] && echo MATCH || echo MISMATCH)"
done
