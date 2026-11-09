#!/usr/bin/env bash
# run-spec29-implement.sh — dispatch the workhorse lane for laya-rs spec 29 (a policy can hard-deny).
# Two-lane doctrine; flash=workhorse; one harness run per spec.
set -uo pipefail
export RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
export PATH="/usr/local/cargo/bin:/root/.local/bin:$PATH"
export IS_SANDBOX=1
set -a; [ -f /root/.hermes/.env ] && . /root/.hermes/.env; set +a
export ORCH_HOME="/root/.pi-orchestration"
export TIER_ollama_MODEL="glm-5.3-flash:cloud"
export ORCH_THINKING="off"
export ORCH_MAX_TOOL_ITERATIONS="${ORCH_MAX_TOOL_ITERATIONS:-150}"
export WALL_SECS="${WALL_SECS:-3600}"
echo "== workhorse lane: laya-rs spec 29 (gate_deny) worker=$TIER_ollama_MODEL"
exec /root/.pi-orchestration/bin/pi-workhorse.sh run \
  /projects/laya-rs/specs/29-a-policy-can-hard-deny.md /projects/laya-rs