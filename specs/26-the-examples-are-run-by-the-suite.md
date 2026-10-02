# Spec 26 — the examples are run by the suite, not by hand

## Overview

The strongest claim this project makes about the examples — **"every committed flow runs"** — has never
been proven by the repository. One committed test **validates** every file in `_docs/02-guides/`
(`the_control_every_committed_flow_still_validates`, `validate.rs`), and nothing **runs** one. The result
was produced by a script in scratch, so a clone gets the validation check free and nothing else. Wipe the
scratch directory and no evidence remains that any example works.

That gap is not hypothetical. It hid three defects that a run would have shown immediately:

- **a decision prompt stating a literal its own counting node computed and discarded** — three examples
  ran, succeeded, and demonstrated nothing, because the value was baked in (fixed 2026-10-02);
- **a control reporting the wrong classification** for a four-hundred-word document, for the same reason;
- **which nodes execute depending on the order of the JSON array** — the severe defect spec 17 fixed,
  which a run of an order-probe fixture would have exposed years earlier.

Each was invisible to validation. **Reading and parsing a flow says nothing about what it does.**

**Project**: /projects/laya-rs · **Phase**: 26 — the examples are proof, not decoration

## Requirements

### Functional

- **R1 — a test that RUNS every committed example flow, hermetically.**
  `crates/laya-graph/tests/examples.rs`: for every `*.flow.json` under `_docs/02-guides/` and
  `crates/laya-graph/tests/fixtures/`, expand it and run it with a **scripted caller** — no head, no
  network, no `laya-mcp` — and assert:
  - every node that ran **succeeded**
  - `stopped_early` is false and `not_started` is empty
  - **the flows that are meant to exercise a branch reach the branch they name** (R2)
- **R2 — the branch assertion, per flow, from data.** Each example declares its expected outcome in a
  table **inside the test** — flow → the node ids that must run and the ones that must be skipped.
  **A test that only checks "no node failed" cannot fail correctly**: it passes when a flow routes
  everything to its catch-all. The expected sets are written out explicitly, taken from the runs recorded
  in the example documents, and a mismatch names the flow and the difference.
- **R3 — the count-driven flows prove the value drives the branch.** For the word-count family, run the
  **same flow against the short and the long document** and assert the branches **differ**. A flow whose
  prompt hardcoded the value passes R1 and R2 and still demonstrates nothing; this is the test that
  catches it, and it is the defect that was actually found in the committed examples.
- **R4 — running the examples is part of the gate.** `scripts/verify-all.sh` must run this test file, and
  the criterion must fail if a committed flow stops running. A check nobody runs is not a check.

### Non-Functional

- **NF1 — hermetic.** No head, no sidecar, no network, no `PATH` dependency. It must pass with the
  sidecar stopped and with the installed `laya-mcp` absent. **This is what makes it survive a clone.**
- **NF2 — no new crate or dependency.** `laya-graph`'s dev-dependencies already suffice.
- **NF3 — no committed flow is changed** to make the test pass. If a flow cannot run hermetically, say so
  in the report rather than weakening it or the test.
- **NF4 — it must fail when an example breaks.** Demonstrated, not asserted: break one flow on purpose
  (a hardcoded value in one of the count-driven ones, or a port pointing at the wrong node), watch the
  test go red, then restore. **A test that has never been red is a test nobody has tested.**

## Architecture

```
cargo test --workspace
  └── tests/examples.rs
        for each committed *.flow.json:
          expand(parse(file))                    <- the same loader the CLI uses
            └── run_from(graph, ScriptedCaller)  <- NO head, NO network, deterministic
                  ├── assert no node failed, nothing stopped early, nothing not_started
                  ├── assert the EXPECTED ran/skipped sets (R2)
                  └── for the count-driven pair: assert the two documents diverge (R3)
```

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `every_committed_flow_runs_to_completion` | all committed flows | no node fails, nothing stopped or not_started |
| `every_committed_flow_reaches_its_declared_branch` | the expected-outcome table | ran/skipped sets match exactly |
| `the_control_a_flow_with_a_wrong_port_is_detected` | a fixture with a mis-pointed port | the test **fails** (proves R2 bites) |
| `the_count_driven_flows_diverge_on_the_two_documents` | short vs long note | different branch for each (R3) |
| `the_control_a_hardcoded_prompt_is_detected` | a copy with the value inlined | the test **fails** (proves R3 bites) |
| `the_example_runner_is_hermetic` | sidecar stopped | the whole file still passes (NF1) |

The two controls are the load-bearing part. Without them this file would be a test that passes for every
possible input — which is a worse outcome than having no test, because it converts an open gap into a
certified one.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --test examples 2>&1 | grep -qE '^test result: ok\. [1-9][0-9]* passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --test examples every_committed_flow_reaches_its_declared_branch 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --test examples the_count_driven_flows_diverge_on_the_two_documents 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && grep -c 'examples' scripts/verify-all.sh` : at least 1 (R4)
- [ ] **with the sidecar stopped**, `cargo test -p laya-graph --test examples` : still `ok`, 0 failed (NF1)
- [ ] `cd /projects/laya-rs && cargo test --workspace` : 0 failed
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0 diagnostics
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [ ] **the break-it proof** is in the report: a deliberately broken flow made the test red, then restored

## Guardrails

- Do NOT check only "no node failed". **R2 and R3 exist because that check cannot fail correctly.**
- Do NOT use the head, the sidecar or the network. Hermetic or it does not survive a clone (NF1).
- Do NOT change a committed flow to make the test pass; report the conflict instead (NF3).
- Do NOT weaken, re-pin or delete an existing test.
- Do NOT add a crate or dependency.
- Do NOT claim a criterion you did not run. **Report the break-it proof with its output.**
- Do NOT commit. Leave the work in the tree.
- Do NOT delete anything pre-existing; `mv` aside and say why.
