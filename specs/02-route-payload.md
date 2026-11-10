# Spec: 02 — The route-shaped decision payload
#: depends=- status=done block=-

## Overview
`laya-rs` must hand a DAG's `route` node a payload whose `label` and `band` are
matchable with plain member access and exact string equality, because that is all
a graph `route` node's expression language supports.

**Status: IMPLEMENTED.** `laya-core::route_payload` (tests in `specs/01`, green).
This spec records the wire contract so a change is made against a written standard.

**Project**: laya-rs
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

**Why this shape and not a computed band in the graph:** a graph `route` node's expression
language is **only object member access** (`$.a.b`) — no arrays, no functions, no arithmetic; a
`transform` emits a member value or a constant; and `route`'s `eq` is **exact
`serde_json::Value` equality**. A computed band is therefore
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

## `calibrate` — whether the confidence is worth gating on (added 2026-09-30)

Spec 04's ledger records `(confidence, correct)` pairs. This is the feature that consumes them.

**Where it lives:** the pure function is in `laya-store` (it is arithmetic over two vectors, with
no I/O and no model); the CLI verb is `laya ledger calibrate`. It is NOT an MCP tool, because it
reads the whole ledger and a per-call tool that scans the store is a different kind of operation
from every other tool on that surface.

### Functional

- **C1.** `calibrate` computes **Expected Calibration Error** as `laya.common.ece_score` defines
  it: 15 equal-width bins over `(0, 1]` (the first edge is exclusive, so a confidence of exactly
  0.0 falls in NO bin), each bin contributing `bin_mass × |mean_confidence − accuracy|`, where
  `bin_mass` is that bin's share of all samples. The reference implementation is the oracle and it
  is pinned by test against values produced by running it:

  | conf | correct | ECE |
  |---|---|---|
  | `[1,1,1,1]` | `[1,1,1,1]` | `0.0` |
  | `[1,1,1,1]` | `[0,0,0,0]` | `1.0` |
  | `[0.9,0.8,0.7,0.95,0.6]` | `[1,1,1,0,1]` | `0.39` |
  | `[0.1,0.2,0.9]` | `[0,1,1]` | `0.3333333333333333` |
  | `[]` | `[]` | `nan` |

- **C2.** An **empty** input returns `nan`, not 0. This is the same rule as an empty percentile:
  zero would read as "perfectly calibrated", which is a measurement nobody made.
- **C3.** The join is by decision identity: an outcome names a decision, and only decisions that
  **have** an outcome enter the calculation. Decisions without outcomes are counted separately.
- **C4.** It reports, and does not judge: `ece`, the sample count, the number of decisions with no
  outcome, the per-bin table, and a **plain statement when the sample is too small to conclude
  anything**. What counts as too small is recorded (see C5) rather than left to vibes.
- **C5.** `MIN_SAMPLES = 30`. Below it, `calibrate` still computes and reports the ECE but marks
  it `conclusive: false`. 30 is a convention, stated as a convention — with 15 bins it means an
  average of 2 samples per bin, which is weak, and the point of saying so is that a number with
  `conclusive: false` next to it cannot be quoted as if it were one.
- **C6.** It reports the **saturation rate** alongside. A saturation rate near 1.0 means the
  model's confidence carries almost no information, which is the single most useful thing to know
  before trusting any threshold on it.

### Non-Functional

- **NC1.** No new dependency. The arithmetic is sums and comparisons.
- **NC2.** Deterministic: the same records give the same ECE, to the last bit.
- **NC3.** It never writes. `calibrate` is a reader, and the store stays append-only.

### TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `ece_matches_the_reference_on_perfect_input` | the 5 oracle vectors above | exact equality, including `nan` |
| `ece_of_an_empty_input_is_nan_not_zero` | `[]` | `nan`, with the reason |
| `ece_uses_(0,1]_bins_so_confidence_zero_is_excluded` | `[0.0]` / `[1]` | excluded from every bin |
| `only_decisions_with_outcomes_enter_the_calculation` | 3 decisions, 1 outcome | `1` sample |
| `a_small_sample_is_marked_inconclusive` | `MIN_SAMPLES − 1` | `conclusive: false` |
| `at_min_samples_it_is_conclusive` | `MIN_SAMPLES` | `conclusive: true` (the boundary) |
| `the_saturation_rate_is_reported_alongside` | a saturated decision | a rate of `1.0` |

### Guardrails

- Do NOT compute an ECE and present it without the sample size. An ECE over 3 decisions is noise.
- Do NOT treat `nan` as an error to be swallowed: it is the correct answer for no data.
- Do NOT let `calibrate` write an outcome. Recording what was right is a human/agent act, and a
  calibration that manufactured its own ground truth would be measuring itself.

## Exit Criteria
- [x] `cargo test -p laya-core --lib` — all pass
- [x] `cargo clippy -p laya-core --all-targets -- -D warnings` — zero
- [x] `python3 -c "import sys;sys.exit(0 if 'label' in open('crates/laya-core/src/route_payload.rs').read() else 1)"`

## Guardrails
- Do NOT compute `band` in a graph `transform` node — **impossible** in the expr
  language (B5), and an attempt will look like it works while never matching.
- Do NOT let a saturated answer reach the `high` band when escalation is on.
- Do NOT rewrite the reported `confidence` when saturating — the flag is the
  signal, the demotion is opt-in.
- Do NOT add arithmetic, comparison or range matching to this contract; the graph
  language cannot express it, so the payload must arrive ready to match.
- Do NOT expose `act_probability` (inert on this checkpoint; see the ledger doc).
