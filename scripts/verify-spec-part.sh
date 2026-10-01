#!/usr/bin/env bash
# verify-spec-part.sh -- INDEPENDENTLY verify a dispatched spec part.
#
# The worker's report is a SELF-REPORT. This script drives the real artefact and
# prints a PASS/FAIL row per criterion, so the claim is re-checkable without the
# session. User's rule: "everything is a script -- the run AND the verification".
#
# USAGE
#   scripts/verify-spec-part.sh 3           # full verification
#   scripts/verify-spec-part.sh 3 --quick   # tests + clippy only
#
# EXIT: 0 when every checked criterion passes, 1 otherwise.
# Evidence file: /projects/_tmp/spec07-part<N>-verify-<timestamp>.txt

set -uo pipefail

REPO=/projects/laya-rs
PART="${1:-}"
QUICK=0
[ "${2:-}" = "--quick" ] && QUICK=1

[ -n "$PART" ] || { echo "usage: $0 <part> [--quick]" >&2; exit 1; }

export PATH="/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup

TS=$(date -u +%Y%m%dT%H%M%SZ)
EVIDENCE="/projects/_tmp/spec07-part${PART}-verify-${TS}.txt"
mkdir -p "$(dirname "$EVIDENCE")"

FAILED=0
PASSED=0
declare -a ROWS

# row <name> <command...>  -- runs, records PASS/FAIL, tees output to the evidence file.
row() {
  local name="$1"; shift
  local out rc
  out=$("$@" 2>&1); rc=$?
  if [ $rc -eq 0 ]; then
    PASSED=$((PASSED+1)); ROWS+=("PASS  $name")
  else
    FAILED=$((FAILED+1)); ROWS+=("FAIL  $name  (rc=$rc)")
  fi
  {
    echo "----- $name  [rc=$rc] -----"
    echo "$out" | tail -25
  } >> "$EVIDENCE"
  return $rc
}

echo "=== verifying spec 07 part ${PART}   $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "    evidence: $EVIDENCE"
echo

cd "$REPO" || exit 1

# --- 0. the worker must not have committed ------------------------------------
# Compared against the HEAD RECORDED AT DISPATCH, not against "is the tree clean".
# The earlier version asserted `git status -- specs/` was empty and called that
# "worker did not commit" -- which is a different property, and it failed on the
# orchestrator's own spec corrections.
HEAD_FILE="/projects/_tmp/spec07-dispatch/part${PART}.head"
if [ -f "$HEAD_FILE" ]; then
  PRE=$(cat "$HEAD_FILE")
  row "worker did not commit (HEAD == pre-dispatch ${PRE:0:9})" \
      bash -c "test \"\$(git rev-parse HEAD)\" = \"$PRE\""
else
  echo "  NOTE: no recorded HEAD for part ${PART} (dispatched before the scripts existed);"
  echo "        the commit check is SKIPPED, not passed. Verify by hand."
fi

# --- 1. the crate's tests -----------------------------------------------------
row "cargo test -p laya-graph" cargo test -p laya-graph
row "tests are non-vacuous (a [1-9] count line exists)" \
    bash -c 'cargo test -p laya-graph 2>&1 | grep -qE "^test result: ok\. [1-9]"'

# --- 2. lint bar: ZERO, not "below baseline" ----------------------------------
row "clippy clean (zero, -D warnings)" \
    cargo clippy -p laya-graph --all-targets -- -D warnings
row "fmt clean" cargo fmt --all --check

if [ "$QUICK" = 0 ]; then
  # --- 3. guardrails ----------------------------------------------------------
  row "specs/ untouched"                bash -c 'git diff --quiet -- specs/'
  row "payload contract untouched"      bash -c 'git diff --quiet -- crates/laya-core/src/route_payload.rs'
  row "G16 no laya-store dep"           bash -c '! grep -qE "^laya-store|^redb" crates/laya-graph/Cargo.toml'
  row "G16 no store access in source"   bash -c '! grep -rqE "redb|Ledger::open" crates/laya-graph/src/'
  row "N5 tokio present + features"     bash -c 'grep -q "rt-multi-thread" crates/laya-graph/Cargo.toml'
  row "N5 no second async runtime"      bash -c '! grep -qE "^async-std|^smol|^futures-executor" crates/laya-graph/Cargo.toml'

  # --- 4. whole-workspace regression -----------------------------------------
  row "workspace tests (no regression)"  cargo test --workspace
  row "workspace clippy clean"           cargo clippy --workspace --all-targets -- -D warnings

  # --- 5. the criteria runner discovers this spec ----------------------------
  row "run-exit-criteria discovers part ${PART}" \
      bash -c "python3 scripts/run-exit-criteria.py 2>&1 | grep -qE 'pass=[0-9]+ fail=0|Pending|not dispatched' ; python3 scripts/run-exit-criteria.py 2>&1 | grep -q '07-graph-executor-part${PART}'"
fi

echo
echo "=== RESULTS"
for r in "${ROWS[@]}"; do echo "  $r"; done
echo
echo "  passed: $PASSED   failed: $FAILED"
if [ "$FAILED" -eq 0 ]; then
  echo "VERDICT: ALL CRITERIA PASS"
  exit 0
fi
echo "VERDICT: FAILURES PRESENT -- do NOT commit"
exit 1
