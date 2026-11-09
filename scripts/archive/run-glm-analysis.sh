#!/usr/bin/env bash
# run-glm-analysis.sh -- dispatch a READ-ONLY analysis job to a pi worker.
#
# WHY THIS FILE IS SO SHORT
# -------------------------
# It was originally a 3.7 KB hand-written runner. That was a mistake: a runner for
# pi already exists and is maintained, and hand-writing a second one re-derived
# four things it already documents -- flags before `-p`, `unset PI_PROVIDER`
# first, the bare model id, and `PI_THINKING=off` -- at the cost of two wasted
# dispatches. The measured lesson: use the maintained artifact instead of a
# parallel one.
#
# THE AUTHORITATIVE RUNNER (copy it, edit its three marked spots):
#   /root/.hermes/skills/autonomous-ai-agents/pi-agent-driver/templates/run-pi-worker.sh
#
# This script exists only to add the ONE thing that template does not do: it takes
# the output directory as an argument and tells the worker the exact absolute path
# to write its report to. That matters because a brief which says "write to the
# output directory" without naming one costs a cheap model a large slice of its
# budget in search -- measured 2026-10-02, roughly a third of the visible reasoning
# in one run. So the path is injected into the prompt, not left to the brief.
#
# USAGE
#   scripts/run-glm-analysis.sh <brief> <outdir> [model] [provider]
#
#   scripts/run-glm-analysis.sh /projects/_tmp/brief-observability-glm.md \
#       /projects/_tmp/glm-observability
#
# Defaults: glm-5.3:cloud on ollama-cloud. NOTE: ollama-cloud is an ACCOUNT,
# metered against the usage bar -- it is NOT "free tier". Only specific models
# (nemotron, gpt-oss) fail to increment the bar; glm-5.3 does increment it.

set -uo pipefail

TEMPLATE=/root/.hermes/skills/autonomous-ai-agents/pi-agent-driver/templates/run-pi-worker.sh
REPO=/projects/laya-rs

usage() { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

[ $# -ge 2 ] || usage 1
case "${1:-}" in --help|-h) usage 0 ;; esac

BRIEF="$1"; OUTDIR="$2"
MODEL="${3:-glm-5.3:cloud}"; PROVIDER="${4:-ollama-cloud}"

[ -f "$BRIEF" ] || { echo "!! missing brief: $BRIEF" >&2; exit 1; }
[ -f "$TEMPLATE" ] || { echo "!! the authoritative runner is missing: $TEMPLATE" >&2; exit 1; }
mkdir -p "$OUTDIR" || exit 1
REPORT="$OUTDIR/REPORT.md"

# Hand the worker a prompt file that names the output path absolutely, then run it
# through the maintained template. One runner, one place the env is set.
PROMPT="$OUTDIR/prompt.txt"
{
  echo "Read ${BRIEF} and execute it exactly as specified. That file is your complete brief."
  echo
  echo "Write your report to exactly this path, which already exists:"
  echo "    ${REPORT}"
  echo "There is no need to search for it, and no other path is correct."
  echo
  echo "Work only under ${OUTDIR} and read-only inside ${REPO}."
  echo "Modify NOTHING under ${REPO} -- not a source file, not a doc, not a script."
} > "$PROMPT"

echo "=== read-only analysis dispatch ==="
echo "  runner  : $TEMPLATE  (the maintained template -- not a second implementation)"
echo "  model   : $MODEL @ $PROVIDER"
echo "  brief   : $BRIEF ($(wc -c < "$BRIEF") bytes)"
echo "  report  : $REPORT  (named in the prompt, so the worker does not search)"
echo
echo "  NOTE: the template pins provider/model itself (unset, then export), so the"
echo "        model argument here is documentation of what the template will use."
echo "        To change it, edit the template's marked line -- do not pass a flag."
echo

PRE_HEAD=$(git -C "$REPO" rev-parse HEAD)
PRE_DIRTY=$(git -C "$REPO" status --porcelain | wc -l)
echo "  pre : HEAD ${PRE_HEAD:0:9}, $PRE_DIRTY uncommitted path(s)"
echo

# No model/provider override is passed: the template owns those two lines, and
# pretending otherwise would be a silent no-op. If the model must differ, edit the
# template -- one place, visible, reviewable.
bash "$TEMPLATE" "$PROMPT"
RC=$?

echo
echo "=== the tree must be unchanged (read-only job) ==="
POST_HEAD=$(git -C "$REPO" rev-parse HEAD)
POST_DIRTY=$(git -C "$REPO" status --porcelain | wc -l)
if [ "$PRE_HEAD" = "$POST_HEAD" ] && [ "$PRE_DIRTY" = "$POST_DIRTY" ]; then
  echo "  unchanged"
else
  echo "  !! THE TREE CHANGED -- a read-only job modified the repo:"
  git -C "$REPO" status --porcelain | sed 's/^/    /'
fi
echo
[ -s "$REPORT" ] && echo "report: $REPORT ($(wc -c < "$REPORT") bytes)" \
                 || echo "NO report at $REPORT"
exit $RC
