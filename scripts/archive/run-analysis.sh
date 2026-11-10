#!/usr/bin/env bash
# Dispatch pi + glm-5.3 (FULL, not flash) for the laya-rs adversarial design review.
# RESUMABLE: re-run with the same SESSION_DIR to continue; FRESH=1 to archive first.
set -uo pipefail

source /root/.bashrc 2>/dev/null || true
source /root/.hermes/.env 2>/dev/null || true
export PATH="/root/.local/bin:/usr/local/cargo/bin:$PATH"

# pid 1 injects the PAID topup into every process — clear it, then set ours.
unset PI_PROVIDER PI_MODEL PI_THINKING
export PI_PROVIDER=ollama-cloud
export PI_MODEL="glm-5.3:cloud"          # FULL model, per user directive
export PI_THINKING=off                    # reasoning model: without this it emits 0 content

PROJ=/projects/laya-rs
SESSION_DIR="$PROJ/_tmp/pi-sessions/design-review"
BRIEF="$PROJ/_docs/06-research/02a-analysis-brief.md"
OUT="$PROJ/_docs/06-research/02-analysis-glm-5.3.md"
LOG="$PROJ/_tmp/analysis-run.log"

mkdir -p "$(dirname "$SESSION_DIR")" "$PROJ/_tmp"

# FRESH=1 archives the session dir so the worker genuinely starts over.
# Default (unset) REUSES the session — the standing rule.
if [ "${FRESH:-0}" = "1" ] && [ -d "$SESSION_DIR" ]; then
  mv "$SESSION_DIR" "$SESSION_DIR.old-$(date +%s)"
fi
mkdir -p "$SESSION_DIR"

echo "=== dispatch $(date -u '+%Y-%m-%dT%H:%M:%SZ') ===" | tee -a "$LOG"
echo "provider=$PI_PROVIDER model=$PI_MODEL thinking=$PI_THINKING" | tee -a "$LOG"
echo "session_dir=$SESSION_DIR" | tee -a "$LOG"
echo "brief=$BRIEF" | tee -a "$LOG"

# Sanity: the deliverable must NOT exist yet, or the worker may skip the work.
[ -f "$OUT" ] && echo "NOTE: $OUT already exists ($(wc -c <"$OUT") bytes) — will be overwritten" | tee -a "$LOG"

# FLAGS BEFORE -p (pi swallows flags placed after the prompt).
/root/.local/bin/pi \
  --provider ollama-cloud \
  --model "$PI_MODEL" \
  --thinking off \
  --session-dir "$SESSION_DIR" \
  --tools "read,write,edit,bash,grep,find,ls" \
  --max-tool-iterations 60 \
  --request-timeout 900 \
  -p "$(cat "$BRIEF")" \
  </dev/null 2>&1 | tee -a "$LOG"

RC=${PIPESTATUS[0]}
echo "=== pi exit rc=$RC $(date -u '+%H:%M:%SZ') ===" | tee -a "$LOG"

# --- verification: the session header is the ground truth of what pi resolved ---
echo "--- session header (provider/modelId actually used) ---" | tee -a "$LOG"
hdr=$(find "$SESSION_DIR" -name '*.jsonl' -type f 2>/dev/null | head -1)
if [ -n "$hdr" ]; then
  python3 -c "
import json,sys
p=sys.argv[1]
with open(p) as f:
    for line in f:
        try: o=json.loads(line)
        except Exception: continue
        if o.get('provider') or o.get('modelId'):
            print('  provider=%s modelId=%s' % (o.get('provider'), o.get('modelId'))); break
" "$hdr" | tee -a "$LOG"
  echo "  session file: $hdr ($(du -h "$hdr" | cut -f1))" | tee -a "$LOG"
else
  echo "  WARNING: no .jsonl session file found — pi may have failed before starting" | tee -a "$LOG"
fi

# --- verification: the deliverable must exist and be non-trivial ---
if [ -f "$OUT" ]; then
  echo "--- deliverable: $OUT ($(wc -l <"$OUT") lines, $(wc -c <"$OUT") bytes) ---" | tee -a "$LOG"
else
  echo "--- FAIL: $OUT was NOT written ---" | tee -a "$LOG"
fi

# --- verification: ANALYSIS scope means NO source edits ---
echo "--- scope check: .rs changes must be EMPTY ---" | tee -a "$LOG"
for d in /projects/laya-rs; do
  if [ -d "$d/.git" ]; then
    n=$(cd "$d" && git status --porcelain 2>/dev/null | grep '\.rs$' | wc -l)
    echo "  $d: $n changed .rs file(s)" | tee -a "$LOG"
  fi
done
find "$PROJ" -name '*.rs' -newermt '-2 hours' 2>/dev/null | tee -a "$LOG"

echo "=== done ===" | tee -a "$LOG"
