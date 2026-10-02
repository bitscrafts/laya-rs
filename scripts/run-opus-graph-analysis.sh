#!/usr/bin/env bash
# run-opus-graph-analysis.sh -- dispatch the generic-graphs analysis to claude + opus.
#
# WHY A SCRIPT (HARD RULE 0d): a dispatch is the most expensive action this project takes and it
# must be reviewable before it runs and re-runnable identically after it fails. Background shells
# do not inherit the interactive environment, so PATH and the model must be set INSIDE here.
#
# WHY A NAMED, REATTACHABLE SESSION (user rule, 2026-09-21, verbatim):
#   "we should never use claude -p (one run) as this way we cant reattach to the session!!! so we
#    could use the previous context to further analysis!"
# So this prints the exact resume command and records the session id. Analysis accumulates; it is
# NOT a fire-and-forget one-shot. (The anti-resume rule applies to WORKER loops relaunched N times,
# not to an analysis a human will extend.)
#
# WHY OPUS AND NOT SONNET: this is judgement work on an architecture question -- the account tier
# per the cost policy (Anthropic subscription via Claude Code), and the highest-leverage spend.
#
# USAGE
#   scripts/run-opus-graph-analysis.sh              # start
#   scripts/run-opus-graph-analysis.sh --resume     # continue the recorded session
#   scripts/run-opus-graph-analysis.sh --list       # what has been dispatched
set -uo pipefail

export PATH="/root/.local/bin:/usr/local/cargo/bin:$PATH"

REPO=/projects/laya-rs
BRIEF=/projects/_tmp/brief-opus-graph-analysis.md
OUTDIR=/projects/_tmp/opus-graph-analysis
STATE=$OUTDIR/session.id
mkdir -p "$OUTDIR"

case "${1:-}" in
  --list)
    echo "brief   : $BRIEF  ($( [ -f "$BRIEF" ] && wc -c < "$BRIEF" || echo MISSING) bytes)"
    echo "outdir  : $OUTDIR"
    echo "session : $( [ -f "$STATE" ] && cat "$STATE" || echo '<none>')"
    ls -la "$OUTDIR" 2>/dev/null | tail -n +4
    exit 0
    ;;
esac

# --- preflight: fail in one second, not after a budget is spent -------------------------------
[ -f "$BRIEF" ] || { echo "!! brief missing: $BRIEF" >&2; exit 2; }
[ -d "$REPO" ]  || { echo "!! repo missing: $REPO" >&2; exit 2; }
BRIEF_BYTES=$(wc -c < "$BRIEF")

echo "=============================================================="
echo "  model   : opus (claude CLI $(claude --version 2>/dev/null | head -1))"
echo "  repo    : $REPO"
echo "  brief   : $BRIEF  ($BRIEF_BYTES bytes, delivered as a POINTER not inlined)"
echo "  outdir  : $OUTDIR"
echo "  session : $([ -f "$STATE" ] && cat "$STATE" || echo '<new>')"
echo "  quota   : $(python3 /root/.hermes/scripts/laya-quota.py 2>/dev/null | tr '\n' ' ' | cut -c1-90)"
echo "=============================================================="

RESUME_ARGS=()
if [ "${1:-}" = "--resume" ]; then
  [ -f "$STATE" ] || { echo "!! no recorded session to resume" >&2; exit 2; }
  SID=$(cat "$STATE")
  RESUME_ARGS=(--resume "$SID")
  echo "==> RESUMING session $SID"
fi

LOG=$OUTDIR/run-$(date -u +%Y%m%d-%H%M%S).log

# The prompt is a POINTER: claude reads the brief itself. Measured failure mode -- a brief passed
# inline arrived truncated (3,577 bytes -> 248). Never inline it.
PROMPT="Read /projects/_tmp/brief-opus-graph-analysis.md and follow it exactly. It is a READ-ONLY
analysis of the Rust engine at $REPO. Write your report to exactly
/projects/_tmp/opus-graph-analysis/REPORT.md as the brief specifies. Do NOT modify the repository,
do NOT commit, do NOT run any git command that writes.

Work entirely in foreground Read/Grep/Bash tool calls. Do NOT start background processes, do NOT
use any scheduling feature. The head and sidecar are live, so \`laya graph run\` works."

echo "==> dispatching (log: $LOG)"
set -o pipefail
claude -p "$PROMPT" \
  --model opus \
  "${RESUME_ARGS[@]}" \
  --allowedTools "Read,Grep,Glob,Bash,Write" \
  --max-turns "${MAX_TURNS:-120}" \
  --output-format stream-json --verbose --include-partial-messages \
  2>&1 | tee "$LOG" | \
  python3 -c '
import json, sys
for line in sys.stdin:
    try: d = json.loads(line)
    except Exception: continue
    t = d.get("type")
    if t == "system" and d.get("subtype") == "hook_started":
        sid = d.get("session_id")
        if sid:
            open("/projects/_tmp/opus-graph-analysis/session.id", "w").write(sid)
            print(f"  SESSION: {sid}", flush=True)
    elif t == "assistant_delta":
        txt = (d.get("event") or {}).get("delta", {}).get("text")
        if txt: sys.stdout.write(txt); sys.stdout.flush()
    elif t == "result":
        print("\n=== final result object ===", flush=True)
        for k in ("subtype","stop_reason","terminal_reason","is_error","num_turns",
                  "total_cost_usd","duration_ms","session_id"):
            if k in d: print(f"  {k}: {d[k]}", flush=True)
'

echo
echo "PI_EXIT=${PIPESTATUS[0]}"
echo "==> REPORT: $OUTDIR/REPORT.md  ($( [ -f "$OUTDIR/REPORT.md" ] && wc -c < "$OUTDIR/REPORT.md" || echo 'NOT WRITTEN') )"
echo "==> resume with: $0 --resume"
