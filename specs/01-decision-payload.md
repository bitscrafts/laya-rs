# Spec: 01 — Decision payload and policy (the pure core)

## Overview
The pure, testable heart of laya-rs: the question/answer contract (`laya-abi`),
the caller-side policy (thresholds, bands, composite weights) and the
route-shaped `DecisionPayload` a DAG consumes. **No I/O, no model, no network** —
everything here is verified offline.

This spec is retro-specified: the code exists (TDD'd in the same session, RED
before GREEN) and the spec records the contract so the next change is made
against a written standard rather than against the implementation.

**Status: IMPLEMENTED.** `laya-core::policy` + `route_payload`; tests green.

**Project**: /projects/laya-rs
**Phase**: 1 — P1 (pure core)

## Requirements

### Functional
- F1. Three question types exist and match Laya's wire names: `noul`, `choice`,
  `score`.
- F2. `choice` requires a non-empty option list; Laya's limit is 255 and a caller
  passing more must be rejected by the caller, not silently truncated.
- F3. An `Answer` carries the probabilities **parallel to its options**, and for a
  `choice` its `value` is always one of the caller's declared options.
- F4. `Answer::saturated()` is true when one option of a real distribution holds
  more than the saturation threshold (default 0.999), and **never** true for a
  `noul` (a single number is not a distribution).
- F5. `Policy` holds every number a decision depends on and parses from TOML:
  `high_threshold` (0.9), `low_threshold` (0.5), `noul_threshold` (0.5),
  `saturation_threshold` (0.999), `escalate_on_saturation` (false).
- F6. `Policy::band` maps a confidence to exactly one of `low` / `mid` / `high`,
  with the boundaries inclusive on the upper side (`>= high` is High, `< low` is
  Low).
- F7. `Policy::gate` returns `allow` / `deny` / `ask`; an **irreversible** action
  is only ever `allow` at the High band.
- F8. `Policy::weighted_score` applies caller-supplied weights and returns `None`
  when the weights are empty or sum to zero.
- F9. `DecisionPayload::from_answer` produces the two keys a `route` node matches
  — `label` and `band` — plus the confidence, probabilities, question id and
  saturation flag.
- F10. The payload serializes so that `route on = "$.payload.label"` and
  `"$.payload.band"` resolve from **JSON**, not merely from the Rust struct.

### Non-Functional
- NF1. `laya-abi` and `laya-core` have **no** I/O, network or model dependency,
  so their whole test suite runs offline in milliseconds.
- NF2. Zero clippy warnings, zero fmt diffs (`cargo xtask gate`).
- NF3. No `unwrap`/`expect`/`panic`/indexing in library code (workspace lints).
- NF4. Every threshold is a named field in one struct, so a threshold change is a
  visible one-line diff and never a prompt edit.

## Architecture

```
laya-abi                        laya-core
┌────────────────────┐          ┌───────────────────────────────┐
│ QuestionType       │          │ Policy                        │
│  noul|choice|score │◄─────────│  high/low/noul thresholds     │
│ Question           │          │  saturation + escalation      │
│ Answer             │          │  band() gate() weighted_score │
│  value, p, score,  │          │                               │
│  probabilities,    │          │ DecisionPayload               │
│  confidence        │─────────►│  label, band, confidence,     │
│  saturated()       │          │  probabilities, question_id,  │
└────────────────────┘          │  saturated                    │
                                └───────────────┬───────────────┘
                                                │ JSON
                                    ┌───────────▼────────────┐
                                    │  graph `route` node  │
                                    │ on = "$.payload.label" │
                                    │ rules = [{eq, port},   │
                                    │          {port}]  ←catch-all
                                    └────────────────────────┘
```

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `question_type_wire_names_match_laya` | each `QuestionType` | `"noul"`, `"choice"`, `"score"` |
| `constructors_set_the_right_kind_and_options` | the three constructors | kind + options as declared |
| `noul_is_never_saturated` | noul with `p=1.0` | `saturated()==false` |
| `a_real_saturated_choice_is_flagged` | `[1.000,0.000,0.000]` | `saturated()==true` |
| `a_normal_distribution_is_not_saturated` (CONTROL) | `[0.998,0.001,0.001]` | `saturated()==false` |
| `a_single_option_distribution_is_not_saturated` (CONTROL) | `[1.0]` | `saturated()==false` |
| `belief_prefers_p_then_falls_back_to_confidence` | `p=Some(0.42)`, conf `0.85` | `0.42`; without `p` → `0.85` |
| `answers_round_trip_through_json` | an `Answer` | equal after round-trip; `"type":"choice"` present |
| `bands_split_at_the_configured_thresholds` | 0.0,0.49,0.5,0.89,0.9,1.0 | low,low,mid,mid,high,high |
| `band_names_are_route_ready` | each `Band` | `"low"`,`"mid"`,`"high"` |
| `the_measured_reversible_case_asks_a_human` | gate(0.35, reversible) | **not** `Allow` |
| `mid_confidence_always_asks_never_denies` (CONTROL) | gate(0.7, either) | `Ask` |
| `irreversible_needs_high_confidence` | gate(0.95,irreversible) / gate(0.7,irreversible) | `Allow` / `Ask` |
| `saturated_answers_ask_only_when_configured` | gate(0.99, sat, flag off/on) | `Allow` / `Ask` |
| `noul_threshold_is_inclusive_and_in_code` | p=0.5 / 0.499 | true / false |
| `composite_score_applies_weights` | `[(.6,.9),(.3,1.0),(.1,.2)]` | weighted mean |
| `composite_score_rejects_degenerate_weights` | `[]`, all-zero weights | `None` |
| `toml_round_trip_and_defaults` | default policy | equal after round-trip |
| `a_partial_toml_takes_defaults_for_the_rest` | `high_threshold = 0.95` | other keys defaulted |
| `inverted_thresholds_are_rejected` | low 0.9 > high 0.5 | `ThresholdsInverted` |
| `equal_thresholds_are_rejected` | low == high | `Err` (Mid would be unreachable) |
| `out_of_range_thresholds_are_rejected` | 1.5; noul −0.1 | `ThresholdOutOfRange`; `NoulThresholdOutOfRange` |
| `unknown_keys_are_rejected` | typo'd key | `Err` (never silently ignored) |
| `the_default_policy_is_valid` | default | `Ok` |
| `payload_exposes_the_two_route_keys` | answer(0.95) | `label`/`band` set, `band=="high"` |
| `label_is_always_one_of_the_declared_options` | 3 answers | every label ∈ declared options |
| `a_low_confidence_routes_to_the_low_band` | conf 0.35 | `band=="low"` |
| `a_saturated_answer_is_flagged_but_keeps_its_band_by_default` | `[1,0,0]`, default policy | `saturated`, band `high` (record-only) |
| `saturation_escalation_demotes_the_band_and_never_allows` | `[1,0,0]`, escalation on | band `mid`, never `high` |
| `the_near_miss_control_stays_high` (CONTROL) | `[0.98,0.011,0.009]`, escalation on | not saturated, band `high` |
| `noul_and_score_fields_carried_through` | noul / score answers | `p` / `score` present on payload |
| `payload_round_trips_and_omits_empty_fields` | payload | equal after round-trip; keys present in JSON |

## Exit Criteria
- [ ] `cd /projects/laya-rs && cargo test -p laya-abi -p laya-core` — all pass
- [ ] `cd /projects/laya-rs && cargo clippy -p laya-abi -p laya-core --all-targets -- -D warnings` — zero
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` — zero diffs
- [ ] `cd /projects/laya-rs && cargo test -p laya-abi -p laya-core 2>&1 | grep -q 'test result: ok'`

## Guardrails
- Do NOT put a threshold inside a prompt or inside the model. Every number is a
  `Policy` field.
- Do NOT let `saturated()` flag a `noul` — it has no distribution.
- Do NOT let saturation silently rewrite the reported confidence; the flag is the
  signal, the demotion is opt-in via `escalate_on_saturation`.
- Do NOT add a model or network call to `laya-abi` or `laya-core`. If a test needs
  the model, it is a live test and belongs elsewhere.
- Do NOT hardcode Laya's temperature table here; it lives in the head's own
  metadata (see spec 03).
