# Spec: 03 — The Rust head port (P4)
#: depends=- status=done block=-

## Overview
Replace the Python decision path with a pure-Rust implementation of Laya's
26.5M-parameter head, so the whole decision layer is one binary with **no Python
and no torch** — which also removes the `tokenizers`/`transformers` version
conflict that already bit us once.

**Status: COMPLETE (2026-09-30).** Every exit criterion passes, including the live corpus, which
measured **label agreement 16/16 = 1.0000** against the Python reference over a 16-case corpus
driven from text on both sides. The component-wise results, each against its own oracle:

* the **safetensors reader** against the real 36-tensor file (36 tensors, every shape);
* the **math primitives** with per-branch MEASURED accuracy (`erf` was wrong three times);
* the **forward pass**, reproduced against **PyTorch itself** — a differential fixture generated
  by running the real `nn.TransformerEncoderLayer`, matching to a **worst logit difference of
  4.05e-6** against a margin scale of 1e-3;
* the **tokenizer and `build_sequence`** — reproduced **exactly**: 91 corpus strings / 2207 ids and
  707 sequence cases / 83411 ids / 2705 markers, no tolerance, because tokenization is discrete and
  any mismatch is a real bug;
* the **end-to-end path** (`Head::run` + `laya decide`), text in and a decision out, which is what
  the live criterion drives.

**What the port learned that this spec did not say**, and which is now recorded because it is the
kind of thing that is silently wrong rather than loudly wrong:

* **The encoder block's activation is `ReLU`, not `GELU`.** `nn.TransformerEncoderLayer` is
  constructed without an `activation` argument and torch's default is `F.relu`, while `scorer`
  and `act_head` carry their own explicit `nn.GELU()`. Two activations in one model.
* **`norm_first=True`** — both `LayerNorm`s precede their sublayer.
* The fused `in_proj` row is `[q_0..q_d, k..., v...]`, so chunking by `head_dim` yields
  `3*n_heads` pieces and head `h` of `k` sits at `n_heads + h`.
* `type_emb` is added **before** the blocks; the attention is bidirectional (no causal mask);
  and the act features come from the **detached** softmax.

**Complete.** The two bugs the live run found — the nested `/embedding` shape and an O(seq^3)
attention from a linear scan inside the inner loops — are recorded in the exit criterion below,
because they are the kind that only a live run surfaces.

> ### MEASURED FACTS THAT CORRECT THIS SPEC (2026-09-30)
>
> Three details below were wrong, found by reading the pinned checkpoint and the reference
> implementation rather than the design notes:
>
> 1. **`act_head` IS in the checkpoint** — 4 tensors, `[256,1028]`, `[256]`, `[2,256]`, `[2]`.
>    The F1/guardrail worry that it might be absent was unfounded: it is present, so the port
>    must read it. (`[256,1028]` = `d + 4` pooled features, confirming the reference.)
> 2. **`temperature` is ALREADY CLAMPED inside the checkpoint.** The stored array is
>    `[1.6369, 1.2514, 1.9834]` — all in range — and the only out-of-range value is
>    `temperature_by_options["choice:11+"] = 0.10058...`. The clamping happens in the *client*
>    (`laya.common.clamp_temperature`), at load. So the port must clamp on read, exactly as F3
>    says, and the reason is a single bucket rather than a whole table.
> 3. **`scorer` is `LayerNorm → Linear → GELU → Linear`**, confirmed by 6 tensors
>    (`0.weight/bias [1024]`, `1.weight [1024,1024]/bias`, `3.weight [1,1024]/bias`). Index 2 is
>    the GELU, which holds no weights — which is why an inventory that counted tensors would see
>    a gap at 2 and could easily misnumber the rest.
>
> **And the thing this spec did NOT say, which decides the architecture:** the reference applies
> temperature and then takes `argmax` (`agent.py:329-341`) — the LABEL does not depend on
> temperature at all. Only the reported probabilities do. That is an edge invariant worth
> preserving, and it is now F10.

**Project**: laya-rs
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

  > **CORRECTION (2026-09-30, measured).** This requirement conflicts with what was built, and the
  > disagreement is kept rather than papered over. The implemented
  > `DecisionPayload::margin_of` in `laya-core` computes the margin **in probability space**, and
  > that is the value the near-tie band acts on. Two quantities named `margin` — one logit, one
  > probability — is exactly how a near-tie rule ends up reading the wrong one, so the two are now
  > named apart:
  >
  > * **`Decision::logit_gap`** (in `laya-head`) is this requirement: the top1 − top2 gap over the
  >   RAW LOGITS, pinned by `the_logit_gap_is_the_raw_top1_minus_top2` against the reference's own
  >   `raw_logit_gap`.
  > * **`DecisionPayload::margin`** (in `laya-core`) stays a probability-space distance, per
  >   question type (`noul` measures distance from the threshold, `score` the gap to the nearest
  >   adjacent level, and a single-option `choice` its whole mass).
  >
  > So F8 is satisfied in substance and renamed in form. The reason the probability form is the one
  > the band uses: it is the quantity a caller can reason about against a threshold, while a logit
  > gap's meaningful scale depends on the temperature the bucket applies — which varies by bucket.
- F9. Carry provenance: the head's sha256 and the backbone's sha256, so a
  mismatched artifact fails loudly instead of silently deciding differently.
- F10. **Temperature must NOT move the label.** The reference applies temperature and *then*
  takes `argmax` (`agent.py:329-341`), so the returned label is temperature-invariant and the
  temperature affects only the reported probabilities. A port that softmaxed with temperature
  before the argmax would make the label depend on a config value — and since that value is a
  0.1006 bucket that the clamp is silently rescuing, the label would move when someone "fixed"
  the config. Pinned by test: the argmax is computed on the **raw logits**.
- F11. The forward pass is a **pure function** of its inputs (`&self`, no interior mutability),
  so it can be shared across threads without a lock. See the Threading section.
- F12. Concurrency, if added, lives in a pool in FRONT of the head (`pool` feature, off by
  default) — never inside it, and never with shared mutable state.

### Non-Functional
- NF1. **No `unsafe`, no C bindings** — pure Rust, matching the workspace lints.
- NF2. The backbone stays in llama.cpp (`/embedding`); the head runs in-process.
- NF3. Deterministic **within a pinned build**: the same inputs give the same
  output. This is the honest form of the determinism claim (blocker B1) — the
  original "fully deterministic/replayable" was overstated across implementations.
- NF4. Cold-load time is **measured and recorded**; it decides whether the hook
  lane can spawn per call or needs a persistent server (review M2).
- NF5. No new dependency for the port beyond what `redb` already pulled in: reading
  safetensors is a documented header + a `mmap`-able byte range, and the math is
  matmul/LayerNorm/GELU/softmax. If a dependency is genuinely needed, it is
  recorded with the reason rather than added quietly.

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

## Threading: does the head need to be single-threaded? (answered 2026-09-30)

The question was asked directly, so it is answered here rather than left to whoever writes the
code. **The short answer: single-threaded is CORRECT for correctness and WRONG for latency, and
both halves are cheap to satisfy.**

### First, the measured size of the problem

The head's forward pass is **4.98 G-MAC ≈ 9.97 GFLOP** per call at the real shape
(`d_model 1024`, `n_head 16`, `n = head_max_len 192`, 2 layers, `d_ff 4096`, read from the real
tensor shapes, not from the config).

> **CORRECTION (2026-09-30).** This section previously read **"7.55 G-MAC ≈ 15.1 GFLOP"**. That
> figure is **1.5× too high**: 7.55 G-MAC is what the formula yields at `n = 300`, and it was
> carried over from a scratch estimate instead of being recomputed from the checkpoint's actual
> shapes. The superseded wording is kept above so the disagreement is visible rather than erased.
> The lesson is the class, not the arithmetic: a constant that is not recomputed from the live
> artefact is a **claim**, and this one had units and looked derived, which is why it survived a
> commit. Full working in `_docs/06-research/03-four-architecture-questions.md` §0.
(`L=192`, `d=1024`, `ffn=4096`, 2 layers — `L = head_max_len` is the *maximum*, and the sequence
is only that long when it is full). On one core at ~20 GFLOPS that is **~755 ms**; across 8 cores
at ~80 GFLOPS, **~189 ms**. Current measured end-to-end latency through the Python sidecar is
173–436 ms, so a naive single-threaded Rust port would be **no faster** — it would only remove
torch and the Python dependency.

### Why single-threaded is right for correctness

`laya-head` is declared `#![forbid(unsafe_code)]`, and the workspace forbids data races by
construction rather than by review. A parallel matmul means either `unsafe` or a dependency, and
neither is worth it for a first correct port.

There is a second, subtler reason: **determinism.** NF3 promises the same inputs give the same
outputs. Parallel reduction changes floating-point summation ORDER, and floating-point addition is
not associative — so a threaded matmul can return a bit-different result run to run. A port whose
whole purpose is to be checkable against a reference must not do that.

### Why single-threaded is wrong for latency

The measured 173–436 ms for the current path is dominated by torch, and 189 ms of it would remain
as pure arithmetic. **The port's win is not speed — it is removing Python, torch, the
`tokenizers`/`transformers` conflict, and the serial socket.** That should be said plainly, because
"port it to Rust and it will be fast" is the assumption that leads to disappointment.

### The design that satisfies both

```
  ONE process, N single-threaded workers, NO shared mutable state
  ┌────────────────────────────────────────────────────────┐
  │  the head is a pure function: Tensors × [seq,1024] → Answer │
  │  no interior mutability, no globals, no lock            │
  └────────────────────────────────────────────────────────┘
        ▲                                    ▲
        │ worker 1 (its own copy view)       │ worker 2
        └──────────────┬─────────────────────┘
                 a small thread pool
        (laya-backbone gains a `pool` feature; the head itself
         remains a pure `&self` function, unchanged)
```

Rules:

- **F11.** The forward pass is a **pure function** of its inputs. No `&mut self`, no interior
  mutability, no thread-local state. Weights are immutable after load and shared by reference.
- **F12.** Concurrency, when it is added, lives in a **pool in front of** the head — never inside
  it. Each call gets its own buffers. This is what makes the model safe to share without `Arc<Mutex<_>>`.
- **F13.** Determinism is preserved because each worker's reduction order is fixed: parallelising
  *across calls* does not change any single call's arithmetic.
- **F14.** The pool is a **feature, not the default** (`pool`, off by default), so the baseline
  build has no threads at all and the correctness tests never run under a scheduler.
- **NF5.** Cold load is measured and recorded (NF4 already requires it) because it is what decides
  whether the hook lane can spawn per call. The head is 106 MB of f32; if the load is ~1 s, a
  per-call spawn stays untenable and the persistent-server design is confirmed rather than assumed.

### What is explicitly NOT decided here

Whether the *tokenizer* is single-threaded is a separate question with a separate answer (it is
fast and sequential by nature). The backbone already parallelises at 5.8× — that is llama.cpp's
business, not ours, and this spec does not touch it.

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
| `the_label_does_not_depend_on_temperature` | the same logits, two in-range temperatures | the SAME argmax, different probabilities |
| `the_forward_pass_takes_no_mutable_state` | compile-time | `&self`, so it is shareable without a lock |
| `the_stored_temperature_array_is_already_in_range` | the real checkpoint | `[1.6369, 1.2514, 1.9834]`, all inside `[0.5, 5.0]` |
| `only_the_choice_11_plus_bucket_is_out_of_range` | the real config | exactly one clamped bucket, and it is flagged |

## Exit Criteria
- [x] `cargo test -p laya-head` — all pass, offline
- [x] `cargo clippy -p laya-head --all-targets -- -D warnings` — zero
- [x] `cargo fmt --all --check` — zero diffs
- [x] `python3 scripts/live/live-corpus-parity.py` — **MEASURED 2026-09-30:
  label agreement 16/16 = 1.0000** (floor 0.9), 0 errors, worst probability difference 4.978e-05
  against the reference's own 4-dp rounding resolution of 5e-05.

  The corpus spreads across both axes that could hide a difference: every question type, every
  temperature bucket boundary (2, 3-5, 6-10, 11+), unicode, a single-option choice, options in
  non-alphabetical order, and a **460-token state** that crosses the head's ~192-token window and
  approaches the backbone's 512. The logit-gap histogram is reported with it, and 1 of 16 cases sits
  at a gap below 0.1 — so the corpus is not uniformly easy, which is what would let a 100% rate mean
  nothing.

  **Both sides are driven from TEXT** (the reference over its socket, the Rust path through
  `laya decide`), which is the point: a comparison starting from a pre-computed hidden state would
  skip the tokenizer, the sequence builder and the embedding call — the parts most likely to differ.

  The criterion is gated in `scripts/verify-all.sh` as step 7b: when the backbone or the reference
  sidecar is down it prints **UNMEASURED** and says why, rather than either failing the build or
  quietly passing.

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
