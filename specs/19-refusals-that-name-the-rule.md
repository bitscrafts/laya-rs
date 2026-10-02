# Spec 19 — make the broken shapes fail loudly, and correct the record

## Overview

Spec 17 fixes the scheduler. This spec is the **interim guard** for anything spec 17 does not cover,
plus the corrections the review requires to documents that currently state the wrong thing. Every
item here is small; together they mean a flow author cannot write a flow that silently mis-executes
without being told which rule they broke.

**Project**: `/projects/laya-rs`
**Phase**: 19 — refusals that name the rule, and an honest record
**Depends on**: nothing (independent of 17/18; can land first)
**Source**: `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md` R4, R5, §6.

## Requirements

### R1 — `validate` refuses the shapes that mis-execute today
Each refusal must **name the node and the rule** in the `FlowError`, following the house style
(`MissingNearTiePort` at `validate.rs:79`):

- **a port target shared by two `decide` nodes** — measured: `p13-shared-target`, where both decides
  routed and **neither** handler ran. Today: silent.
- **a `depends_on` naming a node that is itself a port target** — measured: `p7-join`, where
  `needs_long` ran although `long_path` never did. Today: silent.
- **a `decide` whose `depends_on` names a non-decide** — the ordering it implies is not honoured
  (spec 17 fixes the mechanism; until it lands, refuse the shape).
- **a node id containing `/`** — the collision hole: a decide node id like `x/route` collides with
  the synthetic route id, and a node silently vanishes. One check.

Each refusal's message must say **what to do instead**, not only what is wrong.

### R2 — `escalate_on_near_tie` is refused until it is implemented
Measured on `p12-near-tie`: the flag's target has no inbound edge, so it is a root of the tail and
**runs on every run**. Until near-tie routing exists (spec 17 R4's later half), `validate` must
refuse the flag with a message saying it is not implemented. A safety field that validates and then
fires a branch for no reason is worse than one that refuses.

### R3 — the docs stop saying the withdrawn things
Four corrections, each **annotated with the date and the measurement**, never silently rewritten
(project rule: *"never erase a superseded claim; the disagreement is the finding"*):

1. `_docs/04-development/HEAD-QUESTION-SHAPE-LIMITS.md` — the claim that the 2-option `choice` is
   "the weaker shape" is **withdrawn**: measured through `laya graph run`, a choice is **15/15** under
   the v1 phrasing, and the boundary failures came from the v14c phrasing. `noul`'s advantage is
   **confidence** (0.875–1.0 vs 0.35–0.54), not label correctness.
2. `_docs/02-guides/GRAPH-EXAMPLE-WORDCOUNT.md` — its `count_words` node is a **witness, not an
   input**. Say so where the example is described, not only in a footnote.
3. `_docs/03-reference/GRAPH-SPEC.md` — a hard rule, stated where a flow author reads it:
   **a decide's state is a literal; a shell in the same flow cannot inform it** (until spec 18).
   Also: which nodes run depends on **node array order** (until spec 17).
4. `_docs/03-reference/GRAPH-SEAM.md` — link the above two rules rather than restating them.

### R4 — a checked-in regression fixture for the array-order defect
The two probe flows that demonstrate the defect (`p6-file-order` and `p6c-file-order-control`) are in
`/projects/_tmp/`. They must be **copied into the repo** as `crates/laya-graph/tests/fixtures/`, so the
defect is reproducible by anyone who clones the repo and not only on this machine. Add the live
acceptance test that asserts they now produce the **same** executed-node set (spec 17 R1's criterion)
or, until spec 17 lands, that they produce **different** sets — so the defect is *pinned* either way.
State in the test which state it is in and why; a test that documents a known-wrong behaviour must say
so, or it becomes a specification of the bug.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_port_target_shared_by_two_decides_is_refused_by_name` | two decides porting to one node | refusal naming both decides and the shared target |
| `a_depends_on_naming_a_port_target_is_refused_by_name` | `p7-join`'s shape | refusal naming the node and the target |
| `escalate_on_near_tie_is_refused_as_not_implemented` | `p12-near-tie`'s shape | refusal saying it is not implemented |
| `a_node_id_containing_a_slash_is_refused_by_name` | a decide with id `x/route` | refusal naming the id and the reason |
| `the_control_a_well_formed_flow_is_accepted` | `wc-classify-chained.flow.json` | validates — the refusals must not reject the working example |

The last row is the mandatory CONTROL for this whole spec: four new refusals must be shown **not** to
reject flows that are fine. A validator that refuses everything passes rows 1–4.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_port_target_shared_by_two_decides_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_depends_on_naming_a_port_target_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib escalate_on_near_tie_is_refused_as_not_implemented 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_node_id_containing_a_slash_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_well_formed_flow_is_accepted 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && laya graph validate _docs/02-guides/wc-classify-chained.flow.json` : still valid (the working example survives every new refusal)
- [ ] `cd /projects/laya-rs && git ls-files crates/laya-graph/tests/fixtures/ | grep -c 'p6'` : the two order fixtures are **checked in**
- [ ] the four doc corrections exist and each carries its date and measurement: `cd /projects/laya-rs && grep -l '2026-10-02' _docs/04-development/HEAD-QUESTION-SHAPE-LIMITS.md _docs/02-guides/GRAPH-EXAMPLE-WORDCOUNT.md _docs/03-reference/GRAPH-SPEC.md | wc -l` → 3
- [ ] `cd /projects/laya-rs && cargo test --workspace`
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- **Do NOT delete or silently rewrite a superseded claim** — annotate it with the date and the
  measurement that overturns it. Four docs are affected; the old wording stays visible.
- Do NOT change execution behaviour. This spec adds refusals and documentation only; if a change makes
  a valid flow behave differently, that is a defect in this change.
- Do NOT modify `specs/`.
- **Run graphs, do not only read code** (user instruction: *"graph must be ran from laya!"*).
- Do not delete anything; `mv` aside and say so.
