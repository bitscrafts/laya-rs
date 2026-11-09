#!/usr/bin/env bash
# run-spec-pi.sh -- dispatch ONE spec-implementation job to a pi worker (pi + glm-5.3-flash:cloud).
#
# WHY THIS IS A SCRIPT
# --------------------
# (HARD RULE 0d.) A dispatch is the most expensive action a project takes: it must be reviewable before
# it runs and re-runnable identically after it fails. An inline command exists only in a terminal
# history. This file is the pi counterpart of `run-spec-part.sh` (which drives claude), and it is the
# only place a pi worker is launched from.
#
# It does NOT hand-write the pi invocation. It delegates to the maintained runner, so the four things
# that cost two wasted dispatches are set in exactly one place:
#   (1) `unset PI_PROVIDER PI_MODEL` FIRST, else a stale value in the environment wins
#   (2) provider/model/thinking flags BEFORE `-p`
#   (3) the model id BARE when `--provider` is passed
#   (4) always `</dev/null`, because `pi -p` BLOCKS on an inherited stdin
# An earlier hand-rolled pi runner re-derived all four badly; use the artifact, not a parallel version.
#
# WHAT THIS ADDS over the template (and nothing else):
#   * a project dir and a run name, so the session is recoverable
#   * it requires the spec AND the brief to exist, so a dispatch fails in one second, not after
#     200 iterations
#   * it prints model, provider, spec, brief, log and payload size BEFORE spending anything
#   * it records the session id to a file at dispatch time -- the only handle for a resume
#   * it prints the tree state and HEAD on exit, because the exit code is NOT evidence of work
#
# USAGE
#   scripts/run-spec-pi.sh <spec-number> [--resume [session-file]]   (runs scripts/run-pi-worker.sh)
#
#   scripts/run-spec-pi.sh 24                    # fresh
#   scripts/run-spec-pi.sh 24 --resume           # continue the recorded session
#   scripts/run-spec-pi.sh 24 --resume /path/to/session.jsonl
#
# ENV OVERRIDES
#   MODEL=glm-5.3-flash:cloud   PROVIDER=ollama-cloud   MAX_ITERS=200   WALL_SECS=5400
#   BRIEF_DIR=/projects/_tmp
#
# NOTE ON THE MODEL: glm-5.3-flash on ollama-cloud is an ACCOUNT, metered against the usage bar -- it is
# NOT a free tier. Check the bar before dispatching; it increments on glm-5.3.
# NOTE ON PEAK: glm-5.3-flash has NO peak window (unlike deepseek-v4.1-flash, 12:00-18:00 UTC).
set -uo pipefail

REPO=/projects/laya-rs
TEMPLATE="$(dirname "$0")/run-pi-worker.sh"   # the project-owned copy of the maintained runner
BRIEF_DIR="${BRIEF_DIR:-/projects/_tmp}"
MODEL="${MODEL:-glm-5.3-flash:cloud}"
PROVIDER="${PROVIDER:-ollama-cloud}"
MAX_ITERS="${MAX_ITERS:-200}"
WALL_SECS="${WALL_SECS:-5400}"
DISPATCH_DIR=/projects/_tmp/spec-dispatch

usage() { sed -n '2,44p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
case "${1:-}" in --help|-h|"") usage 0 ;; esac

N="$1"; shift
KEY="$N"
SPEC=$(ls "$REPO"/specs/"$KEY"-*.md 2>/dev/null | head -1)
BRIEF="$BRIEF_DIR/brief-spec${KEY}.md"
LOG="$BRIEF_DIR/spec${KEY}-pi.log"
SESS_DIR="$DISPATCH_DIR/pi-${KEY}"
mkdir -p "$DISPATCH_DIR" "$SESS_DIR" || exit 1

[ -f "$TEMPLATE" ] || { echo "!! the maintained pi runner is missing: $TEMPLATE" >&2; exit 1; }
[ -f "$SPEC" ]     || { echo "!! missing spec: $SPEC" >&2; exit 1; }
[ -f "$BRIEF" ]    || { echo "!! missing brief: $BRIEF" >&2; exit 1; }

RESUME=0
PI_SESSION=""
if [ "${1:-}" = "--resume" ]; then
  RESUME=1; shift || true
  PI_SESSION="${1:-}"
  if [ -z "$PI_SESSION" ] && [ -f "$DISPATCH_DIR/${KEY}.session" ]; then
    PI_SESSION="$(cat "$DISPATCH_DIR/${KEY}.session")"
  fi
  if [ -n "$PI_SESSION" ] && [ ! -f "$PI_SESSION" ]; then
    echo "!! the recorded session file does not exist: $PI_SESSION" >&2; exit 1
  fi
fi

echo "=== DISPATCHING spec $KEY to pi ($([ $RESUME = 1 ] && echo RESUME || echo fresh)) ==="
echo "  spec    : $SPEC"
echo "  brief   : $BRIEF"
echo "  model   : $MODEL @ $PROVIDER   (set inside the runner; this line documents it)"
echo "  log     : $LOG"
echo "  sessions: $SESS_DIR"
echo "  repo    : $REPO  (worker does NOT commit)"
echo "  iters   : $MAX_ITERS   wall: ${WALL_SECS}s"
echo "  payload size: $(wc -c < "$BRIEF") bytes"
if [ $RESUME = 1 ]; then echo "  resuming: ${PI_SESSION:-<the recorded session>}"; fi
echo

PRE_HEAD=$(git -C "$REPO" rev-parse HEAD)
echo "  pre-dispatch HEAD: ${PRE_HEAD:0:9}  (recorded for the commit check)"
echo

# The prompt names the brief and the report path absolutely, so a cheap model does not spend budget
# searching for either. Measured 2026-10-02: leaving the output path implicit cost one run roughly a
# third of its visible reasoning.
# NOT inside SESS_DIR: the runner ARCHIVES that directory when FRESH=1 (mv aside + mkdir), which
# would take the prompt with it and hand the worker no input at all -- measured 2026-10-02, the
# first dispatch did exactly that.
PROMPT="$DISPATCH_DIR/prompt-spec${KEY}.txt"
{
  echo "Read ${BRIEF} and implement it exactly as specified. That file is your complete brief."
  echo "Its spec is ${SPEC}. Follow every guardrail in the brief."
  echo
  echo "Write your report to exactly this path:"
  echo "    ${BRIEF_DIR}/spec${KEY}-REPORT.md"
  echo
  echo "Work in ${REPO}. Do NOT commit. Do NOT modify any other spec in specs/."
} > "$PROMPT"

# The template owns provider/model/thinking/session and the env. Everything it needs is passed as env.
export PI_PROVIDER="$PROVIDER" PI_MODEL="$MODEL" PI_THINKING=off
export PI_HTTP_REQUEST_TIMEOUT_SECS="${PI_HTTP_REQUEST_TIMEOUT_SECS:-600}"  # glm-5.3-flash:cloud timed out at the 120s default (measured 2026-10-02); 600 to match settings.json
export MAX_ITERS WALL_SECS LOG SESS_DIR
export FRESH=$(( 1 - RESUME ))
[ -n "$PI_SESSION" ] && export PI_SESSION

bash "$TEMPLATE" "$PROMPT"
WRAPPER_RC=$?

# The wrapper's exit is NOT the worker's: the runner's last command is an echo, so it returns 0 even
# when the model call failed. Read PI_EXIT= from the log (the runner prints it) -- otherwise a dead
# dispatch reads as success and is only noticed when the report turns out to be missing.
PI_RC=$(grep -o 'PI_EXIT=[0-9]*' "$LOG" 2>/dev/null | tail -1 | cut -d= -f2)
RC=${PI_RC:-$WRAPPER_RC}
[ -n "$PI_RC" ] && [ "$PI_RC" != "$WRAPPER_RC" ] && \
  echo "  NOTE: wrapper exit $WRAPPER_RC but PI_EXIT=$PI_RC -- the worker's exit is the one that counts"

# Record the session id at dispatch time: it is the only handle for a resume after a turn cap.
NEW_SESSION=$(find "$SESS_DIR" -name '*.jsonl' -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)
if [ -n "$NEW_SESSION" ]; then
  echo "$NEW_SESSION" > "$DISPATCH_DIR/${KEY}.session"
  echo
  echo "  session_id: $NEW_SESSION  (recorded; resume with: scripts/run-spec-pi.sh $KEY --resume)"
fi

echo
echo "=== tree state (exit code is NOT evidence; the tree is) ==="
git -C "$REPO" status --porcelain | sed 's/^/  /'
echo
echo "  NOTE: the worker must NOT have committed. HEAD is ${PRE_HEAD:0:9}."
echo "  Verify INDEPENDENTLY before believing the report: scripts/verify-spec-part.sh $KEY"
echo "  worker exit (PI_EXIT): $RC"
exit $RC
