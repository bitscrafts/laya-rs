# Spec: 03 — The Rust head port (P4)

## Overview
Replace the Python decision path with a pure-Rust implementation of Laya's
26.5M-parameter head, so the whole decision layer is one binary with **no Python
and no torch** — which also removes the `tokenizers`/`transformers` version
conflict that already bit us once.

**Status: NOT STARTED.** Blocked on the corrections below, all of which the
adversarial review showed must land *before* the port. This spec is written so the
port is made against the corrected contract rather than the design's original one.

**Project**: /projects/laya-rs
**Phase**: P4

## Requirements

### Functional
- F1. Load the head from `laya-head.safetensors`, reading ALL 36 tensors —
  **including the four `act_head` tensors the original design's inventory omitted**
  (`act_head.0.weight [256,1028]`, `act_head.0.bias [256]`,
  `act_head.2.weight [2,256]`, `act_head.2.bias [2]`).
- F2. Read the config from the safetensors metadata (`laya.config`):
  `head_max_len`, the temperature table, the encoder name.
- F3. **Apply `clamp_temperature` verbatim**: clamp to `[0.5, 5.0]`, NaN → 1.0.
  This is blocker **B2** — the raw config value `choice:11+ = 0.1006` multiplies
  logits ~10× and would publish "a 0.24 top probability as 0.99", defeating the
  confidence gate the whole design depends on.
- F4. Flag a clamped bucket in the answer, and mark confidence from such a bucket
  as **uncalibrated**.
- F5. `nhead = max(1, d / 64)` is **computed, not read** — it is not in the config
  (review M3), so it must be derived from the hidden size.
- F6. Reproduce the forward pass exactly, including what the original spec omitted:
  `src_key_padding_mask`, the marker gather + clamp, the `-1e4` `masked_fill`,
  the `type_emb` broadcast add, `norm_first=True` (pre-LN), the LayerNorm eps, the
  GELU variant, and the `scorer` sequence — which begins with a **LayerNorm**, not
  a Linear (the original spec's `scorer.0 [1024,1024]` was wrong).
- F7. Reproduce `build_sequence`'s constants: the per-option 48-token cap, the
  16/8-token floors, the `"<type> question: "` prefix, mask-token stripping, and
  right-side state truncation.
- F8. Produce the same `Answer` shape as the Python path: `value`, `probabilities`,
  `confidence`, and — per blocker B1 — the **`margin`** (top1 − top2 logit gap).
- F9. Carry provenance: the head's sha256 and the backbone's sha256, so a
  mismatched artifact fails loudly instead of silently deciding differently.

### Non-Functional
- NF1. **No `unsafe`, no C bindings** — pure Rust, matching the workspace lints.
- NF2. The backbone stays in llama.cpp (`/embedding`); the head runs in-process.
- NF3. Deterministic **within a pinned build**: the same inputs give the same
  output. This is the honest form of the determinism claim (blocker B1) — the
  original "fully deterministic/replayable" was overstated across implementations.
- NF4. Cold-load time is **measured and recorded**; it decides whether the hook
  lane can spawn per call or needs a persistent server (review M2).

## Architecture

```
text ──► tokenizer (Rust) ──► build_sequence ──► llama.cpp /embedding
                                                       │ hidden [seq, 1024]
                                                       ▼
                                            head (in-process, Rust)
                                  type_emb add → pad → 2 × (LayerNorm →
                                  self_attn → LayerNorm → GELU MLP) → gather
                                  → scorer(LayerNorm → Linear → act → Linear)
                                  → mask → softmax(clamped temperature)
                                                       │
                                                       ▼
                          Answer { value, probabilities, confidence, margin }
```

## TDD Contract

The oracle is **decision-level, not Δp on one value** (review M4). It runs the
Rust binary **from text** — its own tokenizer, its own `build_sequence` — against a
pinned Python reference over a corpus.

| Test Name | Input | Expected Output |
|---|---|---|
| `clamp_temperature_clamps_low` | 0.1006 | `0.5` |
| `clamp_temperature_clamps_high` | 9.9 | `5.0` |
| `clamp_temperature_passes_through` | 1.906 | `1.906` |
| `clamp_temperature_nan_is_one` | NaN | `1.0` |
| `clamped_bucket_is_flagged` | a config with an out-of-range entry | answer flags `bucket_clamped` |
| `nhead_is_derived_from_hidden_size` | d = 1024 | `nhead == 16` |
| `all_36_tensors_are_loaded` | the real safetensors | 36 tensors, 0 unmatched |
| `act_head_tensors_are_present` | the real safetensors | the 4 `act_head` tensors load |
| `head_max_len_and_temperatures_read_from_metadata` | the real safetensors | config matches the file |
| `margin_is_top1_minus_top2` | a known logit vector | the gap, not the probability |
| `label_agreement_over_a_corpus` (LIVE) | N states, Rust vs Python | agreement ≥ the agreed floor |
| `margin_histogram_is_reported` (LIVE) | the same corpus | a histogram, so near-ties are visible |
| `a_near_tie_escalates` | a state with margin < `margin_floor` | routes to escalate, not a confident action |

## Exit Criteria
- [ ] `cd /projects/laya-rs && cargo test -p laya-head` — all pass, offline
- [ ] `cd /projects/laya-rs && cargo clippy -p laya-head --all-targets -- -D warnings` — zero
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` — zero diffs
- [ ] the live corpus suite reports label agreement and its floor is met

## Guardrails
- Do NOT apply a raw config temperature. Clamp first — B2 is exactly this bug, and
  it silently defeats the confidence gate.
- Do NOT drop `act_head` without an explicit decision, recorded: the original
  inventory implied dropping it, which would remove `act_probability` from the tool
  contract and diverge from Laya's documented response (review M3/F8).
- Do NOT claim "replayable re-execution". The journal records `input_hash`, not the
  input (review F15) — the honest word is **auditable**.
- Do NOT compare against `Δp ≤ 0.001` on a single probability and call the port
  certified (review M4). That test passes while labels flip on near-ties.
- Do NOT ship without provenance hashes; a swapped backbone must not decide
  silently.
