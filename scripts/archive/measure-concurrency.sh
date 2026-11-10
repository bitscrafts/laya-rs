#!/usr/bin/env bash
# Measure the TWO contention points that decide P1's architecture.
#
# 1. Does the Laya server SERIALIZE concurrent requests, or parallelise them?
#    Laya on CPU is ~0.6s/call via this path. If N hooks fire at once they contend
#    on ONE llama.cpp server — that is a real queue, and priority there is valuable
#    (a `gate` blocking an agent should beat a background scan).
#
# 2. Does redb admit a second process? Blocker B4 says no. PROVE it rather than
#    assume — it is the difference between a design that works and one that fails
#    at first concurrent use.
set -uo pipefail
URL=http://10.10.10.217:8083

echo "=== 1. CONCURRENCY: 4 simultaneous /embedding requests ==="
body='{"content":["is this reversible"]}'
start=$(date +%s.%N)
for i in 1 2 3 4; do
  ( t0=$(date +%s.%N)
    code=$(curl -sS --max-time 60 -o /tmp/emb_$i.json -w '%{http_code}' \
      -X POST "$URL/embedding" -H 'Content-Type: application/json' -d "$body" 2>/dev/null)
    t1=$(date +%s.%N)
    printf '  req%d: HTTP %s  %.0f ms\n' "$i" "$code" "$(echo "($t1-$t0)*1000" | bc)"
  ) &
done
wait
end=$(date +%s.%N)
echo "  WALL TOTAL: $(echo "($end-$start)*1000" | bc | cut -d. -f1) ms"
echo "  -> if wall ~= max(individual), they SERIALIZE; if ~= max/N, they PARALLELISE"
echo
echo "=== 2. SEQUENTIAL baseline (one request) ==="
t0=$(date +%s.%N)
curl -sS --max-time 60 -o /tmp/emb_seq.json -X POST "$URL/embedding" \
  -H 'Content-Type: application/json' -d "$body" >/dev/null 2>&1
t1=$(date +%s.%N)
echo "  one request: $(echo "($t1-$t0)*1000" | bc | cut -d. -f1) ms"
echo
echo "=== 3. did the concurrent responses come back sane? ==="
for i in 1 2 3 4; do
  printf '  emb_%d.json: %s bytes, first key: %s\n' "$i" \
    "$(wc -c </tmp/emb_$i.json 2>/dev/null)" \
    "$(head -c 60 /tmp/emb_$i.json 2>/dev/null)"
done
