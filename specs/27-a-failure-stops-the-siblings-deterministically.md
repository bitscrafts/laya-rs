# Spec 27 — a failure stops the siblings deterministically

## Overview

Two tests assert that a failure stops queued siblings from starting, and both assert it against a
**wall-clock race**, so they fail intermittently — measured **3 times in 20 runs under load** at a commit
that predates every recent change, and 0/20 under light load. A test that fails one run in seven will
fail in continuous integration, on someone else's machine, at the worst moment.

This is a **harness** defect, not a product defect: no node runs late, and the assertions are about
dispatch ordering. But it is a defect, and this project's own standard is that a test whose control is a
stopwatch is a test that will fail elsewhere.

**Project**: /projects/laya-rs · **Phase**: 27 — deterministic dispatch

## Requirements

### Functional

- **R1 — the property must hold deterministically, not usually.** With a fanout limit of one, a node that
  fails must stop every sibling that has not yet been dispatched. The scheduler must **observe the halt
  before spawning the next node**, rather than draining the ready set and discovering the failure
  afterwards.
- **R2 — the tests must not race a clock.** `a_failure_stops_queued_siblings_from_starting` and
  `a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not` currently rely on
  `sleep 0.2; exit 1` interleaving with the scheduler's first pass. They must be made deterministic:
  - the failing node signals **when it has reached the point under test** (a file or a channel the test
    waits on), so the ordering under test is *controlled* rather than *raced*; and
  - **the assertions must stay as strict as they are** — a sibling must still be proven not to have run.
- **R3 — the controls must keep biting.** The `allow_failure` arm of the timeout test must still show the
  sibling **does** run. If R1–R2 made that arm pass vacuously, the test would be worse than the flake.

### Non-Functional

- **NF1 — no wall-clock assertion anywhere in either test.** A bounded wait for a **signal** is fine; a
  sleep whose duration decides the outcome is not.
- **NF2 — the fix must not serialise the engine.** Independent nodes must still run concurrently:
  `a_timed_out_node_...` and the fanout tests must still show parallel dispatch where it is expected.
- **NF3 — no new crate or dependency.**

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_failure_stops_queued_siblings_from_starting` | failing node + signalling | siblings did **not** run; **no clock decides it** |
| `a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not` | strict vs lenient arms | strict: sibling did not run; lenient: sibling **did** run |
| `the_control_a_late_failure_still_stops_later_siblings` | failure after the first pass | a sibling in a **later** pass does not run |
| `the_control_independent_nodes_still_run_concurrently` | two independents | both run (NF2 — the fix did not serialise) |
| `the_control_the_signal_is_required` | remove the signal | the test **times out/fails**, proving it waits on the signal and not the clock |

The third and last controls are the point: the first proves the halt survives past the first pass, and the
last proves the test is genuinely synchronised rather than accidentally green.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_failure_stops_queued_siblings_from_starting 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_independent_nodes_still_run_concurrently 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] **stability**: 20 consecutive runs of the two tests, **0 failures**, and the same 20 **under load**
      (4 busy-loop generators on this host), reported with the counts
- [ ] `cd /projects/laya-rs && cargo test --workspace` : 0 failed
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0 diagnostics
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean

## Guardrails

- Do NOT fix this by lengthening a sleep or raising a timeout. **The sleep is the defect.**
- Do NOT weaken either assertion. The strict arm must still prove a sibling did not run, and the
  `allow_failure` arm must still prove one did.
- Do NOT serialise the engine to make the race go away (NF2).
- Do NOT mark either test `#[ignore]`, `#[flaky]`, or delete it.
- Do NOT add a crate or dependency.
- Do NOT commit. Leave the work in the tree.
- Do NOT delete anything pre-existing; `mv` aside and say why.

## Evidence of the flake (do not re-derive it)

`_docs/05-operations/TEST-HARNESS-FLAKE.md` records: **3 failures in 20 runs at commit `10fc381` under 4
busy-loop generators**, and **0/20 pre- and post-change at light load**. The affected tests are the two
above. Do not spend the budget re-measuring; spend it making them deterministic.
