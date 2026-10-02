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
# Overridable so a follow-up round is a first-class reviewable action, not an inline command.
#   ROUND=2 scripts/run-opus-graph-analysis.sh --resume
BRIEF=${BRIEF:-/projects/_tmp/brief-opus-graph-analysis.md}
OUTDIR=${OUTDIR:-/projects/_tmp/opus-graph-analysis}
# The report this round must produce. Recorded and then CHECKED after the run: a round that exits 0
# having written nothing reads exactly like a round that succeeded (measured 2026-10-02).
REPORT=${REPORT:-$OUTDIR/REPORT.md}
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
echo "  report  : $REPORT  (checked after the run; a round that writes nothing FAILS)"
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

# Required for `claude --permission-mode bypassPermissions` as root (measured 2026-09-09).
export IS_SANDBOX=1
export OUTDIR   # the embedded status printer records the session id under it

LOG=$OUTDIR/run-$(date -u +%Y%m%d-%H%M%S).log

# The prompt is a POINTER: claude reads the brief itself. Measured failure mode -- a brief passed
# inline arrived truncated (3,577 bytes -> 248). Never inline it.
#
# The brief path and the report path are INTERPOLATED FROM $BRIEF/$REPORT, never hardcoded: an
# earlier version of this script named the round-1 brief here while the preflight honoured the
# override, so a follow-up round silently re-read the round-1 brief and did the round-1 job
# (measured 2026-10-02: 5 turns, 24 s, no ROUND2 output).
PROMPT="Read ${BRIEF} and follow it exactly. It is a READ-ONLY analysis of the Rust engine at $REPO.
Write your report to exactly ${REPORT} as the brief specifies. Do NOT modify the repository, do NOT
commit, do NOT run any git command that writes.

Work entirely in foreground Read/Grep/Bash tool calls. Do NOT start background processes, do NOT use
any scheduling feature. The head and sidecar are live, so \`laya graph run\` works."

echo "==> dispatching (log: $LOG)"
set -o pipefail
# `--permission-mode bypassPermissions` IS LOAD-BEARING -- do not remove it and do not weaken it.
# Measured 2026-09-09 (recorded in amem) and re-measured here on 2026-10-02: `claude -p` in print mode
# WITHOUT the skip-permissions flag gets STUCK requesting approval for its FIRST write, and answers
# from prior context instead of doing the work -- which reads as a completed run. `--allowedTools`
# alone is NOT enough. IS_SANDBOX=1 is required for bypassPermissions as root.
claude -p "$PROMPT" \
  --model opus \
  "${RESUME_ARGS[@]}" \
  --permission-mode bypassPermissions \
  --max-turns "${MAX_TURNS:-120}" \
  --output-format stream-json --verbose --include-partial-messages \
  2>&1 | tee "$LOG" | \
  python3 -c '
import json, os, sys
for line in sys.stdin:
    try: d = json.loads(line)
    except Exception: continue
    t = d.get("type")
    if t == "system" and d.get("subtype") == "hook_started":
        sid = d.get("session_id")
        if sid:
            open(os.environ["OUTDIR"] + "/session.id", "w").write(sid)
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
# A round that exits 0 having written nothing reads exactly like a round that succeeded.
# MEASURED 2026-10-02: a resumed round ran 5 turns / 24 s / exit 0 and produced NO output, because
# the tools had been blocked -- it answered from prior context instead. So the report is CHECKED,
# and a missing or stale report FAILS the dispatch.
PI_EXIT=${PIPESTATUS[0]}
echo "CLAUDE_EXIT=$PI_EXIT"
if [ ! -f "$REPORT" ]; then
  echo "!! DISPATCH FAILED: $REPORT was not written at all. Read the log: $LOG"
  exit 3
fi
REPORT_AGE=$(( $(date +%s) - $(stat -c %Y "$REPORT") ))
echo "==> REPORT: $REPORT  ($(wc -c < "$REPORT") bytes, written ${REPORT_AGE}s ago)"
if [ "$REPORT_AGE" -gt 900 ]; then
  echo "!! DISPATCH FAILED: $REPORT is ${REPORT_AGE}s old -- this run did NOT write it."
  echo "   The run exited 0 and produced nothing. Usual cause: the tools were blocked"
  echo "   (needs --permission-mode bypassPermissions AND IS_SANDBOX=1 -- --allowedTools alone is"
  echo "   not enough), or the brief was not honoured. Read the log: $LOG"
  exit 3
fi
echo "==> resume with: scripts/run-opus-graph-analysis.sh --resume"
