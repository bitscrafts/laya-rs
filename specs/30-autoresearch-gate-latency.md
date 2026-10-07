# Spec: 30 — Autoresearch loop #1: laya gate latency

**Project**: /projects/laya-rs
**Phase**: first real run of the autoresearch-loop skill ON this repo's own machinery
**Status**: NOT STARTED (STUB mechanics first, worker run after)
**Derives from**: laya-rs rule 11 (autoresearch = the standing test bed); 05 seam review
(SPEC-A's loop half); skill `autoresearch-loop` (selftest 16/16)

## What this loop is (honest)

Metric: **laya gate latency** — p50 wall-clock of 5 `laya-gate "ls"` calls against the
DEPLOYED binaries/head (ms, lower is better). The loop optimizes the same decision path
it exercises; every experiment produces a (laya verdict, next-metric outcome) pair — the
calibration corpus G1 needs — regardless of keep/discard.

The worker's allowed target under this spec is deliberately **NOT the laya runtime**
(the head's 460-token CPU forwards dominate p50 ≈ 481 ms; no overnight agent edit should
touch the real inference path). The loop's first run therefore proves the LOOP, not a
runtime win: candidate edits are documented hypotheses recorded on ledger rows; keeps
require a >25 ms sustained improvement, which noise must not fake.

## spec config (planner-authored; workers cannot edit specs)

<!-- autoresearch-config
TAG=gate-lat
METRIC_SCRIPT=scripts/ar-metric-gate-latency.sh
WORKER_CMD=STUB
ALLOWED_TARGETS=README.md
MAX_EXPERIMENTS=6
WINDOW_END_UTC=2026-10-08T06:00:00Z
KEEP_THRESHOLD=25
EXPERIMENT_TIMEOUT_SECS=240
-->

```yaml
autoresearch-config:
  TAG: gate-lat
  METRIC_SCRIPT: scripts/ar-metric-gate-latency.sh
  WORKER_CMD: pi            # STUB first for mechanics; then glm-5.3-flash:cloud
  ALLOWED_TARGETS: README.md
  MAX_EXPERIMENTS: 6
  WINDOW_END_UTC: "2026-10-08T06:00:00Z"
  KEEP_THRESHOLD: "25"      # ms; p50 must beat best by this much to keep
  EXPERIMENT_TIMEOUT_SECS: 240
```

## Requirements

### R1 — metric script is real and quiet
`scripts/ar-metric-gate-latency.sh` times 5 sequential `laya-gate "ls" --run-id ar-metric`
calls (date +%s%N around each, LAYA_LEDGER_DIR exported), prints ONE number: the p50 in
integer ms. No other output; exit 0 even if a call is slow. (Those are REAL gate decisions
— ledger rows, outcomes joinable — that is intended and part of the corpus.)

### R2 — loop runs the skill's driver with THIS spec
`autoresearch-driver.sh /projects/laya-rs $PWD/scripts/ar-metric-gate-latency.sh
specs/30-autoresearch-gate-latency.md gate-lat`; ledger at the project's `_tmp/autoresearch/
results-gate-lat.tsv`; one HANDOFF.md line per experiment; worktree
`/projects/_tmp/autoresearch-wt/laya-rs-gate-lat` on branch `autoresearch/gate-lat`.

### R3 — keep-seam live
Every experiment row records `laya_keep=yes|no conf=0.xx` from `laya decide -` (state =
change + measured result). Verdicts are data; the METRIC alone decides keep/discard
(rule 5: one signal; calibration flips enforcement later, on proof).

### R4 — STUB mechanics proof (zero spend) BEFORE the worker run
`WORKER_CMD=STUB` run: baseline row + up to 6 stub experiments; all edit-rows discarded
(noise cannot beat +25 ms); tree restored clean after every discard; ledger/HANDOFF/
worktree state verified; then (and only then) the real flash run.

### R5 — guardrails
No edits outside ALLOWED_TARGETS (violation → restored + logged); worktree-only edits;
main tree untouched between driver calls; specs/ read-only; never commit to master from
the loop; quota window respected (window guard); no destructive commands — move aside.

## Exit Criteria

- [ ] `bash scripts/ar-metric-gate-latency.sh | grep -qE '^[0-9]+$'`
- [ ] `[ "$(grep -c 'baseline' /projects/laya-rs/_tmp/autoresearch/results-gate-lat.tsv)" -ge 1 ]`
- [ ] `[ "$(grep -c 'laya_keep=' /projects/laya-rs/_tmp/autoresearch/results-gate-lat.tsv)" -ge 3 ]`
- [ ] `bash /root/.hermes/skills/software-development/autoresearch-loop/scripts/selftest.sh` (16/16 must stay green)

## Non-goals

- No laya runtime/latency change is expected from THIS spec's first run; a real latency
  intervention (pinned python sidecar, persistent head session) is its own spec after the
  runtime profile is measured in anger.