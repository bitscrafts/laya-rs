# Spec 16 — a flow can ask the question shape the head answers best

## Overview

A `decide` node's question type is **hardcoded to `choice`** at
`crates/laya-cli/src/mcp_client.rs:105-115`, and a `Node` has no field to override it. So a flow can
ask a `choice` over its options and nothing else — and this head answers a `noul` threshold far
better than a `choice` over the same content:

| question shape | measured result |
|---|---|
| `noul` "Is the word count greater than 300?" | **15/15** over 5..5000, incl. the 299/300/301 boundary (conf 0.89–1.0) |
| 2-option `choice` — the graph's only form | **20/22**, failing AT both boundaries (conf 0.163 at 301, 0.014 at 101) |

So the head's clean arithmetic is unreachable from any flow, and every flow written today inherits
boundary errors at every threshold it tests. This spec adds the field and wires it, so a flow author
can choose the shape.

**Project**: `/projects/laya-rs`
**Phase**: 16 — the flow surface can express what the head is good at

## Requirements

### R1 — `Node` gains `question_type`
`crates/laya-graph/src/flow.rs`: add `pub question_type: Option<String>` to `Node`, with
`#[serde(default, skip_serializing_if = "Option::is_none")]` and a doc comment stating the accepted
values and the default. Absent means the current behaviour (`choice`), so **every existing flow keeps
working unchanged** — a flow with no `question_type` must produce byte-identical behaviour to today.

### R2 — `validate` refuses an unknown type by name
`crates/laya-graph/src/validate.rs`: a `question_type` that is not one of the accepted values is a
`FlowError` naming the node id and the bad value. Follow the existing refusal style (`FlowError`
variant with `id`, like `MissingNearTiePort` at `validate.rs:79`). Do **not** silently fall back to
`choice` — a typo must not change the question shape without saying so.

Also: `question_type: "noul"` on a node that declares `options` is a contradiction (a `noul` has no
options). Decide the rule and state it in the refusal or in the doc — do not leave it undefined.

### R3 — the lowering carries the type
`crates/laya-graph/src/lower.rs`: `lower_decide` must forward `question_type` into the mcp node's
arguments so the caller can read it. Keep the existing keys unchanged (`question`, `options`,
`prompt`) — add, do not replace.

### R4 — the caller honours it
`crates/laya-cli/src/mcp_client.rs`: the `decide` branch builds `questions.answer` from
`question_type` instead of hardcoding `"choice"`:
- `noul` (or absent-with-no-options) → `{"type": "noul", "instructions": question}` — **no
  `criteria`**. A `noul` with criteria is refused by the server.
- `choice` (or absent, the default) → today's exact shape, `criteria` built from `options`.
- Anything else must never reach here if R2 holds; if it does, return an error rather than guessing.

### R5 — the contract is documented where a flow author looks
`_docs/03-reference/GRAPH-SPEC.md` (the flow format reference) must state the field, its accepted
values, the default, and the measured reason to prefer `noul` for a threshold question. One place,
linked from `_docs/03-reference/GRAPH-SEAM.md`.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_flow_without_question_type_still_asks_a_choice` | an existing fixture, unchanged | the wire payload is byte-identical to before the change |
| `a_noul_decide_sends_no_criteria` | `question_type: "noul"` | the wire `questions.answer` has `type=noul` and **no** `criteria` key |
| `an_unknown_question_type_is_refused_by_name` | `question_type: "yolo"` | `FlowError` naming the node id and the value |
| `the_control_a_choice_decide_still_carries_its_options` | `question_type: "choice"` | `criteria` is present and keyed by the option strings |

The first two rows are the pair that matters: R1's whole safety argument is that absent == today.
If the first row cannot fail when the default is changed, the test is decorative — **verify it fails
by changing the default to `noul` and watching it go red, then restore.**

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_documented_fields_exist_on_the_row 2>&1 | grep -qE '^test result: ok'` : the schema test still passes with the new field
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_flow_without_question_type_still_asks_a_choice` : default preserved
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_noul_decide_sends_no_criteria` : the new path
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib an_unknown_question_type_is_refused_by_name` : refusal
- [ ] `cd /projects/laya-rs && cargo test --workspace` : no regression
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [ ] `cd /projects/laya-rs && laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | grep -q selected_port` : **the committed example still runs unchanged** (R1's compatibility claim, end to end)
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- Do NOT change the behaviour of a flow that has no `question_type`. That is the load-bearing
  safety property; a fixture that produces a different wire payload is a FAILURE, not a fix.
- Do NOT touch the execution order (`route_exec.rs`'s drain loop) or the phase split. Those are under
  review in `_docs/04-development/GENERIC-GRAPHS-ANALYSIS.md` and are explicitly out of scope here.
- Do NOT delete anything; `mv` aside and say so.
- Run the flow, do not only read the code: an exit criterion above requires a real `laya graph run`.
