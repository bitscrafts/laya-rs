#!/usr/bin/env bash
# scripts/spec-all-gate.sh -- is EVERY spec finished? One compact line, exit status is the answer.
#
# This is the shell half of the graph in _docs/02-guides/spec-gate.flow.json. It exists because a graph
# is static and the spec set is not: the shell walks the specs, the graph decides and routes.
#
# FAST=1 (the default here) passes through to spec-gate.sh, which skips the criteria known to take
# minutes -- measured 2026-10-02, a full sweep ran over twenty minutes without finishing because three
# specs' criteria include the mutation gate at ~12 minutes each. A QUALITY check is not needed to answer
# a COMPLETENESS question; run the full form before a release.
set -uo pipefail
cd /projects/laya-rs
FAST="${FAST:-1}"
export FAST

done_n=0; open_n=0; open_list=""
for s in specs/[0-9]*.md; do
  [ -f "$s" ] || continue
  if scripts/spec-gate.sh "$s" >/dev/null 2>&1; then
    done_n=$((done_n+1))
  else
    open_n=$((open_n+1))
    open_list="$open_list $(basename "$s" .md | cut -c1-2)"
  fi
done

mode=""; [ "$FAST" = "1" ] && mode=" [fast: slow criteria skipped]"
if [ "$open_n" -eq 0 ]; then
  echo "spec set: ALL DONE ($done_n specs)${mode}"
  exit 0
fi
echo "spec set: $open_n NOT DONE of $((done_n+open_n)); open:${open_list}${mode}"
exit 1
