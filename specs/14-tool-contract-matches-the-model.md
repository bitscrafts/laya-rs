# laya-rs — spec-14: the tool contract matches the model — questions, refusals, and a contract test

- **Project**: `/projects/laya-rs`
- **Priority**: P0 for R1–R3 (each is a caller-visible failure on the advertised surface); P1 for R4–R5.
- **Status**: SPECIFIED — the three defects below are **fixed in the working tree** as of `447f7a3`
  onwards; this spec exists so the fix is certified, extended to every argument, and cannot regress.
- **Source**: the ten-levels harness, `scripts/check-ten-levels.py`, run 2026-10-02. Its first run
  reported 3/11 and exposed three defects in the tool's ADVERTISED contract, each found by an
  agent-shaped caller failing.
- **Depends-On**: none. Touches `crates/laya-mcp` only.

---

## Verified Premises

All measured 2026-10-02, through the deployed `laya-mcp` over stdio, on the shipped surface.

- `decide`'s schema declared `"questions": {"type": "array", "description": "typed questions
  (noul/choice/score)"}` — an array with **no item shape** (`crates/laya-mcp/src/lib.rs:295`
  before this change). Sending an array produced, verbatim:
  `laya-mcp: the decision head is unavailable: sidecar returned an error: AttributeError: 'list'
  object has no attribute 'keys'`. **The model takes an object map keyed by question id.**
- The question's text field was **not described in the schema at all**. The only name available
  to a caller was `text`, from `laya_abi::Question` (`crates/laya-abi/src/lib.rs:74`). Sending
  `text` produced: `KeyError: 'instructions'` — on every question in the call. **The model
  requires `instructions`.**
- Neither schema marked `criteria` as required for `choice`/`score`. A `score` without it
  produced `TypeError: 'NoneType' object is not iterable`. The model's own docstring
  (`laya/agent.py:272-274`) states the shape and the
  requirement:
  `choice: {"type","instructions","criteria":{opt:desc}}`, `score: {..., "criteria":[…]}`,
  `noul: {"type","instructions"}`.
- `laya_core::agentic::validate_questions` **already** refuses a non-object
  (`AskFailure::QuestionsNotAnObject`) and a `choice`/`score` without `criteria`
  (`QuestionWithoutCriteria`) — but **`decide` never called it**: `decide_via_sidecar` forwarded
  `args.questions` straight to the model. So the refusal existed for `ask` and not for `decide`,
  which is a rule enforced on one of two paths.
- The first run of `scripts/check-ten-levels.py` reported **3/11**; correcting the harness's own
  invented inputs raised it to **7/11**, and the remaining four were these defects plus one
  assertion on a response field that does not exist. **Eight of the first eleven failures were
  the check's fault, not the tool's** — recorded because it is the same class as the defects.

---

## Overview

The tool's advertised schema and the model's actual requirements disagree on three points, and a
caller that follows the schema fails. That is worse than a bug in the logic: the caller is told to
do one thing and the system requires another, and the error it gets comes from inside the model's
own Python, naming neither the argument at fault nor the fix.

The fix has three parts, in this order of value:

1. **Advertise what the model reads.** One schema function, used by every tool that takes
   questions, matching the model's documented shape exactly.
2. **Refuse a malformed question by name, in front of the model.** A message that says which
   question id and which field, and — for the `text`/`instructions` confusion — names the rename.
3. **Test the wiring, not just the validator.** A control proved this was missing: disabling the
   validator left every validation test passing, because they called it directly. Tests now drive
   the handler.

Then extend the same treatment to the remaining arguments, because these three were found by
accident and there is no reason to think they are the only ones.

---

## Requirements

### Functional

- **R1: one question schema, advertised by every tool that takes questions.**
  `questions_schema()` returns a JSON Schema object whose `additionalProperties` requires
  `type` and `instructions` and documents `criteria` per type. `decide` and `ask` both use it, so
  they cannot drift. Today they are edited independently and only one would get fixed.

- **R2: `decide` refuses a malformed question by name, before the model sees it.**
  `validate_question_shape` is called on the decide path, and each refusal names the offending
  question id and field:
  - not an object → says the argument must be an object and shows the shape
  - missing `instructions` → says so, and if `text` is present, **names the rename**
  - `choice`/`score` missing `criteria` → says so, and gives the shape that type needs
  - `choice` whose `criteria` is empty → refused, because it offers nothing to choose
  - unknown `type` → refused, naming the value received
  Critically: **no caller ever sees `AttributeError` or `KeyError`.** Those are the model's
  internal errors and they leak through today.

- **R3: the wiring is tested through the handler, not the validator.** At least three tests drive
  `handle("decide", …)`. The control is that **disabling the validator call site fails them**;
  measured before this fix, disabling it failed **nothing**.

- **R4: the same audit for every other argument.** `RawArgs` is deserialised with
  `serde_json::from_value(args_value).unwrap_or_default()` (`crates/laya-mcp/src/lib.rs:1584`), so
  a wrongly-typed argument silently becomes the default and surfaces as *"requires X"* — naming
  the wrong problem. Measured precedent: an `ask_file` call with an **object** `question` returned
  `-32602 "ask_file requires path"` while `path` was supplied. Every tool argument gets the same
  treatment as `questions`: an advertised type, and a refusal that names it.

- **R5: the schema is asserted as a contract.** A test reads `questions_schema()` and asserts the
  three things that were wrong — object not array, `instructions` required, `criteria`'s
  requirement stated — so the schema cannot silently drift back.

### Non-Functional

- **NF1: a refusal costs no model call.** Verified by inspecting the model's stderr: the
  `AttributeError`/`KeyError` runs show the request reaching the model. After the fix, a refused
  call must produce no model traffic.
- **NF2: the refusals are readable by the caller that made the mistake**, not by the maintainer.
  Each names the argument, the wrong value, and the required shape.
- **NF3: one implementation per rule.** `questions_schema` and `validate_question_shape` are the
  single definitions; a second copy anywhere is the defect class this project keeps finding.

---

## TDD Contract

| Test | Input | Expected |
|---|---|---|
| `the_advertised_schema_states_the_three_things_the_defects_got_wrong` | `questions_schema()` | object; `instructions` required; `text` NOT advertised; `criteria`'s requirement in the description |
| `every_tool_that_takes_questions_advertises_the_same_schema` | `tool_meta` for `decide` and `ask` | both `properties.questions` are objects and require `instructions` |
| `an_array_of_questions_is_refused_by_name` | a list of questions | refused; message contains `must be an OBJECT` |
| `text_instead_of_instructions_is_refused_and_names_the_rename` | `{"type":"noul","text":"…"}` | refused; message contains ``not `text` `` |
| `a_score_without_criteria_is_refused_by_name` | `{"type":"score","instructions":"…"}` | refused; message names `ARRAY` and `ascending` |
| `a_choice_without_criteria_is_refused_and_says_which_shape` | `{"type":"choice","instructions":"…"}` | refused; message names `OBJECT mapping each option` |
| `a_choice_with_an_empty_criteria_offers_nothing_to_choose` | `criteria: {}` | refused; message contains `no option to choose` |
| `a_noul_needs_no_criteria` (near-miss) | `{"type":"noul","instructions":"…"}` | **accepted** — the rule must not demand `criteria` of all three types |
| `a_well_formed_question_map_is_accepted` (control) | all three types, correct | accepted |
| `decide_refuses_a_malformed_question_through_the_handler` | array, via `handle("decide", …)` | refused by the validator, and the message contains no `AttributeError`/`KeyError` |
| `decide_refuses_a_text_field_through_the_handler_and_names_the_rename` | `text`, via `handle` | refused, names the rename |
| `decide_refuses_a_score_without_criteria_through_the_handler` | score, via `handle` | refused |
| **control: unwire the validator** | comment out the call site | the three `through_the_handler` tests **FAIL** |

---

## Architecture

```
  caller sends questions
        │
        ▼
  questions_schema()  ──────────►  advertised in tools/list
        │                          (one definition, both tools)
        │
        ▼
  validate_question_shape()  ────►  refusal NAMING the field, before the model
        │
        ▼
  sidecar.predict()  ─────────────►  the model reads:
                                     {id: {type, instructions, criteria?}}
```

The order matters: the refusal must come **before** the model, or the caller is handed an error
from a layer it cannot see and cannot fix.

---

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib the_advertised_schema_states_the_three_things_the_defects_got_wrong 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R5
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib every_tool_that_takes_questions_advertises_the_same_schema 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib an_array_of_questions_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib text_instead_of_instructions_is_refused_and_names_the_rename 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2 — the rename must be named
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_score_without_criteria_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_choice_with_an_empty_criteria_offers_nothing_to_choose 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_noul_needs_no_criteria 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2's control
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_well_formed_question_map_is_accepted 2>&1 | grep -qE '^test result: ok\. 1 passed'`: the control
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib decide_refuses_a_malformed_question_through_the_handler 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3 — the WIRING
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib decide_refuses_a_text_field_through_the_handler 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib decide_refuses_a_score_without_criteria_through_the_handler 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [ ] **The control, run by hand:** with the `validate_question_shape(...)` call site in
  `decide_via_sidecar` commented out, all three `through_the_handler` tests must FAIL. Measured
  before this fix: they did not exist, and disabling the validator failed **nothing**.
- [ ] `cd /projects/laya-rs && uv run scripts/check-ten-levels.py 2>&1 | grep -q 'VERDICT: all ten levels'`: the end to end, through the shipped surface
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py 2>&1 | grep -q 'every citation resolves'`: docs still honest
- [ ] **R4 (not yet implemented):** `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_wrongly_typed_argument_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'` — fails today

---

## Guardrails

- Do NOT add a second question schema. `questions_schema()` is the definition; a copy is the
  defect class this project keeps removing.
- Do NOT make the refusal a `CallToolResult::success` whose text begins "error". A failure must be
  a protocol error, or a caller that only checks for success mistakes it for an answer.
- Do NOT weaken `validate_question_shape` to make a caller's malformed input pass. The whole point
  is that the caller is told.
- Do NOT assert on response fields the tools do not have. Three of this spec's own findings were
  checks reading `payloads`/`answers`/`files_read` where the tool sends something else.
- Do NOT let a test call `validate_question_shape` directly and call it coverage. The control that
  proves the wiring is the only test that proves the wiring.

---

## Provenance

- `scripts/check-ten-levels.py` — the harness that found this, and the record of the eight
  self-inflicted failures that came first.
- `_docs/04-development/TEN-LEVELS-MAPPING.md` — the findings, with the verbatim model errors.
- The model's authority for the shape: the pinned model venv, `laya/agent.py`, `system_one`'s docstring (`pip show laya` → 0.3.5).
