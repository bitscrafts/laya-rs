#!/usr/bin/env bash
# run-glm-analysis.sh -- dispatch a read-only review to GLM via pi.
#
# WHY A SCRIPT (HARD RULE 0d): a dispatch is the most expensive action a project
# takes. It must be reviewable before it runs, re-runnable identically after it
# fails, and it must set its own environment -- a background shell does not
# inherit an interactive one, so PATH and the API keys have to be set here or the
# run fails for a reason that has nothing to do with the work.
#
# USAGE
#   scripts/run-glm-analysis.sh <brief> <outdir> [model] [provider]
#
#   scripts/run-glm-analysis.sh /projects/_tmp/brief-observability-glm.md \
#       /projects/_tmp/glm-observability
#
# Defaults: model glm-5.3:cloud, provider ollama-cloud -- confirmed working
# through pi. BOTH are passed, because --model alone keeps the session's default
# provider (anthropic here) and would spend the paid quota instead.
#
# A brief's REPORT is written into <outdir>; the run's stream log sits beside it
# so a failed run can be diagnosed without re-spending.

set -uo pipefail

REPO=/projects/laya-rs
# The model and its provider. BOTH must be set, and on this box they must be set as
# ENVIRONMENT VARIABLES, not flags:
#
#   `pi --model glm-5.3:cloud --provider ollama-cloud`  -> still routed to anthropic
#                                                          (HTTP 429 on the paid quota)
#   `PI_PROVIDER=ollama-cloud PI_MODEL=glm-5.3:cloud pi -p ...` -> PONG from GLM
#
# Measured. The flag form does not override the session's default provider here, so a
# dispatch that looked correct spent the paid Anthropic quota while the free GLM path
# sat idle. The env form is what this script uses, and the first thing it prints is
# which provider it believes it is using.
PROVIDER_DEFAULT="ollama-cloud"
MODEL_DEFAULT="glm-5.3:cloud"

# The worker's environment, set here and never assumed.
export PATH="/root/.local/bin:/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
if [ -f /root/.hermes/.env ]; then set -a; . /root/.hermes/.env; set +a; fi

usage() {
  sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

[ $# -ge 2 ] || usage 1
case "${1:-}" in --help|-h) usage 0 ;; esac

BRIEF="$1"
OUTDIR="$2"
MODEL="${3:-$MODEL_DEFAULT}"
PROVIDER="${4:-$PROVIDER_DEFAULT}"
# Env, not flags -- see the note above: the flags do not override here.
export PI_MODEL="$MODEL"
export PI_PROVIDER="$PROVIDER"

# Preconditions: fail HERE, in one second, not after a long run.
[ -f "$BRIEF" ] || { echo "!! missing brief: $BRIEF" >&2; exit 1; }
command -v pi >/dev/null || { echo "!! pi not on PATH" >&2; exit 1; }
mkdir -p "$OUTDIR" || exit 1

REPORT="$OUTDIR/REPORT.md"
LOG="$OUTDIR/run.log"
ERR="$OUTDIR/run.err"

echo "=== GLM observability review (read-only) ==="
echo "  model   : $MODEL"
echo "  provider: $PROVIDER   (BOTH are passed -- --model alone keeps the default provider)"
echo "  brief   : $BRIEF  ($(wc -c < "$BRIEF") bytes)"
echo "  outdir  : $OUTDIR"
echo "  report  : $REPORT"
echo "  repo    : $REPO  (READ-ONLY -- the worker must not modify it)"
echo "  log     : $LOG"
echo

# Record the tree state BEFORE, so a modification can be detected afterwards.
PRE_HEAD=$(git -C "$REPO" rev-parse HEAD)
PRE_DIRTY=$(git -C "$REPO" status --porcelain | wc -l)
echo "  pre-run: HEAD ${PRE_HEAD:0:9}, $PRE_DIRTY uncommitted path(s)"
echo

cd "$REPO" || exit 1

# The brief is handed over as a POINTER, never inlined: a brief inlined on the
# command line can be silently truncated, and a truncated contract produces a
# confident worker doing half the job.
rm -f "$LOG" "$ERR"
stdbuf -oL -eL pi -p "$(printf '%s' \
  "Read ${BRIEF} and execute it exactly as specified. That file is your complete brief. \
Write your report to ${REPORT}. Do not modify anything under ${REPO}. \
Do not look for instructions anywhere else.")" \
  --model "$MODEL" \
  --provider "$PROVIDER" \
  > "$LOG" 2> "$ERR"
RC=$?
echo "EXIT=$RC  (brief delivered as a pointer to $BRIEF, not inlined)"

# The provider is a claim until it is read back. A dispatch that silently used the
# wrong provider is the failure this script was written to prevent, so it reports
# what actually answered rather than what it asked for.
echo
echo "=== which provider actually answered? ==="
if grep -qi 'anthropic' "$ERR" 2>/dev/null; then
  echo "  !! ANTHROPIC appeared in the error stream -- the routing leaked to the paid"
  echo "     provider. Investigate before spending again."
  grep -i 'anthropic' "$ERR" | head -2 | sed 's/^/    /'
else
  echo "  no anthropic error in the stream (expected for an ollama-cloud run)"
fi
if grep -qiE 'rate limit|429' "$ERR" 2>/dev/null; then
  echo "  !! RATE LIMITED:"; head -3 "$ERR" | sed 's/^/    /'
fi

echo
echo "=== did the report land? ==="
if [ -s "$REPORT" ]; then
  echo "  YES: $REPORT  ($(wc -c < "$REPORT") bytes, $(wc -l < "$REPORT") lines)"
else
  echo "  NO report at $REPORT"
  echo "  last lines of stderr:"
  tail -12 "$ERR" | sed 's/^/    /'
fi

echo
echo "=== was the repo left alone? (it must be) ==="
POST_HEAD=$(git -C "$REPO" rev-parse HEAD)
POST_DIRTY=$(git -C "$REPO" status --porcelain | wc -l)
echo "  post-run: HEAD ${POST_HEAD:0:9}, $POST_DIRTY uncommitted path(s)"
if [ "$PRE_HEAD" != "$POST_HEAD" ] || [ "$POST_DIRTY" != "$PRE_DIRTY" ]; then
  echo "  !! THE TREE CHANGED -- the brief said read-only. Diff:"
  git -C "$REPO" status --porcelain | sed 's/^/    /'
else
  echo "  unchanged -- read-only was respected"
fi

exit $RC
