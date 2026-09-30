#!/usr/bin/env bash
# Full end-to-end verification of the laya-rs foundation.
set -uo pipefail
export PATH="/usr/local/cargo/bin:$PATH"
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
cd /projects/laya-rs

echo "########## 1. THE GATE (the documented command, run for real) ##########"
cargo xtask gate 2>&1 | tail -14
echo

echo "########## 2. TEST COUNT ##########"
cargo test --workspace 2>&1 \
  | grep -E '^test result' | awk '{s+=$4} END {print "TOTAL PASSING (whole workspace): " s}'
echo

echo "########## 3. CLIPPY WARNING COUNT (must be 0) ##########"
C=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1 | grep -cE '^(warning|error)')
echo "count: $C"
echo

echo "########## 4. FMT (must be clean) ##########"
cargo fmt --all --check && echo "fmt: CLEAN"
echo

echo "########## 5. DOC LINKS (must be 0 broken) ##########"
python3 "$(dirname "$0")/regen-index.py" 2>&1 | tail -2
echo

echo "########## 5b. DOC CITATIONS (every citation must resolve) ##########"
# WHY THIS IS IN THE GATE: writing _docs/03-reference/GRAPH-SEAM.md I cited a module that does
# not exist (`laya-core::validate`) and nothing caught it — prose is not compiled, and the link
# check above only proves a RELATIVE LINK resolves, not that a CLAIM is true. This fails the gate
# on an unresolvable citation, so a doc cannot drift away from the code it describes.
python3 "$(dirname "$0")/check-doc-citations.py"
echo

echo "########## 6. SPECS PRESENT ##########"
ls specs/*.md | sed 's/^/  /'
echo

echo "########## 6b. THE SPECS' OWN EXIT CRITERIA ##########"
# Run every exit criterion from its OWN text. This is the check that would have caught a
# criterion naming an example target the crate never had (spec 06 cited `queue_demo`, which
# did not exist) — a spec can claim completion while its own check cannot run.
python3 scripts/run-exit-criteria.py
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
# failure mode this repo has already paid for twice.
if ! curl -sf -m 5 http://10.10.10.217:8083/v1/models > /dev/null 2>&1; then
  echo "  UNMEASURED: the backbone at :8083 is not reachable, so the live corpus cannot run."
  echo "  This is NOT a pass — run it when the backbone is up:"
  echo "    python3 scripts/live-corpus-parity.py"
elif [ ! -S "${LAYA_SIDECAR_SOCKET:-/run/laya/sidecar.sock}" ]; then
  echo "  UNMEASURED: the Python reference sidecar is not up at ${LAYA_SIDECAR_SOCKET:-/run/laya/sidecar.sock}."
  echo "  This is NOT a pass — the reference is the oracle and there is nothing to compare against."
  echo "    scripts/laya-sidecar.py   (see the sidecar's own docs to start it)"
else
  python3 scripts/live-corpus-parity.py --json /tmp/laya-parity.json 2>&1 | tail -14
fi
echo

echo "########## 8. THE TREE ##########"
find . -path ./target -prune -o -type f \( -name '*.md' -o -name '*.rs' -o -name '*.toml' \) -print | sort | sed 's/^/  /'
echo
echo "########## DONE ##########"
