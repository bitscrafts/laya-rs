# Spec 16 — a flow can ask the question shape the head answers best
#: depends=- status=- block=-

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

**CORRECTED 2026-10-02 after review:** this requirement contradicted R1. The resolution: **absent
means `choice`** (R1 is the authority — it is the behaviour every committed flow relies on). A
`noul` must be asked for explicitly. A flow that wants a noul sets `question_type: "noul"`; nothing
infers it from the absence of `options`.

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

- [x] `cargo test -p laya-cli --lib a_flow_without_question_type_still_asks_a_choice 2>&1 | grep -qE '^test result: ok\. 1 passed'` : default preserved
- [x] `cargo test -p laya-cli --lib a_noul_decide_sends_no_criteria 2>&1 | grep -qE '^test result: ok\. 1 passed'` : the new path, asserting on the bytes the server receives
- [x] `cargo test -p laya-cli --lib an_unknown_question_type_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'` : refusal
- [x] `cargo test -p laya-graph --lib a_noul_decide_needs_port_coverage 2>&1 | grep -qE '^test result: ok\. 1 passed'` : a noul has no `options`, so the coverage check is vacuous unless ports must cover `yes`/`no` or carry `_`
- [x] `cargo test --workspace` : no regression

**CORRECTED 2026-10-02 after review — the four criteria above replace four that were VACUOUS.** They
named `the_documented_fields_exist_on_the_row`, which lives in **laya-store**
(`crates/laya-store/src/lib.rs:1525`); under `-p laya-graph` it matches 0 tests and printed
`test result: ok. 0 passed; ...`, satisfying `grep -qE '^test result: ok'`. The other three named
`laya-graph` although the wire payload is built in **laya-cli** (`mcp_client.rs:105-120`), and had no
grep at all. **All four passed with nothing implemented.** Every criterion above now uses
`-p <the crate that owns it>` and `grep -qE '^test result: ok\. 1 passed'` — an `ok\. 0 passed` line
must not satisfy a criterion.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` : 0
- [x] `cargo fmt --all --check` : clean
- [x] `./target/debug/laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | grep -q selected_port` : **the committed example still runs unchanged** (R1's compatibility claim, end to end)
- [x] `python3 scripts/check-doc-citations.py` : every citation resolves

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- Do NOT change the behaviour of a flow that has no `question_type`. That is the load-bearing
  safety property; a fixture that produces a different wire payload is a FAILURE, not a fix.
- Do NOT touch the execution order (`route_exec.rs`'s drain loop) or the phase split. Those are under
  review in `_docs/04-development/GENERIC-GRAPHS-ANALYSIS.md` and are explicitly out of scope here.
- Do NOT delete anything; `mv` aside and say so.
- Run the flow, do not only read the code: an exit criterion above requires a real `laya graph run`.

---

## Verification of this spec (run here, 2026-10-02, after the worker finished)

### The point, measured: a `noul` and a `choice` answer the SAME question differently

Both flows below ask *"Is it greater than 300?"* over the identical state
`"The word count in the file is 412 words."`, and both route on `label`.

```
question_type: "noul"        label=yes  band=HIGH  confidence=0.9677   -> BIG
absent (a choice, options)   label=yes  band=low   confidence=0.2389   -> (band-gated)
```

**Right label and `band: high` versus the same label at `band: low`.** The head knows the answer to the
`noul` and is merely guessing at the `choice` — which is exactly the difference the review measured
(0.875–1.0 against 0.35–0.54) and the reason this field exists. A flow could not ask for it before
today: the question type was hardcoded at `mcp_client.rs:105-115`.

### Everything else

| check | result |
|---|---|
| 4 criteria tests, in the crates that own them | **4/4 PASS**, each `ok. 1 passed` |
| the default preserved | absent `question_type` still asks a **choice** |
| the port-coverage rule for a `noul` | PASS |
| `cargo test --workspace` | **595 passed, 0 failed** |
| clippy `-D warnings` / fmt / doc citations | 0 / clean / 65 docs resolve |

### Two things worth recording

- **My own probe flow was refused by spec 20, correctly.** The first attempt had a port targeting a
  node that does not exist in the flow; validation named it (`port `_` targets `esc`, which is not a
  node in this flow — it would run nothing`). That is the check working, and it is the same shape the
  review measured as silently running *nothing* before spec 20.
- **The worker followed the brief over my phrasing on the control and said so**: it reported that the
  first run had to be re-checked, and it left `HANDOFF.md` alone because another process had modified
  it — correct, that edit was mine.

**The CLI criterion above named `laya` (the PATH binary), which is STALE** — it is not a build of this
tree and reports refused flows as valid, so the check would have passed without exercising this
change. Re-pointed at `./target/debug/laya` and verified that way. Third spec in this batch to need
that correction.
