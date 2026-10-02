# Spec 20 — the flow format is a contract, not a wish

## Overview

**The schema accepts any key, so a typo of any field is silently inert.** `Flow` and `Node` have no
`#[serde(deny_unknown_fields)]`; a node written `"depend_on": ["a"]` instead of `"depends_on"`
**validates as valid**, runs at ms 0 alongside its supposed predecessor, and exits 0. Field *values*
are unchecked too: four measured silent misroutes (`on: "bnad"` routes on `label` regardless;
`on: "band"` with `yes`/`no` port keys sends everything to the fallback; a port-key typo is hidden by
the `_` catch-all; a port target naming no node runs **nothing** and reports `not_started: []`).

`_docs/03-reference/GRAPH-SPEC.md` advertises `"inputs": ["issue"]` and `"prompt": "$.inputs.issue"`
in three places, and the `prompt` field's own doc comment gives the same example. **That feature does
not exist**: the string `$.inputs.doc` is sent to the head as literal text, and the head answers it
(band low, confidence 0.3985).

This spec makes every declared thing either **work**, **be refused by name**, or **be documented as
carrying no behaviour** — and adds the test that makes that true for every field added later.

**Project**: `/projects/laya-rs`
**Phase**: 20 — the format is a contract
**Source**: `_docs/04-development/OPUS-ROUND2-BUILD-ORDER.md` (N1–N5, §2, §6); every item re-verified
by the orchestrator with `laya graph run`. Ranked **first** by the review: hours of work, no execution
blast radius.

## The format delta — what this changes about the flow format

| # | the format today | after this spec | kind |
|---|---|---|---|
| 1 | any key is accepted on `Flow`/`Node`; a typo is silently inert | **`deny_unknown_fields`**; an unknown key is refused, naming it | **strictness** |
| 2 | `inputs` is accepted; `$.inputs.*` is documented as substitution in 3 places and in the `prompt` doc comment | **`inputs` and `$.inputs.*` are refused**, and every advertisement of them is removed | **shrinkage (honest)** |
| 3 | `on` accepts any string; anything but `band` silently means `label` | **`on ∈ {label, band}`**, or refused | **strictness** |
| 4 | port keys are unchecked; a typo is hidden by `_` | port keys must be **options or `_`** (`on=label`), or **band names or `_`** (`on=band`) | **strictness** |
| 5 | a port target naming no node silently runs nothing (`not_started: []`) | every port target must **exist** | **strictness** |
| 6 | `run` may be absent on a `shell` (succeeds with an empty command) | `shell.run`, `mcp.tool`, `decide.question`+`prompt` are **required for their kind** | **strictness** |
| 7 | `version: 999` is accepted | **`version == 1`**, or refused | **strictness** |
| 8 | `timeout` and `allow_failure` are declared but dead on a `decide` | refused on a `decide` **until spec 17** — then real, and this refusal is lifted | **interim refusal** |
| 9 | `retry` is declared, validated, and inert | **refused on every node** until implemented for non-`decide`, non-`writes` nodes | **interim refusal** |
| 10 | `escalate_on_near_tie` / `near_tie_port` are declared and the target runs **unconditionally** | **refused**, until near-tie routing exists | **interim refusal** |
| 11 | a node id may contain `/`, colliding with the synthetic route id | **refused** | **strictness** |
| 12 | a `subflow` node is not a `Kind` variant; it falls through to `Other` and the error blames `depends_on` | the message says **"`subflow` needs a library, which the CLI does not load"** | **message fix** |

Rows 1–7 and 11–12 are permanent and unconditional. Rows 8–10 are **interim refusals**: each is
lifted by the spec that implements it (8 by 17, 9 and 10 by their own later specs), and the test
table records which. A refusal that outlives its reason is a bug of the same class as a field that
does nothing.

**This is a compatibility question and it was measured, not assumed:** the review scanned every flow
JSON under `_docs/` and `crates/laya-graph/tests/fixtures/` — **none uses a key outside the schema**,
so strictness breaks no committed flow. One thing to check when the attribute lands: `expand.rs`
parses `subflow` nodes off the raw `Value`, so whether those nodes reach `Node`'s derive at all must
be confirmed (the review marks this **inference**).

## Requirements

### R1 — the parse layer refuses what the engine will not read
`#[serde(deny_unknown_fields)]` on `Flow` and `Node`. An unknown key is refused with the key's name
and the nearest known key (`depend_on` → *"did you mean `depends_on`?"*), so a typo is a one-second fix
rather than a silent behaviour change.

### R2 — the validate layer refuses the values the engine branches on, by name
Rows 2–7 and 11 of the table above. Every refusal message says **what to do instead**, not only what
is wrong, following the house style at `validate.rs:79`.

### R3 — the interim refusals, each naming the spec that will lift it
Rows 8–10. The message states that the field is not implemented, so the reader does not conclude their
flow is wrong in some other way. `retry`'s message offers the working alternative: *"wrap the command
in a shell loop"*.

### R4 — the field-effect test
A table in `crates/laya-graph/src/field_contract.rs` (`#[cfg(test)]`):

```rust
enum Effect {
    /// Two fixtures differing ONLY in this field. Both run through validate -> lower -> run_flow with
    /// a ScriptedCaller (label chosen per node id; no head needed). `differs` must return true.
    Observable { with: &'static str, without: &'static str,
                 differs: fn(&RunReport, &RunReport) -> bool },
    /// Refused until implemented; validate must return an error containing this text.
    Refused { fixture: &'static str, error_contains: &'static str },
    /// Carries no behaviour by design. The reason is required and reviewed.
    Documentary { reason: &'static str },
}
const NODE_FIELDS: &[(&str, Effect)] = &[ /* every field of Node */ ];
const FLOW_FIELDS: &[(&str, Effect)] = &[ /* every field of Flow */ ];
```

Three tests:

1. **`every_serialised_field_has_a_row`** — build a **fully populated** `Node` (every `Option` is
   `Some`, every `bool` true) and `Flow`, take `serde_json::to_value(..)`'s keys, and require them to
   equal the table's names. **This is the part no future field can escape**: adding a field without a
   row fails. A fully populated value is required because `skip_serializing_if` hides `None`.
2. **`every_row_holds`** — iterate the table; each row must hold.
3. **`an_unknown_key_is_refused`** — a `depend_on` fixture must fail to parse. This pins the root
   cause.

Wire-level fields (`question_type`, `options` → `criteria`) are observable in **`laya-cli`**, not in
`laya-graph`. Give them the same table shape there, asserting on the bytes a fake server receives —
that pattern already exists in `the_decide_translation_also_carries_the_ids`.

### R5 — the documentation stops advertising what does not exist
`_docs/03-reference/GRAPH-SPEC.md` is the format's reference. Remove every `$.inputs` example
(three at lines ~32, ~40, ~103) and the `prompt` field's doc comment example in `flow.rs:169`; replace
with what is true: **the prompt is literal state text**. The withdrawn wording is annotated with the
date and the measurement (*"never erase a superseded claim"*), not silently deleted.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `every_serialised_field_has_a_row` | a fully populated `Node` and `Flow` | the serialised keys **equal** the table names |
| `every_row_holds` | the table | every row holds |
| `an_unknown_key_is_refused` | `"depend_on"` | a parse error naming the key |
| `the_control_a_known_key_is_accepted` | `"depends_on"` | parses — strictness must not refuse valid flows |
| `an_unknown_port_key_is_refused_by_name` | ports `{"yes","noo","_"}` | refusal naming `noo` |
| `a_port_target_that_names_no_node_is_refused_by_name` | `"no": "nosuch"` | refusal naming `nosuch` |
| `a_band_routing_with_label_port_keys_is_refused_by_name` | `on: "band"`, ports `yes`/`no` | refusal |
| `the_control_a_shell_with_a_run_is_accepted` | `{"id":"a","kind":"shell","run":"echo a"}` | valid |
| `inputs_and_dollar_inputs_are_refused_by_name` | `inputs: ["doc"]`, prompt `$.inputs.doc` | refusal naming `inputs` |
| `retry_is_refused_and_names_the_alternative` | `retry: {attempts: 3}` | refusal mentioning a shell loop |
| `the_control_every_committed_flow_still_validates` | every `.flow.json` under `_docs/` and `tests/fixtures/` | **all valid** |

The controls are load-bearing. Several refusals must be shown **not** to reject the flows that work —
including `wc-classify-chained.flow.json`, which uses the gate-then-ask pattern. (An earlier draft of
spec 19 proposed refusals too broad to pass this; they were dropped for exactly this reason.)

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib every_serialised_field_has_a_row 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib an_unknown_key_is_refused 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib an_unknown_port_key_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_port_target_that_names_no_node_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib inputs_and_dollar_inputs_are_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib retry_is_refused_and_names_the_alternative 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] **every committed flow still validates, run through the CLI**: `cd /projects/laya-rs && for f in $(git ls-files '_docs/**/*.flow.json' 'crates/laya-graph/tests/fixtures/*.json'); do laya graph validate "$f" >/dev/null || echo "BROKE: $f"; done` → no output
- [ ] `cd /projects/laya-rs && grep -c 'inputs' _docs/03-reference/GRAPH-SPEC.md` : the `$.inputs` examples are gone (or annotated as withdrawn, per the project rule)
- [ ] `cd /projects/laya-rs && cargo test --workspace`
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves

**Every criterion uses `grep -qE '^test result: ok\. 1 passed'`.** Not `ok\.` — a criterion that
accepts `ok. 0 passed` is satisfied by a test name that matches nothing, which is how spec 16's
criteria came to pass with nothing implemented. Verify each criterion is non-vacuous by running it
**before** the work.

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- **Do NOT weaken an existing test to make the suite pass.** A test that encoded a now-refused shape
  must be re-pinned with a comment naming the refusal and this spec — or removed with the reason
  recorded. Never silently relaxed.
- Do NOT delete or silently rewrite a superseded claim in the docs; annotate it with the date and the
  measurement.
- Do NOT change execution behaviour: this spec adds strictness and refusals at the parse/validate
  layer. If a valid flow behaves differently, that is a defect in this change.
- Do NOT modify `specs/`.
- **Run graphs, do not only read code** (user instruction: *"graph must be ran from laya!"*). The
  committed-flows criterion requires real `laya graph validate` output.
- Do not delete anything; `mv` aside and say so.
