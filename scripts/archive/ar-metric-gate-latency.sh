#!/usr/bin/env bash
# ar-metric-gate-latency.sh — autoresearch metric for spec 30: p50 laya-gate latency in ms.
# v2 (defect fix): a FAILED gate call is a FAILED metric, never a fast sample — v1 swallowed
# the gate's rc, so a dead sidecar produced p50=2 (5x connection-refused in ~0.4ms each) and
# the loop kept a no-op edit with a bogus 70x improvement (ledger seq 21, 2edbede).
set -u
export LAYA_LEDGER_DIR="${LAYA_LEDGER_DIR:-/var/lib/laya-ledger}"
GATE="${GATE_BIN:-/root/.local/bin/laya-gate}"
times=()
for i in 1 2 3 4 5; do
  s=$(date +%s%N)
  "$GATE" "ls" --run-id "ar-metric-$i" >/dev/null 2>&1
  rc=$?
  e=$(date +%s%N)
  if [ "$rc" -ne 0 ]; then echo "metric: gate call $i failed rc=$rc" >&2; exit 1; fi
  times+=( $(( (e - s) / 1000000 )) )
done
# floor guard: every real decision includes a model forward (>50ms measured); below it the
# calls did NOT reach the head (stub env, socket refused) — fail, never celebrate.
p50=$(printf '%s\n' "${times[@]}" | sort -n | awk 'NR==3 { print $1 }')
if [ "$p50" -lt 50 ]; then echo "metric: p50=${p50}ms below the floor — gate did not reach the head" >&2; exit 1; fi
echo "$p50"