# Spec 15 — the `wc` example, rebuilt on a measured boundary
#: depends=- status=- block=-

## Overview
The first `wc` example routed on a 3-option `choice` and produced the **same** label for a 13-word
and a 412-word document, so its control certified nothing. The cause is now **measured**, and it is
not the head's ability to read numbers.

**Project**: /projects/laya-rs
**Phase**: 15 — worked examples that encode a measured boundary

## The measurement this spec is built on (2026-10-02, re-runnable)

Same head, same options, only the number varying:

| question shape | 13 words | 412 words | verdict |
|---|---|---|---|
| 3-option `choice` "SHORT/MEDIUM/LONG" | MEDIUM | MEDIUM | **does not track**; confidence 0.09–0.26 (band `low`) |
| `noul` "greater than 300?" | no (1.0) | yes (0.9189) | **tracks**, including the 300/301 boundary |
| `noul` "greater than 100?" | no (1.0) | yes (1.0) | **tracks**, including the 100/101 boundary |

14 of 14 correct across both thresholds, tested at 5/13/50/100/299/300/301/412/1000/5000.

A separate finding, recorded because it is dangerous: a **bare `noul`** ("Is this feedback
positive?") answered **`no` at confidence 1.0 for both** clearly-positive and clearly-negative text
— a confident constant. The two-option **`choice`** form answered both correctly. So a `noul` whose
subject the head cannot bind is worse than no question: it is confidently wrong, and `band` will
report `high`.

## Requirements

### R1 — a 3-way classification built from TWO threshold questions
A `decide` node asks `noul` "Is the word count greater than 300?"; on `yes` the flow is LONG. On
`no` a second `decide` node asks "Is the word count greater than 100?" — `yes` is MEDIUM, `no` is
SHORT. This is the shape the measurement supports, and it is the fix: the engine's own
`choice`-over-3 on this head does not track the number.

### R2 — the pair is its own control, and it must differ
Two flows over two real documents that land in **different** terminal branches. A 13-word and a
412-word document must NOT reach the same branch. If they do, the pair is decorative and must be
reported as failing.

### R3 — a low-confidence answer escalates, it does not pick a branch
The flow routes on **`band`**, not `label`, at the threshold questions. Measured justification: on
the 3-option phrasing the head reported `band=low` on every input while the `label` still nominated
a branch — so a `label` route acts on an answer the head has just disowned.

### R4 — the two measured limits are documented where the example lives
`_docs/02-guides/GRAPH-EXAMPLE-WORDCOUNT.md` must state, with the numbers above:
the engine has **no shell→route interpolation** (`lower.rs:104` copies `prompt` verbatim;
`route_exec.rs:70-76` refuses a non-`mcp` upstream), and the 3-option-`choice` / bare-`noul` limits
with their measurements.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `wc_threshold_low_route_differs_from_high_route` | 13-word flow vs 412-word flow, run live | different `selected_port` in each run |
| `the_control_a_forged_equal_count_reaches_one_branch` | both flows pointed at the SAME count | the pair collapses — proves the difference above came from the data, not the graph |
| `a_low_band_escalates_rather_than_selecting_a_label_branch` | a state the head bands `low` | `selected_port` is an escalate port |
| `a_shell_node_cannot_feed_a_route_without_a_decide_node` | a graph whose route upstream is a shell node | refused, not silently mis-routed |

## Exit Criteria

- [x] `cd /projects/laya-rs && ./target/debug/laya graph validate _docs/02-guides/wc-classify-chained.flow.json` _(CORRECTED 2026-10-02: named `laya` on PATH, which a bare shell does not have; the tree's own binary is the thing to test)_ : exit 0, "is valid"
- [x] `cd /projects/laya-rs && ./target/debug/laya graph validate _docs/02-guides/wc-classify-chained.control.flow.json` : exit 0, "is valid"
- [x]  `cd /projects/laya-rs && ./target/debug/laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | grep -q 'selected_port'` : the run report carries a routing decision
- [x]  `cd /projects/laya-rs && ./target/debug/laya graph run _docs/02-guides/wc-classify-chained.control.flow.json 2>&1 | grep -q 'selected_port'` : likewise for the control
- [x]  the two runs' `selected_port` values DIFFER (the pair is a real control) — paste both reports
- [x] `cd /projects/laya-rs && python3 scripts/check-wc-boundary.py` : re-runs the threshold measurement and exits 0 only if both thresholds are 0-wrong
- [x]  `cd /projects/laya-rs && cargo test --workspace` : no regression
- [x]  `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [x] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves

## Guardrails
- Do NOT commit; leave the work in the tree.
- Do NOT add a crate or dependency.
- Do NOT modify any source file under `crates/`. If you believe the engine must change, **stop and report** — this spec is about using the graph correctly, not changing it.
- Do NOT delete anything; `mv` aside and say so.
- Do NOT weaken a threshold to make a run "work". If the head cannot do the comparison, that FAILS and gets reported.
- Route on `band`; never add a port that swallows a `low` band into a confident branch.
