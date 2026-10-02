# Spec 23 — the scheduler cannot wait forever

## Overview

The mutation gate reports **two mutants that hang** rather than fail, and both are replacements of
`dependency_tables` (`crates/laya-graph/src/sched.rs:89`) that return a table inconsistent with the
graph they were handed. **`missed.txt` is empty** — zero mutants survived — but the hangs expose a
production failure mode, not a test artefact:

- `dependency_tables` is the **shared** source of the graph's shape for `topo_order`, `refuse_cycle`
  and the `Tracker`. Its own doc comment says the sharing exists *"so the two can never disagree about
  the graph's shape"* — and nothing checks that the table is about the graph at all.
- `refuse_cycle` is **the only cycle guard**, and it is built on the same table. A table wrong in this
  way defeats the single check protecting the DAG assumption the scheduler's termination relies on.
- **There is no spin bound anywhere.** Greps in `sched.rs` for `steps`, `debug_assert`, `unreachable!`,
  `spin`, `iterations` return nothing.

So a scheduler that never settles a node **waits forever instead of reporting**, holding fanout permits,
with no test name in the output — the reason `cargo-mutants` classifies it separately and the gate exits
**3** rather than **2**.

**Project**: `/projects/laya-rs`
**Phase**: 23 — a hang becomes a named failure
**Source**: `_docs/04-development/SCHEDULER-SINGLE-GUARD.md` (measured from the gate's own preserved
artefacts, 2026-10-02).

## Requirements

### R1 — the invariant is asserted at its source
`dependency_tables` must produce an **entry for every node id in `graph.nodes`**. If not, fail fast with
a message **naming the missing ids** — not a panic without context, and not silence. Every consumer
(`topo_order`, `refuse_cycle`, the `Tracker`) depends on this, so it belongs where the table is built.

### R2 — the scheduler's progress is bounded, and a stall is NAMED
`dispatch` (`sched.rs:405`) and the settle loop must be unable to run unbounded. Concretely: assert that
every iteration either **spawns**, **settles** or **skips** a node, and that no node is settled twice.
On violation, return a `RunError` variant naming **the nodes left unsettled** and **which edges never
resolved** — the same shape as `RunError::Cycle`, which already names its leftovers.

**A hang with no name is the worst possible signal.** The goal is that a stall reports in a second, with
the offending node ids, instead of a 37-second silence.

### R3 — the guard is tested by breaking it, not by reading it
A test must **construct** the failure the mutants represent — a `Tracker`/graph whose dependency table
is deliberately inconsistent — and assert the **named error**. Reading the code cannot show that the
bound fires; the mutation gate already showed that nothing currently does.

### R4 — the mutation gate's criterion is stated as the property it means
Spec 17's criterion says `mutants-gate.sh` exits 0. That is **stronger than the property**: it demands
no hanging mutants in addition to zero missed ones, and a hang is neither a pass nor a miss. Amend the
criterion (in spec 17, annotating rather than erasing) to: **`missed.txt` is empty, and every entry in
`timeout.txt` is explained by this spec's R1–R2** — i.e. a hanging mutant is acceptable **only** where
the code now fails fast on that input. If R1/R2 land correctly, those two mutants should change
classification (they will fail fast, and so be **caught**), and `timeout.txt` should become empty; the
criterion then reads `exit 0` again, honestly.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `an_inconsistent_dependency_table_is_refused_by_name` | a graph and a table missing a node id | an error naming the missing id, **not** a hang |
| `the_control_a_consistent_table_is_accepted` | the same graph with a correct table | no error — the check must not fire on valid input |
| `a_scheduler_that_cannot_settle_a_node_reports_which_one` | a constructed unsatisfiable node | a `RunError` naming that node, returned promptly |
| `the_control_a_satisfiable_graph_completes` | a normal DAG | completes; the bound must not turn a slow run into a failure |
| `no_node_is_settled_twice` | a fan-in graph | each node appears once in the settle accounting |

Rows 1 and 3 are the ones the mutants would exercise; rows 2, 4 and 5 are the mandatory controls —
**a bound that fires on every run passes row 1**, so the near-misses are what make it a test.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib an_inconsistent_dependency_table_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_consistent_table_is_accepted 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_scheduler_that_cannot_settle_a_node_reports_which_one 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_satisfiable_graph_completes 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] **the mutants stop hanging** — `cd /projects/laya-rs && bash scripts/mutants-gate.sh` : `timeout.txt` is now **empty** and `missed.txt` is empty. Paste both counts. **This is the criterion the spec exists for**, and it must be the real run, not a subset.
- [ ] `cd /projects/laya-rs && cargo test --workspace`
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean

**Every criterion uses `-p <the owning crate>` and `ok\. 1 passed`.** A criterion that accepts
`ok. 0 passed` is satisfied by a test name matching nothing. **Verify each is non-vacuous before
starting** — and note that the mutation-gate row can only be judged by reading `missed.txt` and
`timeout.txt`, **not** by the script's exit code, because the exit code conflates the two.

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- **Do NOT mark the two mutants `#[mutants::skip]`** or add an exclusion. That would hide the finding
  rather than fix it, and it is the one thing that would make this spec worse than doing nothing.
- **Do NOT make the bound so tight that a legitimately slow graph fails.** The control runs a normal DAG
  for exactly this reason.
- Do NOT weaken an existing test; re-pin one with a comment naming this spec if the accounting changes.
- Do NOT modify `specs/` except spec 17's criterion, and there **annotate** the change rather than
  erasing the old wording (project rule: the disagreement is the finding).
- **Run graphs, do not only read code** (operator instruction, verbatim: *"graph must be ran from
  laya!"*).
- Do not delete anything; `mv` aside and say so.
