#!/usr/bin/env bash
# Full end-to-end verification of the laya-rs foundation.
#
# EVERY CHECK IS BINDING. This script previously ran each check and discarded its
# exit code: `set -uo pipefail` without `set -e` and with no aggregation, so the
# script's exit status was its LAST command's -- an `echo`, always 0 -- and
# `GATE EXIT=0` was reported while `check-doc-citations.py` had exited 1 with three
# unresolved citations. Measured 2026-10-01. A report that says PASS while a check
# below it failed is exactly the "check that cannot fail" class this repo has paid
# for repeatedly, so the fix is aggregation, not a louder echo.
#
# VERDICT is derived from the accumulated failures, never from the last command.
set -uo pipefail
# A script must be runnable from ANY shell. Prepending the cargo bin alone is not enough: the CLI
# criteria below invoke `laya`, so without this the whole file fails with `laya: command not found`
# when the caller's PATH lacks the install dir (measured 2026-10-02: `env -i PATH=/usr/bin:/bin`).
export PATH="$HOME/.local/bin:/usr/local/cargo/bin:$PATH"
# And pin the binary the CLI criteria test. The installed `laya` can be a STALE build -- three specs
# this session had criteria that passed against an old binary while testing nothing. Prefer this
# tree's build when it exists, so a criterion about THIS tree reads THIS tree.
if [ -x "./target/debug/laya" ]; then
  export LAYA_CLI_BIN="$PWD/target/debug/laya"
  export PATH="$PWD/target/debug:$PATH"
fi
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

# --- failure accounting -------------------------------------------------------
# FAILS accumulates "step: what" for every check that exits non-zero. Each check
# runs through `chk`, which RECORDS and still prints the full output, so the log
# stays as informative as before.
FAILS=()
UNMEASURED=()

chk() {  # chk <step-name> <command...>
  local name="$1"; shift
  "$@"
  local rc=$?
  if [ $rc -ne 0 ]; then
    FAILS+=("$name (rc=$rc)")
    echo "  >>> FAIL: $name (rc=$rc)"
  fi
  return 0   # do not abort the run: report ALL failures, not just the first
}

echo "########## 1. THE GATE (the documented command, run for real) ##########"
cargo xtask gate 2>&1 | tail -14
# `cargo xtask gate` is the fmt+clippy+test gate; its own exit code binds here.
if ! cargo xtask gate > /tmp/_gate_inner.log 2>&1; then
  FAILS+=("cargo xtask gate")
  echo "  >>> FAIL: cargo xtask gate"
  tail -6 /tmp/_gate_inner.log | sed 's/^/      /'
fi
echo

echo "########## 2. TEST COUNT ##########"
cargo test --workspace 2>&1 \
  | grep -E '^test result' | awk '{s+=$4} END {print "TOTAL PASSING (whole workspace): " s}'
echo

echo "########## 3. CLIPPY WARNING COUNT (must be 0) ##########"
C=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1 | grep -cE '^(warning|error)')
echo "count: $C"
[ "$C" = "0" ] || { FAILS+=("clippy count $C (must be 0)"); echo "  >>> FAIL: clippy count is $C"; }
echo

echo "########## 4. FMT (must be clean) ##########"
if cargo fmt --all --check; then echo "fmt: CLEAN"; else FAILS+=("cargo fmt --all --check"); echo "  >>> FAIL: fmt not clean"; fi
echo

echo "########## 4b. THE EXAMPLES RUN (spec 26: every committed flow, hermetically) ##########"
# Validation proves a flow parses; only a run proves which branch it takes. A zero-test pass
# (`ok. 0 passed`) must not satisfy this, so the passed count is required to be non-zero.
run_examples() {
  local out
  out=$(cargo test -p laya-graph --test examples 2>&1); local rc=$?
  echo "$out" | grep -E '^test |^test result'
  [ $rc -eq 0 ] && echo "$out" | grep -qE '^test result: ok\. [1-9][0-9]* passed'
}
chk "the examples run (laya-graph --test examples)" run_examples
echo

echo "########## 5. DOC LINKS (must be 0 broken) ##########"
chk "regen-index (doc links)" python3 "$(dirname "$0")/regen-index.py"
echo

echo "########## 5b. DOC CITATIONS (every citation must resolve) ##########"
# WHY THIS IS IN THE GATE: writing _docs/03-reference/GRAPH-SEAM.md I cited a module that does
# not exist (`laya-core::validate`) and nothing caught it — prose is not compiled, and the link
# check above only proves a RELATIVE LINK resolves, not that a CLAIM is true. This fails the gate
# on an unresolvable citation, so a doc cannot drift away from the code it describes.
#
# AND ITS EXIT CODE MUST BIND. This check exited 1 on 2026-10-01 while the wrapper printed
# "gate: PASS" and "GATE EXIT=0", because nothing aggregated it. `chk` is what makes it count.
chk "doc citations (3 unresolved were ignored here on 2026-10-01)" \
    python3 "$(dirname "$0")/check-doc-citations.py"
echo

echo "########## 5c. COMMIT HYGIENE (no tool named in a commit) ##########"
# HARD RULE 0c. This was broken for 35 commits (register row A24) and THREE more on
# 2026-10-01 while implementing spec 07 -- with a clean author in every case, because
# A24 fixed the author field and nothing scanned the MESSAGE. A rule enforced on one
# of the three places it applies is half a rule, and half a rule fails silently.
#
# THE BASELINE. This gate once carried a list of older commits that named a tool, accepted as debt so
# that new violations could not hide behind them. On 2026-11-10 those eight messages were reworded in a
# history rewrite, so the list is empty: every commit in the window now has to pass, and a commit that
# names a tool fails the gate. Keep the variable: it is the one place a deliberate exception would go,
# and the label prints its size so that a non-empty list is visible.
KNOWN_HYGIENE_DEBT=""
chk "commit hygiene: no NEW commit names a tool (accepted pre-fix debt: $(echo $KNOWN_HYGIENE_DEBT | wc -w))" \
    bash -c "for sha in \$(git log --format=%h -60); do
               case ' $KNOWN_HYGIENE_DEBT ' in *\" \$sha \"*) continue ;; esac
               git log -1 --format=%B \"\$sha\" | scripts/check-commit-msg.sh /dev/stdin >/dev/null 2>&1 || {
                 echo \"commit \$sha names a tool:\"; git log -1 --format=%s \"\$sha\"; exit 1; }
             done"
echo

echo "########## 6. SPECS PRESENT ##########"
ls specs/*.md | sed 's/^/  /'
echo

echo "########## 6b. THE SPECS' OWN EXIT CRITERIA ##########"
# Run every exit criterion from its OWN text. This is the check that would have caught a
# criterion naming an example target the crate never had (spec 06 cited `queue_demo`, which
# did not exist) — a spec can claim completion while its own check cannot run.
# run-exit-criteria exits non-zero on a FAILING criteria; PENDING is counted, not failed.
chk "spec exit criteria" python3 scripts/run-exit-criteria.py
echo

echo "########## 7. THE PAYLOAD CONTRACT — live ##########"
cargo run -q -p laya-core --example payload_demo 2>&1 | head -30
echo

echo "########## 7b. THE LIVE CORPUS — Rust vs the Python reference ##########"
# Spec 03's last criterion, and the ONLY check here that needs a live backbone and the Python
# reference. It is deliberately gated rather than always-on: both dependencies are outside this
# repo, so an unavailable one must be reported as UNMEASURED, not as a failure of the code.
#
# The gate is explicit and prints WHY. A silent skip would be a check that cannot fail, which is the
# failure mode this repo has already paid for twice. UNMEASURED is tracked SEPARATELY from FAILS so
# "not measured" can never be read as "passed" -- but it also does not fail the build.
if ! curl -sf -m 5 "${LAYA_URL:-http://127.0.0.1:8083}/v1/models" > /dev/null 2>&1; then
  UNMEASURED+=("live corpus: backbone :8083 unreachable")
  echo "  UNMEASURED: the backbone at :8083 is not reachable, so the live corpus cannot run."
  echo "  This is NOT a pass — run it when the backbone is up:"
  echo "    python3 scripts/live/live-corpus-parity.py"
elif [ ! -S "${LAYA_SIDECAR_SOCKET:-/run/laya/sidecar.sock}" ]; then
  UNMEASURED+=("live corpus: sidecar socket absent")
  echo "  UNMEASURED: the Python reference sidecar is not up at ${LAYA_SIDECAR_SOCKET:-/run/laya/sidecar.sock}."
  echo "  This is NOT a pass — the reference is the oracle and there is nothing to compare against."
  echo "    scripts/laya-sidecar.py   (see the sidecar's own docs to start it)"
else
  chk "live corpus parity" python3 scripts/live/live-corpus-parity.py --json /tmp/laya-parity.json
fi
echo

echo "########## 7c. THE GRAPH ACCEPTANCE TEST — laya-mcp as judge (spec 07 part 4) ##########"
# R-ACC's own gate: a real flow must route a decision through the REAL laya-mcp,
# spawned over stdio, exactly as `laya graph run` does. Same shape as §7b: gated on
# the live backbone, reported as UNMEASURED (never a silent pass) when it is down —
# an unmeasured acceptance test is not an acceptance test (the spec's own words).
if ! curl -sf -m 5 "${LAYA_URL:-http://127.0.0.1:8083}/health" > /dev/null 2>&1; then
  UNMEASURED+=("graph acceptance: backbone :8083 unreachable")
  echo "  UNMEASURED: the backbone at :8083 is not reachable, so R-ACC cannot run."
  echo "  This is NOT a pass — run it when the backbone is up:"
  echo "    cargo test -p laya-graph --test acceptance -- --include-ignored"
else
  chk "graph acceptance (R-ACC, laya-mcp live)" cargo test -p laya-graph --test acceptance -- --include-ignored
fi
echo

echo "########## 7d. THE MUTATION GATE (opt-in: --with-mutants; takes minutes; spec 13 R2) ##########"
if [ "${1:-}" = "--with-mutants" ]; then
  chk "mutation gate (zero missed mutants)" bash scripts/mutants-gate.sh
else
  echo "  skipped: pass --with-mutants to run it (this is NOT a pass, it was not run)"
fi
echo

echo "########## 8. THE TREE ##########"
find . -path ./target -prune -o -type f \( -name '*.md' -o -name '*.rs' -o -name '*.toml' \) -print | sort | sed 's/^/  /'
echo

echo "########## VERDICT ##########"
echo "  checks failed: ${#FAILS[@]}"
for f in "${FAILS[@]:-}"; do [ -n "$f" ] && echo "    FAIL  $f"; done
echo "  checks unmeasured (NOT passes): ${#UNMEASURED[@]}"
for u in "${UNMEASURED[@]:-}"; do [ -n "$u" ] && echo "    ??    $u"; done

if [ "${#FAILS[@]}" -gt 0 ]; then
  echo "VERDICT: FAILURES PRESENT"
  exit 1
fi
echo "VERDICT: ALL CHECKS PASS"
exit 0
