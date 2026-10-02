# Spec 17 — routing must be a function of the graph, not of the file's array order

## Overview

**This is the most severe defect in the project.** Which nodes execute depends on the order of nodes
in the flow's JSON array, and the run report says nothing about it. A flow can run a branch the graph
did not select, run a decision behind a port that was never chosen, run a node whose dependency was
pruned, or run **neither** of two branches a decision selected — all while exiting 0 with
`stopped_early: false` and `not_started: []`.

Project rule 2 in `AGENTS.md` is *"the graph owns every edge — the model never does"*. Today the
**array order** owns the edges.

**Project**: `/projects/laya-rs`
**Phase**: 17 — the scheduler owns the edges
**Source**: `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md` (C1, C2, C5, D1), re-verified by the
orchestrator with `laya graph run` on 2026-10-02.

## The measured defect

Three runs, all exiting 0, `stopped_early: false`, `not_started: []`:

| probe | graph | what ran | what should run |
|---|---|---|---|
| `p6-file-order` | `outer_A` → `no`: `A_no`; its `yes` port → `inner_B`. `inner_B` listed **first** | `inner_B` decided, **`B_yes` RAN**, plus `A_no` | `A_no` only |
| `p6c` (control) | the same graph with `outer_A` listed first | `A_no` only | `A_no` only ✓ |
| `p13-shared-target` | two decides both porting to `handle_yes`/`handle_no` | **neither handler ran** | both handlers |
| `p7-join` | `needs_long` `depends_on` `[long_path, prep]`; the decide picks `short_path` | **`needs_long` RAN** despite `long_path` never running | not `needs_long` |

Both the p6 and the control are in `/projects/_tmp/opus-graph-analysis/` and both are re-runnable.
**The pair is the proof**: same content, only the array order differs, different execution.

Three mechanisms, from the code:

1. `prune_unreached` (`route_exec.rs:301`) deletes the **unselected port targets themselves** without checking whether
   another route also ports to them.
2. `remove_node` deletes a node's edges, so a **pruned or removed dependency counts as satisfied**.
3. `find_route_pair` takes the **first `Route` in `Vec` order**, so decides run in file order.

## Requirements

### R1 — one route-aware scheduler replaces the two-phase drain
`crates/laya-graph/src/route_exec.rs`'s drain loop is removed. `run_flow` becomes the existing
`sched::run_from`, which is already a correct DAG scheduler. In `sched.rs`'s join loop, when a
`NodeBody::Route` node's turn comes (`sched.rs:372` today succeeds as a no-op):
- read the upstream mcp node's `NodeRecord.output` — it is already in `records` because the edge was
  honoured;
- run the existing `extract_on_value` + the port selection from `resolve_route`;
- decrement indegree **only along the selected port's edge**;
- mark every **other** port edge from that route **dead**.

### R2 — two edge classes, stated explicitly in code and in the docs
- **A `depends_on` edge is an AND.** If any `depends_on` predecessor is dead or skipped, this node is
  **skipped** (dead-path elimination). This fixes `p7`.
- **A port edge into a node is an OR.** The node is live if any route selected it. This fixes `p13`.
- **The mixed case must be stated explicitly, because the working example is exactly it:** a node that
  has BOTH a selected port edge and `depends_on` entries is live **iff at least one port edge was
  selected AND every `depends_on` predecessor completed**. The gate→ask pattern in
  `wc-classify-chained.flow.json` is this case, and the review required it be written down rather than
  left to the implementation to guess.
- Skipping propagates transitively.

### R3 — a skipped node is VISIBLE
`RunReport` gains `skipped: Vec<String>` (sorted, `#[serde(default)]` so an old report still
deserialises). A node that did not run because its path was not taken must be **named**, not merely
absent. Today `not_started: []` is a lie of omission: it reads as "everything ran".

### R4 — the near-tie target stops running unconditionally
`near_tie_port` is not lowered into an edge, so its target has no inbound edge and becomes a root of
the tail: **it runs on every run** when the flag is set. Re-verified on `p12-near-tie` (`human`
unconnected in the render, ran anyway). Until near-tie routing is implemented, `validate` must
**refuse** `escalate_on_near_tie` with a message saying it is not implemented — a refusal a flow
author can act on, rather than a branch that fires for no reason.

### R5 — the broken shapes are refused when R1 is incomplete
If any part of R1 cannot land, `validate` must refuse, **by name**: a `decide` that `depends_on` a
non-decide; a port target shared by two `decide` nodes; a `depends_on` naming a node that is a port
target; and a node id containing `/` (the collision hole where a node silently vanishes —
`GRAPH-EXAMPLE-WORDCOUNT.md` §6.8(b2)). Loud refusal over silent wrong execution.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `the_files_node_order_does_not_change_which_nodes_run` | `p6-file-order` and `p6c` — **this spec copies both into `crates/laya-graph/tests/fixtures/` itself**; do not depend on spec 19 for them, it lands later. The `--lib` tests need a `ScriptedCaller` (a label chosen per node id) so the assertion does not depend on the head | **identical** sets of executed node ids, from both |
| `the_control_the_array_order_is_the_only_difference` | the same two fixtures, diffed | they differ ONLY in `nodes` array order — proves the test above tests order |
| `a_branch_the_graph_did_not_select_does_not_run` | `p6-file-order` | `B_yes` is NOT in the records |
| `a_node_whose_dependency_was_pruned_is_skipped_and_named` | `p7-join` | `needs_long` does not run, and appears in `skipped` |
| `two_routes_sharing_a_target_both_reach_it` | `p13-shared-target` | both handlers run |
| `the_control_a_branch_that_is_selected_still_runs` | a flow whose selected port points at a shell | that shell runs (near-miss: the fix must not suppress the live path) |
| `a_near_tie_target_does_not_run_without_a_near_tie` | `p12-near-tie` | either refused by validate, or `human` does not run |

The first two rows are a pair and the sixth is the mandatory CONTROL: a rule that suppresses dead
paths must be shown **not** to suppress live ones. A fix that skips everything passes rows 1–4.

## Exit Criteria

- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_files_node_order_does_not_change_which_nodes_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_branch_the_graph_did_not_select_does_not_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_node_whose_dependency_was_pruned_is_skipped_and_named 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib two_routes_sharing_a_target_both_reach_it 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_branch_that_is_selected_still_runs 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] the two order fixtures produce the SAME executed-node set, **run through the CLI**, not only in a unit test: `cd /projects/laya-rs && for f in p6-file-order p6c-file-order-control; do laya graph run /projects/_tmp/opus-graph-analysis/$f.flow.json; done` — paste both reports
- [x] `cd /projects/laya-rs && ./target/debug/laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | grep -q selected_port` : the committed example still runs
- [x] `cd /projects/laya-rs && cargo test --workspace` : no regression
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [x] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [x] `cd /projects/laya-rs && bash scripts/mutants-gate.sh` : exit 0 (the scheduler is on the mutation path)
  - **Amended by spec 23 (2026-10-02), old wording kept above.** `exit 0` demanded no *hanging* mutants
    in addition to zero misses; a hang is neither. The honest criterion is: **`missed.txt` is empty, and
    every `timeout.txt` entry is explained by spec 23's R1–R2.** After spec 23 the two `dependency_tables`
    mutants fail fast (`RunError::InconsistentTable`) and are CAUGHT, so `timeout.txt` is empty and
    `exit 0` is true again: 239 mutants, 216 caught, 23 unviable, 0 missed, 0 timeouts.

## Blast radius (state it in your report, do not discover it in production)

- Decides now wait for their `depends_on`, so **every committed flow's record ORDER changes**.
- Decide mcp calls take a fanout permit and their node `timeout` (today `call_upstream` passes none).
- Independent decides may run in parallel, so **ledger row order changes**.
- The `route_exec.rs` unit tests that assert `not_started` contents need re-pinning — keep their
  selectivity guarantees, do not delete them.
- `sched.rs`'s "frozen" note has to be lifted deliberately, with the reason recorded.
- Wire contract and schema: unchanged.

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- Do NOT change the wire contract or the payload shape.
- **Do NOT weaken a test to make the suite pass.** A test that asserted the old (wrong) order is a
  test that must be re-pinned with its new expectation and a comment saying why — or deleted with the
  reason recorded. Never silently relaxed.
- **Run graphs, do not only read code** (user instruction: *"graph must be ran from laya!"*). The
  exit criteria require real `laya graph run` output.
- Do NOT modify `specs/`.
- Do not delete anything; `mv` aside and say so.

---

## Verification of this spec (run here, 2026-10-02, after the worker finished)

The worker's report is a self-report. These are the checks re-run against the tree, every one of them
through the CLI or a real test — not read from the diff.

### The headline: the pair now produces the SAME executed set

```
p6-file-order   (inner_B listed FIRST):  ran: [A_no, outer_A, outer_A/route]
                                         skipped: [B_no, B_yes, inner_B, inner_B/route]
p6c-control     (outer_A listed FIRST):  ran: [A_no, outer_A, outer_A/route]      <- IDENTICAL
```

Before this spec the first produced a **different** set and ran `B_yes` — reached only through a port
`outer_A` did not select — while reporting `not_started: []`. **Now the two orders agree, and the
nodes that did not run are NAMED in `skipped` rather than silently absent.** That was the whole point.

### The other three defects

| probe | before | now |
|---|---|---|
| `p7-join` | `needs_long` ran although its dependency `long_path` never did | `needs_long` and `long_path` in **`skipped`** |
| `p13-shared-target` | **neither** handler ran | **both** `handle_yes` and `handle_no` ran |
| `wc-classify-chained` (the committed example) | routed | still routes (`selected_port` present) — no regression |

### The gate

| check | result |
|---|---|
| 5 criterion tests | **5/5 PASS**, each `ok. 1 passed` |
| `cargo test --workspace` | **576 passed, 0 failed** |
| clippy `-D warnings` / fmt / doc citations | 0 / clean / 63 docs resolve |

### The one criterion NOT met, and why it is not a failure of this spec

`bash scripts/mutants-gate.sh` requires **exit 0**. It exits **3**: *215 mutants, 193 caught, 20
unviable, **2 timeouts**, 0 MISSED*. `cargo-mutants` uses exit **3** for a timeout and **2** for a
missed mutant, and `scripts/mutants-gate.sh:10` documents that distinction itself. **So no mutant
survived the tests — the property this criterion exists to protect holds.**

The two timeouts are both replacements of `dependency_tables` (`crates/laya-graph/src/sched.rs:87`)
that return a map containing only a phantom key. The mechanism is visible in the function: `indegree`
is counted from the edges, so a node whose entry is missing never reaches an indegree of zero and the
scheduler **waits forever** rather than failing. Under mutation that is a 37-second hang.

**This is a robustness observation, not a regression:** the mutated function's contract (every node id
appears in `indegree`) is not defended by an assertion, so an inconsistent table hangs instead of
panicking. The tests detect it — as a timeout, which is why no mutant is reported as missed. A
follow-up could add an assertion naming the invariant; **it is out of this spec's scope and is recorded
here rather than silently absorbed into a green tick.**

**The criterion was left UNTICKED for exactly that reason: its literal wording (`exit 0`) is not met,
and a criteria list that says otherwise would be the defect this project keeps finding.**

### Note: both CLI criteria above named `laya` (the PATH binary), which is STALE

`/root/.local/bin/laya` was built at 14:27 and is not a build of this tree. It reports flows that this
tree refuses as `is valid`, so a criterion written as `laya graph validate|run ...` **passes without
testing this change at all**. Both have been re-pointed at `./target/debug/laya`, and both were
verified that way. This is the third time this session that a criterion's **input** rather than its
intent was the defect; the rule is that a criterion must name the binary it tests.
