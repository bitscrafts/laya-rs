#!/usr/bin/env bash
# run-pi-worker.sh -- COPY of the maintained pi runner, with the three marked spots filled
# for THIS project. The upstream template lives at
#   /root/.hermes/skills/autonomous-ai-agents/pi-agent-driver/templates/run-pi-worker.sh
# and its own header instructs: "copy it, edit its three marked spots". This IS that copy.
# It is committed so a dispatch is reviewable and re-runnable (HARD RULE 0d). — TEMPLATE for dispatching a pi worker job.
#
# Copy this next to the project (e.g. _tmp/run-<job>.sh), edit the three marked
# spots, and run it in the BACKGROUND. Do not inline a long `export ...; pi ...`
# string into a background terminal call — that is the failure mode this
# template exists to prevent.
#
# Why every line is here (all four are load-bearing; see the skill's
# "Always launch pidag/pi via a self-contained script" + the env-inheritance
# and flags-first pitfalls):
#   1. Background/cron/wrapped shells do NOT source ~/.bashrc and do NOT have
#      /root/.local/bin on PATH -> source the env file and set PATH explicitly.
#   2. pid 1 injects PI_PROVIDER/PI_MODEL (the PAID topup) into every process,
#      and env OUTRANKS settings.json -> unset them, then set what you want.
#   3. pi 0.2.0's trailing_var_arg swallows any flag placed AFTER the prompt
#      -> --provider/--model/--thinking MUST precede -p.
#   4. With `pi --provider X` the model id must be BARE (`glm-5.3-flash:cloud`),
#      not the slash-first `X/...` form pidag uses in .pidag/config.toml.

set -uo pipefail
export PATH="/usr/local/cargo/bin:/root/.local/bin:$PATH"

# (1) Keys: background shells do not inherit the interactive shell's env.
set -a; . /root/.hermes/.env 2>/dev/null; set +a
: "${OLLAMA_API_KEY:?OLLAMA_API_KEY missing (looked in /root/.hermes/.env)}"

# (2) provider/model: explicit, never inherited.
unset PI_PROVIDER PI_MODEL 2>/dev/null || true
export PI_PROVIDER="ollama-cloud"
export PI_MODEL="glm-5.3-flash:cloud"   # BARE id (#4)
export PI_THINKING="off"                # glm-5.3-flash/deepseek-v4.1-flash are
                                        # REASONING models: with thinking ON they
                                        # burn the whole output budget on the
                                        # trace and emit ZERO content.

# ---- EDIT: project dir ------------------------------------------------------
cd /projects/laya-rs || exit 2

SESS_DIR="${SESS_DIR:-/projects/_tmp/spec-dispatch/pi}"
# ---- EDIT: prompt file ------------------------------------------------------
PROMPT_FILE="${1:-/projects/_tmp/pi-prompt.txt}"
[ -f "$PROMPT_FILE" ] || { echo "usage: $0 [prompt-file]" >&2; exit 2; }

# SESSION STORAGE vs RESUMING — these are DIFFERENT FLAGS (measured 2026-10-02).
#   --session-dir <DIR>  = where session files are written/looked up. NOT a resume.
#   --session <FILE>     = continue a SPECIFIC session file. This is the resume.
#
# `FRESH=0` + `--session-dir` does NOT continue a prior session -- it starts a NEW
# session file in that dir. An earlier version of this comment claimed pi "resumes
# the LARGEST session in it"; that was measured FALSE: the old file stayed frozen
# at 76 lines / 646 KB while a new 4-line file appeared beside it, carrying no
# memory of what the first run had read. The resume silently became a fresh run,
# which is the exact waste the resume-first rule exists to prevent.
#
# So: FRESH=1 archives the dir and starts clean (the default, for a NEW job).
#     FRESH=0 keeps the dir -- but to genuinely CONTINUE work, do NOT rely on it;
#     pass the session file explicitly instead (see the resume block below).
if [ "${FRESH:-1}" = "1" ] && [ -d "$SESS_DIR" ]; then
    mv "$SESS_DIR" "${SESS_DIR}.old-$(date -u +%Y%m%d-%H%M%S)"
fi
mkdir -p "$SESS_DIR"

# ---- RESUMING A SPECIFIC SESSION --------------------------------------------
# To continue an interrupted run, set PI_SESSION to that session's FILE PATH and
# the runner will pass --session instead of --session-dir:
#
#   PI_SESSION="$SESS_DIR/--projects-<proj>--/<ts>_<id>.jsonl" bash run-pi-worker.sh prompt.txt
#
# then VERIFY the original file GREW (bytes AND lines). If a new file appeared
# beside a frozen one, the resume failed and you paid for a fresh run:
#   ls -la "$SESS_DIR"/--*/
SESSION_ARG=(--session-dir "$SESS_DIR")
if [ -n "${PI_SESSION:-}" ]; then
    [ -f "$PI_SESSION" ] || { echo "!! PI_SESSION not a file: $PI_SESSION" >&2; exit 2; }
    echo "==> RESUMING session file: $PI_SESSION" >&2
    SESSION_ARG=(--session "$PI_SESSION")
fi

LOG="${LOG:-/projects/_tmp/pi-$(date -u +%Y%m%d-%H%M%S).log}"
echo "==> pi ${PI_MODEL} | sessions: $SESS_DIR | log: $LOG" >&2

# (3) flags BEFORE -p. Always `</dev/null` — pi -p BLOCKS on an inherited stdin.
#     "${SESSION_ARG[@]}" is either --session-dir (new job) or --session <file> (resume).
timeout "${WALL_SECS:-3600}" pi \
    --provider "$PI_PROVIDER" \
    --model "$PI_MODEL" \
    --thinking off \
    "${SESSION_ARG[@]}" \
    --tools "read,write,edit,bash,grep,find,ls" \
    --max-tool-iterations "${MAX_ITERS:-200}" \
    -p "$(cat "$PROMPT_FILE")" \
    </dev/null 2>&1 | tee "$LOG"

echo "PI_EXIT=${PIPESTATUS[0]}"

# ---------------------------------------------------------------------------
# AFTER the run — never accept exit 0 as success:
#   * Verify resolution (ground truth, not the flags you passed):
#       F=$(find "$SESS_DIR" -name '*.jsonl' | head -1)
#       head -c 400 "$F"   # line 1 records "provider" and "modelId"
#     Expect provider=ollama-cloud. A provider of `anthropic` means the call
#     MISROUTED (flag order / env / enabled_models gate) — see the skill.
#   * Confirm the deliverable actually EXISTS (exit 0 + plausible chat output
#     does not mean the file was written).
#   * `git status --porcelain` the project: for an analysis-only job, NO source
#     file should appear modified.
#   * Run the project's real gate; a worker summary claim is not a gate result.
#
# PI'S WRITE TOOL IS PATH-SCOPED: it can be refused at a project ROOT while
# working fine in a subdirectory (observed: refused at /projects/<proj>/,
# allowed at /projects/<proj>/_tmp/). bash is not subject to that scope. If the
# worker reports it cannot write the deliverable, the fix is to write into a
# scratch subdir and `mv` into place — do NOT conclude the tool is broken.
# ---------------------------------------------------------------------------
