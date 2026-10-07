#!/usr/bin/env bash
# ar-metric-gate-latency.sh — autoresearch metric for spec 30: p50 laya-gate latency in ms.
# Prints ONE integer; the five calls are REAL gate decisions (ledger rows with decision_id,
# outcomes joinable — that is the calibration corpus by design).
set -u
export LAYA_LEDGER_DIR="${LAYA_LEDGER_DIR:-/var/lib/laya-ledger}"
GATE="${GATE_BIN:-/root/.local/bin/laya-gate}"
times=()
for i in 1 2 3 4 5; do
  s=$(date +%s%N)
  "$GATE" "ls" --run-id "ar-metric-$i" >/dev/null 2>&1
  e=$(date +%s%N)
  times+=( $(( (e - s) / 1000000 )) )
done
printf '%s\n' "${times[@]}" | sort -n | awk 'NR==3 { print $1 }'