#!/usr/bin/env bash
# run-spec28-implement.sh — dispatch the workhorse lane (pi-workhorse harness) for laya-rs spec 28.
# Two-lane doctrine (hermes/rules/two-lane-pi-analysis-then-workhorse-2026-10-06): the ANALYSIS was
# pi+glm-5.3:cloud read-only; the IMPLEMENTATION goes through the pi-workhorse harness from THIS script.
#
# usage: bash scripts/run-spec28-implement.sh [implement|repair|review]
set -uo pipefail
export PATH="/usr/local/cargo/bin:/root/.local/bin:$PATH"
set -a; . /root/.hermes/.env 2>/dev/null; set +a   # harness loads keys too; explicit for the worker
unset PI_PROVIDER PI_MODEL                          # pid 1 injects the paid topup — never trust it

export ORCH_HOME="/root/.pi-orchestration"
# The chain tier vars BIND; ORCH_MODEL/ORCH_PROVIDER are documented-inert nvidia-legacy overrides
# (measured 2026-10-06: ORCH_MODEL=glm-5.3-flash:cloud was ignored, the run went to nemotron).
# Override the TIER, not the legacy var.
export TIER_ollama_MODEL="glm-5.3-flash:cloud"      # worker tier = glm-5.3-flash (user directive; IP repo
                                                    # excludes empero; nemotron is transcription-class)
export TIER_ollama_PLANNER_MODEL="glm-5.3:cloud"    # planner/escalation lane = glm-5.3 FULL
export ORCH_ESCALATION_PROVIDER="ollama-cloud"
export ORCH_ESCALATION_MODEL="glm-5.3:cloud"        # escalate to the FULL glm-5.3 (user: 'if needed')
export ORCH_CHAIN="ollama deepseek"                 # empero removed THIS lane: laya-rs is IP work,
                                                    # empero logs + may train (config.env caveat)
export ORCH_THINKING="off"
export ORCH_MAX_TOOL_ITERATIONS="${ORCH_MAX_TOOL_ITERATIONS:-150}"   # 60 was the binding cap: the RED
                                                    # phase consumed all 60 writing tests+fixture
export WALL_SECS="${WALL_SECS:-2400}"

cd /projects/laya-rs || exit 2
PHASE="${1:-implement}"
SPEC="specs/28-decision-identity-is-visible.md"
echo "== workhorse lane: $PHASE $SPEC  worker=$TIER_ollama_MODEL  esc=$ORCH_ESCALATION_MODEL  chain=$ORCH_CHAIN"
/root/.pi-orchestration/bin/pi-workhorse.sh "$PHASE" "$SPEC" /projects/laya-rs
RC=$?
echo "WORKHORSE_EXIT=$RC"
exit $RC