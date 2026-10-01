#!/usr/bin/env bash
# run-spec-part.sh -- dispatch ONE spec part to the workhorse (claude + sonnet).
#
# WHY THIS IS A SCRIPT AND NOT AN INLINE COMMAND
# ----------------------------------------------
# The user's rule, verbatim: "you must create a script!!! thats the way we were
# working so far!" and, on 2026-10-01, "should be written that all dispatch must
# be done from a script?!" -- after three parts had been dispatched from long
# inline shell commands that existed only in a terminal history.
#
# The reasons are not stylistic:
#   * A dispatch is the most expensive action this project takes ($2.87 a run
#     measured). It must be REVIEWABLE before it is run, and re-runnable
#     identically after it fails.
#   * Background shells do NOT inherit the interactive environment, so PATH and
#     the cargo env must be set INSIDE the script. Dispatching from an inline
#     command that happened to have the right PATH is not reproducible.
#   * The session id must be captured to a file at dispatch time, because resume
#     needs it and it is the only handle on a run that stopped early.
#
# USAGE
#   scripts/run-spec-part.sh 3            # dispatch part 3 (fresh)
#   scripts/run-spec-part.sh 3 --resume   # resume part 3's LAST session
#   scripts/run-spec-part.sh 3 --resume ac864d00-...   # resume a specific one
#   scripts/run-spec-part.sh --list       # what has been dispatched
#
# A part's brief lives at /projects/_tmp/brief-spec07-part<N>.md and its spec at
# specs/07-graph-executor-part<N>.md. Both are checked before dispatch: a dispatch
# with a missing brief fails here, not 120 turns later.

set -uo pipefail

REPO=/projects/laya-rs
BRIEF_DIR=/projects/_tmp
STATE_DIR=/projects/_tmp/spec07-dispatch
MODEL=sonnet
MAX_TURNS=120

# The worker's environment. Set here, never assumed: a background shell starts
# with a minimal env and `cargo` would not be found.
export PATH="/root/.local/bin:/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
# Required for `claude --permission-mode bypassPermissions` as root.
export IS_SANDBOX=1
# The claude CLI's own credentials live here.
if [ -f /root/.hermes/.env ]; then set -a; . /root/.hermes/.env; set +a; fi

mkdir -p "$STATE_DIR"

usage() {
  sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

list_dispatched() {
  echo "=== dispatched parts (state in $STATE_DIR) ==="
  local found=0
  for f in "$STATE_DIR"/part*.session; do
    [ -f "$f" ] || continue
    found=1
    local part sid started
    part=$(basename "$f" .session)
    sid=$(cat "$f")
    started=$(stat -c %y "$f" 2>/dev/null | cut -d. -f1)
    printf '  %-6s session %s\n         started %s\n' "$part" "$sid" "$started"
    printf '         resume:  %s %s --resume %s\n' "$0" "${part#part}" "$sid"
  done
  [ "$found" = 1 ] || echo "  (none)"
}

[ $# -ge 1 ] || usage 1

case "${1:-}" in
  --list|-l) list_dispatched; exit 0 ;;
  --help|-h) usage 0 ;;
esac

PART="${1#part}"
RESUME_SID=""
if [ "${2:-}" = "--resume" ]; then
  RESUME_SID="${3:-}"
  if [ -z "$RESUME_SID" ]; then
    # No id given: resume this part's own last session.
    if [ -f "$STATE_DIR/part${PART}.session" ]; then
      RESUME_SID=$(cat "$STATE_DIR/part${PART}.session")
    else
      echo "!! no session recorded for part ${PART}; pass an explicit id" >&2
      exit 1
    fi
  fi
fi

SPEC="$REPO/specs/07-graph-executor-part${PART}.md"
BRIEF="$BRIEF_DIR/brief-spec07-part${PART}.md"
LOG="$BRIEF_DIR/spec07-part${PART}.log"
ERR="$BRIEF_DIR/spec07-part${PART}.err"

# Preconditions -- fail HERE, cheaply, rather than after a long run.
[ -f "$SPEC" ]  || { echo "!! missing spec:  $SPEC"  >&2; exit 1; }
[ -f "$BRIEF" ] || { echo "!! missing brief: $BRIEF" >&2; exit 1; }

# A resume must not silently become a fresh run if the session has aged out --
# `claude --resume` on an unknown id errors, which is the honest outcome.
if [ -n "$RESUME_SID" ]; then
  echo "=== RESUMING part ${PART} session ${RESUME_SID}"
  echo "    (the brief below is followed as ADDITIONAL instruction to that session)"
else
  echo "=== DISPATCHING part ${PART} fresh"
fi
echo "    spec   : $SPEC"
echo "    brief  : $BRIEF"
echo "    model  : $MODEL (claude)"
echo "    log    : $LOG"
echo "    repo   : $REPO  (worker does NOT commit)"
echo "    turns  : $MAX_TURNS"
echo "    payload size: $(wc -c < "$BRIEF") bytes"
echo

cd "$REPO" || exit 1

# The dispatch. stream-json + partial messages so progress is observable -- the
# tool-use count alone is NOT a stall signal (a long thinking block freezes it
# while the log keeps growing; measured 2026-10-01).
#
# The brief is handed over as a POINTER, never inlined. `$(cat brief)` puts the
# whole brief on the command line, and worker-output-acceptance records the
# measured failure: a 3,577-byte brief arriving as 248 bytes, silently, producing
# a confident worker executing half a contract. A truncation like that does not
# fail loudly. Pointing at the file cannot truncate.
rm -f "$LOG" "$ERR"
stdbuf -oL -eL claude -p "Read ${BRIEF} and implement it exactly as specified. That file is your complete brief: follow every guardrail in it, and do not look for instructions anywhere else." \
  --model "$MODEL" \
  --permission-mode bypassPermissions \
  --output-format stream-json --verbose --include-partial-messages \
  ${RESUME_SID:+--resume "$RESUME_SID"} \
  --max-turns "$MAX_TURNS" \
  > "$LOG" 2> "$ERR"
RC=$?
echo "EXIT=$RC"
echo "  (brief delivered as a pointer to $BRIEF, not inlined -- see the note above)"

# Capture the session id -- the only handle for a resume.
SID=$(grep -o '"session_id":"[^"]*"' "$LOG" 2>/dev/null | head -1 | cut -d'"' -f4)
if [ -n "$SID" ]; then
  echo "$SID" > "$STATE_DIR/part${PART}.session"
  echo "  session_id: $SID  (recorded; resume with: $0 $PART --resume)"
fi

# Record the PRE-DISPATCH HEAD so `verify-spec-part.sh` can prove the worker did
# not commit. The old check compared `git status -- specs/` and called it "worker
# did not commit" -- but it actually tested "specs/ is clean", so it FAILED on the
# ORCHESTRATOR's own uncommitted spec corrections. A check whose name and assertion
# disagree certifies nothing (the class this project keeps re-finding).
PRE_HEAD=$(git -C "$REPO" rev-parse HEAD)
echo "$PRE_HEAD" > "$STATE_DIR/part${PART}.head"
echo "  pre-dispatch HEAD: ${PRE_HEAD:0:9}  (recorded for the commit check)"

echo "=== final result object ==="
tail -1 "$LOG" | python3 -c "
import json,sys
try:
    d=json.loads(sys.stdin.read())
    for k in ('subtype','stop_reason','terminal_reason','num_turns','total_cost_usd','duration_ms'):
        if k in d: print(f'  {k}: {d[k]}')
    r=d.get('result')
    if isinstance(r,str): print('  result:', r[:400])
except Exception as e:
    print('  (could not parse final line:', e, ')')
" 2>/dev/null

echo "=== tree state (exit code is NOT evidence; the tree is) ==="
git -C "$REPO" status --porcelain
echo
echo "  NOTE: the worker must NOT have committed. HEAD is $(git -C "$REPO" rev-parse --short HEAD)."
echo "  Verify INDEPENDENTLY before believing the report: scripts/verify-spec-part.sh $PART"
exit $RC
