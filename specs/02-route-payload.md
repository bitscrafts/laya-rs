# Spec: 02 — The route-shaped decision payload

## Overview
`laya-rs` must hand a DAG's `route` node a payload whose `label` and `band` are
matchable with plain member access and exact string equality, because that is all
pii-graph's expression language supports.

**Status: IMPLEMENTED.** `laya-core::route_payload` (tests in `specs/01`, green).
This spec records the wire contract so a change is made against a written standard.

**Project**: /projects/laya-rs
**Phase**: 1 — P1

## Requirements

### Functional
- F1. The payload has a `label` field: the answer as a routing label.
- F2. The payload has a `band` field: `low` | `mid` | `high`, **computed by us**
  from `Policy`, never by a `transform` node (blocker B5).
- F3. The payload carries `confidence`, `probabilities`, `options`,
  `question_id` and `saturated`.
- F3a. The payload carries **`margin`** (top1−top2 for a choice; |p −
  `noul_threshold`| for a noul; gap to the nearest level for a score) and the derived
  boolean **`near_tie`** (`margin <= near_tie_margin`). Reason: the two backends were
  measured to differ by **Δp = 0.0006**, which can flip a 0.5001-vs-0.4999 decision, so a
  near tie must be visible rather than decided silently.
- F3b. With `escalate_on_near_tie` set, a near tie **cannot** report the `high` band, so an
  existing `route on = "$.payload.band"` escalates it. The flag defaults **false** so the
  behaviour change is opt-in and appears in a config diff, while `near_tie` is reported
  either way.
- F4. The payload serializes with `label` and `band` as **JSON members**, so
  `route on = "$.payload.label"` resolves from the JSON.
- F5. For a `noul`, `p` is carried; for a `score`, `score` is carried.
- F6. `saturated()` is true only for a real multi-option distribution where one
  option exceeds `saturation_threshold`; never for a `noul`.
- F7. When `escalate_on_saturation` is set, a saturated answer is demoted so its
  band can never be `high` — a saturated answer must not auto-allow.
- F8. `band_enum()` round-trips the string band to the typed `Band`.
- F9. **`decide` RETURNS this payload** (added 2026-09-30). Until then `DecisionPayload` was
  built and unit-tested but `decide` returned raw `answers`, so a graph author following
  the documented `route on = "$.payload.label"` had nothing to match on. The MCP response
  is `{"payloads": {<question_id>: <payload>}, "answers": {...}}`.

### Non-Functional
- NF1. No I/O; the payload is built from an `Answer` and a `Policy`.
- NF2. Empty optional fields are omitted from the JSON (a compact wire form).
- NF3. Zero clippy warnings, zero fmt diffs.

## Architecture

```
Answer { value, confidence, probabilities, options, ... }
   +  Policy { thresholds }
        │
        ▼  DecisionPayload::from_answer
{ "label": "...", "band": "low|mid|high", "confidence": 0.93,
  "probabilities": [...], "options": [...], "saturated": false }
        │
        ▼  route on = "$.payload.label"   (member access + exact equality ONLY)
```

**Why this shape and not a computed band in the graph:** pii-graph's expr language
is *"only object member access … no arrays, no functions"* (`expr.rs:3-8`);
`transform` emits a member value or a constant (`nodes.rs:43-62`); `route`'s `eq` is
exact `serde_json::Value` equality (`nodes.rs:113`). A computed band is therefore
**not expressible**, and this payload is the workaround that keeps thresholds in
`Policy` TOML while giving the graph an exact string to match.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `payload_exposes_the_two_route_keys` | answer(label=bug_in_code, 0.95) | `label=="bug_in_code"`, `band=="high"` |
| `label_is_always_one_of_the_declared_options` | 3 different answers | every `label` ∈ declared options |
| `a_low_confidence_routes_to_the_low_band` | confidence 0.35 | `band=="low"` |
| `a_saturated_answer_is_flagged_but_keeps_its_band_by_default` | `[1,0,0]`, default policy | `saturated==true`, `band=="high"`, confidence unchanged |
| `saturation_escalation_demotes_the_band_and_never_allows` | `[1,0,0]`, escalation on | `band=="mid"`, never `high` |
| `the_near_miss_control_stays_high` (CONTROL) | `[0.98,0.011,0.009]`, escalation on | `saturated==false`, `band=="high"` |
| `noul_and_score_fields_carried_through` | noul with p; score with score | `p` / `score` present |
| `payload_round_trips_and_omits_empty_fields` | any payload | equal after round-trip; `label`/`band` present in the JSON |

## Exit Criteria
- [ ] `cd /projects/laya-rs && cargo test -p laya-core --lib` — all pass
- [ ] `cd /projects/laya-rs && cargo clippy -p laya-core --all-targets -- -D warnings` — zero
- [ ] `cd /projects/laya-rs && python3 -c "import sys;sys.exit(0 if 'label' in open('crates/laya-core/src/route_payload.rs').read() else 1)"`

## Guardrails
- Do NOT compute `band` in a graph `transform` node — **impossible** in the expr
  language (B5), and an attempt will look like it works while never matching.
- Do NOT let a saturated answer reach the `high` band when escalation is on.
- Do NOT rewrite the reported `confidence` when saturating — the flag is the
  signal, the demotion is opt-in.
- Do NOT add arithmetic, comparison or range matching to this contract; the graph
  language cannot express it, so the payload must arrive ready to match.
- Do NOT expose `act_probability` (inert on this checkpoint; see the ledger doc).
