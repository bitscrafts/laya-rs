#!/usr/bin/env bash
# Mutation gate (spec 13 R2): zero missed mutants on the safety-critical files.
#
# TMPDIR and --gitignore true are BOTH load-bearing. By default cargo-mutants copies the
# whole tree into $TMPDIR including gitignored files -- this repo's .venv-laya is 5.4 GB --
# and measured 2026-10-01 that filled the root filesystem ("No space left on device").
# So: the copy goes under /projects (large disk), gitignored files are not copied, and the
# scratch directory this run created (mktemp -d, nothing pre-existing) is removed afterwards.
#
# Exits with cargo-mutants' own exit code (0 = all caught, 2 = missed, 3 = timeout, 4 = baseline failed).
set -uo pipefail
export PATH="/usr/local/cargo/bin:$PATH"
cd /projects/laya-rs || exit 1

BASE=/projects/_tmp/mutants-tmp
OUT=/projects/_tmp/mutants-out-laya-rs
mkdir -p "$BASE" "$OUT" || exit 1
# A scratch dir created by THIS run and nothing else: the only thing this script ever removes.
SCRATCH="$(mktemp -d "$BASE/run.XXXXXX")" || exit 1
[ -n "$SCRATCH" ] && [ -d "$SCRATCH" ] || exit 1
export TMPDIR="$SCRATCH"

TARGETS=(
  crates/laya-core/src/policy.rs
  crates/laya-core/src/agentic.rs
  crates/laya-core/src/route_payload.rs
  crates/laya-graph/src/sched.rs
)
FILE_ARGS=()
for f in "${TARGETS[@]}"; do FILE_ARGS+=(--file "$f"); done

cargo mutants --gitignore true --output "$OUT" "${FILE_ARGS[@]}" --cargo-test-arg=--lib "$@"
rc=$?

# Cleanup: only the mktemp -d directory made above, and only if it still has that exact prefix.
case "$SCRATCH" in
  "$BASE"/run.??????) rm -rf -- "$SCRATCH" ;;
  *) echo "refusing to clean unexpected scratch path: '$SCRATCH'" >&2 ;;
esac
echo "mutants-gate: cargo-mutants exit $rc; results in $OUT/mutants.out"
exit $rc
