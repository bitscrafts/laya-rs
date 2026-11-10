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
#   scripts/run-spec-part.sh 08           # dispatch spec 08 (fresh)
#   scripts/run-spec-part.sh 08 --resume  # resume spec 08's LAST session
#   scripts/run-spec-part.sh 08 --resume 9f2c1a4e-...   # resume a specific one
#   scripts/run-spec-part.sh part3        # legacy: spec 07, part 3
#   scripts/run-spec-part.sh --list       # what has been dispatched
#
# The spec is found by GLOB: `N` selects the first `specs/<N>-*.md`, so a new
# numbered spec needs no edit here. `part<N>` still selects spec 07's part, which
# is how that spec was split.
#
# A spec's brief lives at $LAYA_TMP/brief-spec<N>.md (default _tmp/ in the repo). Both spec and brief are
# checked before dispatch: a dispatch with a missing brief fails here, in one
# second, not after a full turn budget is spent.
#
# GENERALISED 2026-10-02 -- it was hardcoded to spec 07, so the six specs the
# architectural review produced could not be dispatched from it at all, which
# would have pushed the dispatch back into an inline command (HARD RULE 0d).

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRIEF_DIR="${LAYA_TMP:-$REPO/_tmp}"
STATE_DIR="${SPEC_STATE_DIR:-$BRIEF_DIR/spec-dispatch}"
MODEL=sonnet
# The turn cap is a COST bound, not a difficulty setting — but 120 turned out to sit
# inside the range that legitimate specs need, so it was failing real work rather
# than bounding runaways. Measured 2026-10-01/02 across five dispatches:
#
#   part1   53 turns  success                     part4  121 turns  error_max_turns  (FAILED)
#   part2   47 turns  success                     spec08 120 turns  success (AT the cap)
#   part3   84 turns  success
#
# Spec 08 finished on its LAST allowed turn; one more and it would have died with
# its verification unfinished, which is the expensive shape of failure. 160 gives
# roughly a third more headroom over the largest observed need. At the measured
# ~$0.06/turn that bounds a run near $9.50 — close to what part 4 already cost.
# Override per-run with MAX_TURNS=200 when a spec is known to be larger.
MAX_TURNS="${MAX_TURNS:-160}"

# The worker's environment. Set here, never assumed: a background shell starts
# with a minimal env and `cargo` would not be found.
export PATH="$HOME/.local/bin:/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
# Required for `claude --permission-mode bypassPermissions` as root.
export IS_SANDBOX=1
# The claude CLI's own credentials live here.
if [ -n "${AGENT_ENV_FILE:-}" ] && [ -f "$AGENT_ENV_FILE" ]; then set -a; . "$AGENT_ENV_FILE"; set +a; fi

mkdir -p "$STATE_DIR"

usage() {
  sed -n '2,41p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

list_dispatched() {
  echo "=== dispatched specs (state in $STATE_DIR) ==="
  local found=0
  for f in "$STATE_DIR"/*.session; do
    [ -f "$f" ] || continue
    found=1
    local key sid started
    key=$(basename "$f" .session)
    sid=$(cat "$f")
    started=$(stat -c %y "$f" 2>/dev/null | cut -d. -f1)
    printf '  %-8s session %s\n           started %s\n' "$key" "$sid" "$started"
    printf '           resume:  %s %s --resume %s\n' "$0" "$key" "$sid"
  done
  [ "$found" = 1 ] || echo "  (none)"
}

[ $# -ge 1 ] || usage 1

case "${1:-}" in
  --list|-l) list_dispatched; exit 0 ;;
  --help|-h) usage 0 ;;
esac

ARG="${1#part}"
# `part<N>` keeps its meaning: spec 07's part N, which is how that spec was split.
case "${1:-}" in
  part*) KEY="07-part${ARG}"; SPEC="$REPO/specs/07-graph-executor-part${ARG}.md" ;;
  *)     KEY="$ARG";         SPEC=$(ls "$REPO"/specs/"$ARG"-*.md 2>/dev/null | head -1) ;;
esac
PART="$KEY"
RESUME_SID=""
if [ "${2:-}" = "--resume" ]; then
  RESUME_SID="${3:-}"
  if [ -z "$RESUME_SID" ]; then
    # No id given: resume this spec's own last session.
    if [ -f "$STATE_DIR/${KEY}.session" ]; then
      RESUME_SID=$(cat "$STATE_DIR/${KEY}.session")
    else
      echo "!! no session recorded for ${KEY}; pass an explicit id" >&2
      exit 1
    fi
  fi
fi

BRIEF="$BRIEF_DIR/brief-spec${KEY}.md"
LOG="$BRIEF_DIR/spec${KEY}.log"
ERR="$BRIEF_DIR/spec${KEY}.err"

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
# `--permission-mode bypassPermissions` IS LOAD-BEARING -- do not remove it, and do
# not weaken it to the default. Measured 2026-09-09: `claude -p`
# in print mode WITHOUT the skip-permissions flag gets STUCK requesting an
# interactive file-write approval that print mode cannot surface. The symptom is a
# silent hang, not an error, so it costs a full timeout before anyone notices. This
# is why the wrapper exports IS_SANDBOX=1 as well (required for bypassPermissions as
# root).
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
  echo "$SID" > "$STATE_DIR/${KEY}.session"
  echo "  session_id: $SID  (recorded; resume with: $0 $KEY --resume)"
fi

# Record the PRE-DISPATCH HEAD so `verify-spec-part.sh` can prove the worker did
# not commit. The old check compared `git status -- specs/` and called it "worker
# did not commit" -- but it actually tested "specs/ is clean", so it FAILED on the
# ORCHESTRATOR's own uncommitted spec corrections. A check whose name and assertion
# disagree certifies nothing (the class this project keeps re-finding).
PRE_HEAD=$(git -C "$REPO" rev-parse HEAD)
echo "$PRE_HEAD" > "$STATE_DIR/${KEY}.head"
echo "  pre-dispatch HEAD: ${PRE_HEAD:0:9}  (recorded for the commit check)"

echo "=== final result object ==="
# THE RESULT EVENT IS NOT THE LAST LINE. Measured 2026-10-01: `type: result` sat at
# line 12146 and a `system/task_notification` followed at 12149, so `tail -1` parsed
# the notification and printed only `subtype: task_notification` -- losing
# `stop_reason`, `num_turns` and the cost. Exactly the class this project keeps
# finding: a report that does not correspond to the property it names. Scan for the
# result event instead of trusting its position.
python3 - "$LOG" <<'PY' 2>/dev/null
import json, sys
res = None
with open(sys.argv[1], errors="ignore") as fh:
    for line in fh:
        try:
            d = json.loads(line.strip())
        except Exception:
            continue
        if d.get("type") == "result":
            res = d            # keep the LAST one, if several
if res is None:
    print("  (no `type: result` event in the log -- the run may have died before finishing)")
else:
    for k in ("subtype", "stop_reason", "terminal_reason", "is_error",
              "num_turns", "total_cost_usd", "duration_ms", "session_id"):
        if k in res:
            print(f"  {k}: {res[k]}")
    r = res.get("result")
    if isinstance(r, str):
        print("  result:", r[:400])
    # A turn cap is the documented wall, and it is NOT the same as success.
    if res.get("subtype") == "error_max_turns":
        print("  >>> TURN CAP HIT: the tree may hold partial work. Verify it, then"
              " resume this session with only what remains (see MAX_TURNS).")
PY

echo "=== tree state (exit code is NOT evidence; the tree is) ==="
git -C "$REPO" status --porcelain
echo
echo "  NOTE: the worker must NOT have committed. HEAD is $(git -C "$REPO" rev-parse --short HEAD)."
echo "  Verify INDEPENDENTLY before believing the report: scripts/verify-spec-part.sh $PART"
exit $RC
