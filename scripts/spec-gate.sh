#!/usr/bin/env bash
# scripts/spec-gate.sh -- run a spec's exit criteria and report, in one compact line.
#
# WHY A SCRIPT UNDER THE GRAPH: a graph is static and a spec's criteria are not, so the SHELL does the
# dynamic work (parse the criteria out of the spec, run them) and the GRAPH does the deciding and the
# routing. That split is what makes a graph able to follow a spec at all.
#
# exit 0 = every criterion passed | 1 = at least one failed | 2 = could not even start
# stdout is ONE line, so the whole verdict costs a handful of tokens instead of a transcript.
#
# FAST MODE -- and why it exists (measured 2026-10-02):
# A full sweep of every spec ran for OVER TWENTY MINUTES and had not finished, because three specs'
# criteria include the mutation gate, which takes roughly twelve minutes EACH. For a daily "is this
# spec done?" question that is the wrong trade: the expensive criteria prove QUALITY, not COMPLETENESS.
#   FAST=1  skips the criteria that are known to take minutes, and SAYS SO in the line
#   (default) runs everything, which is what you want before a release or a commit
set -uo pipefail
cd /projects/laya-rs

SPEC="${1:-}"
[ -n "$SPEC" ] || { echo "usage: spec-gate.sh <spec-file>"; exit 2; }
[ -f "$SPEC" ] || { echo "no such spec: $SPEC"; exit 2; }
FAST="${FAST:-0}"

# criteria known to be minutes-long: the mutation gate and whole-workspace runs
# every form of the mutation run (measured: four specs use `cargo mutants` directly, not the
# scripts/mutants-gate.sh wrapper, and missing that made FAST mode not fast at all)
EXPENSIVE='mutants-gate|cargo[ -]mutants|cargo test --workspace|verify-all'

mapfile -t ALL < <(python3 - "$SPEC" <<'PY'
import re, sys
for line in open(sys.argv[1]):
    m = re.match(r'\s*- \[[x ]\]\s*`(.+?)`', line)
    if m:
        print(m.group(1))
PY
)
total_all=${#ALL[@]}
[ "$total_all" -gt 0 ] || { echo "$(basename "$SPEC"): no runnable criteria found"; exit 2; }

CRITS=(); skipped=0
for c in "${ALL[@]}"; do
  if [ "$FAST" = "1" ] && printf '%s' "$c" | grep -qE "$EXPENSIVE"; then
    skipped=$((skipped+1))
  else
    CRITS+=("$c")
  fi
done

note=""
[ "$skipped" -gt 0 ] && note=" (+$skipped slow criteria not run: FAST=1)"

pass=0; fail=0; first_fail=""
for c in "${CRITS[@]}"; do
  if bash -c "$c" >/dev/null 2>&1; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    [ -z "$first_fail" ] && first_fail="$c"
  fi
done

if [ "$fail" -eq 0 ]; then
  echo "$(basename "$SPEC"): DONE $pass/$total_all criteria passed${note}"
  exit 0
fi
echo "$(basename "$SPEC"): NOT DONE $pass/$total_all passed, $fail failed; first failure: ${first_fail:0:90}${note}"
exit 1
