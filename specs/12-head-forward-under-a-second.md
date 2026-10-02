# laya-rs — spec-12: the head's forward pass under one second at seq 460, with no change in decisions

- **Project**: `/projects/laya-rs`
- **Priority**: P2, but it unlocks the biggest missed opportunity (`REPORT.md` §6): serving decisions from the proven Rust head in-process, which removes the unsupervised Python sidecar from the hot path.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`REPORT.md` §1.4); measurements `notes/bench-head.txt` (scratch crate `bench/`, outside the repo)
- **Depends-On**: none.

---

## Verified Premises

- **Measured, release build, real checkpoint `laya-head.safetensors`, 32-core host** (`notes/bench-head.txt`):
  ```
  cold load (open+load): 0.149 s
  forward seq=  32: 0.698 s
  forward seq= 128: 2.824 s
  forward seq= 460: 10.641 s
  ```
  Linear in seq, about 23 ms/token. The live sidecar answers a whole decision in 173–436 ms (HANDOFF). The Rust head is roughly 25–50× slower than the path it was built to replace.
- `crates/laya-head/src/math.rs:207-218`: `matvec` is one row × one vector with a **single serial accumulator**, `row.iter().zip(x).map(|(&a,&b)| a*b).sum()`. The comment says *"the reduction order is part of the contract"*. LLVM cannot vectorise a strict-order f32 reduction, so this runs at scalar latency-bound speed.
- `crates/laya-head/src/forward.rs:393-414, 438-449`: every projection is `linear()` called **per token**. Each weight matrix (up to 4096×1024 f32 = 16 MiB) is streamed from memory once per token, with no reuse across the sequence.
- **Measured micro-benchmark** on the FFN shape [4096×1024] × 460 tokens (`notes/bench-head.txt`): per-row `matvec` **1.641 s**; tiled across 8 tokens with 8 accumulators **0.244 s**, which is **6.7× single-threaded**; max |diff| **9.83e-7**.
- `crates/laya-head/tests/forward_differential.rs:46`: `LOGIT_TOL: f32 = 1e-4`, justified against the 1e-3 near-tie margin. The worst measured logit difference today is 4.05e-6 (HANDOFF). The **reference itself (PyTorch/BLAS) does not sum in index order**, so index order is not a property of the reference. The differential tolerance is the real contract, and the reordering error (~1e-6) sits two orders below it.
- `crates/laya-head/src/forward.rs:456-467` attributes an earlier slowdown to *"`chunks_exact(..).nth(..)` … a LINEAR scan"*. **Measured false**: `notes/nth-measure.txt` shows 1e6 calls of `chunks_exact(3).nth(k)` for random `k ≤ 471040` taking 0.003 s. `ChunksExact::nth` is O(1). The pre-fix code (`git show 4f807d0^:crates/laya-head/src/forward.rs:455-482`) used exactly that, so the recorded root cause of the "four minutes" is not the one stated. Release builds of both versions were not compared in this review.
- No `rayon` in the offline registry. `std::thread::scope` is in std and needs no `unsafe`.
- No benchmark exists (`benches/` absent; no `criterion`, which is also absent offline).

---

## Overview

The head is correct, and differential testing proves it to 4e-6. It is also slow, for two
measured reasons: scalar strict-order reductions, and no weight reuse across tokens. Fix both
behind the existing differential tolerance, add a benchmark that fails when the time regresses,
and correct the misattributed root cause in place.

---

## Requirements

### Functional

- **R1: a batched matmul.** `math::matmul(w, xs, out_dim, in_dim) -> Vec<f32>` over `seq` rows. It
  tiles tokens (T = 8) so each weight row is reused across a tile, and uses 8 independent
  accumulators. `forward` uses it for `in_proj`, `out_proj`, `linear1` and `linear2`.
- **R2: parallel across output rows.** `std::thread::scope` over contiguous chunks of `out_dim`. The
  thread count comes from `std::thread::available_parallelism()` and is capped by a
  `LAYA_HEAD_THREADS` env var. The result is **bit-identical for any thread count**, because each
  output element is computed by exactly one thread with a fixed reduction order.
- **R3: a benchmark that can fail.** `cargo run --release -p laya-head --example forward_bench -- --seq 460 --max-ms <N>`
  loads the real checkpoint, runs `forward` 3 times, prints min/median ms, and exits non-zero if the
  median exceeds `--max-ms`. It exits 2 with a labelled message if the checkpoint is absent. It
  never skips.
- **R4: correct the record.** Annotate the comment at `forward.rs:456-467` (do not erase it): the
  `nth` explanation was measured false, and the real costs are the scalar reductions and the
  missing weight reuse, with this spec's numbers.

### Non-Functional

- **N1: decisions do not move.** `forward_differential`, `decide_differential` and
  `tokenizer_differential` pass with **unchanged** tolerances. The live corpus parity stays
  16/16 with the worst probability difference ≤ 5e-5.
- **N2: determinism across thread counts.** Logits for `LAYA_HEAD_THREADS=1` and `=8` are bitwise
  equal (test).
- **N3: no `unsafe`, no new crate, zero clippy.**
- **N4: target.** Median `forward` at seq 460 ≤ **1000 ms** on the review host (32 cores), from a
  measured 10 641 ms.

---

## Architecture

```
forward ─► block ─► LN (per row) ─► matmul(in_proj, all rows) ─► attention (per head) ─► matmul(out_proj)
                                 └► LN ─► matmul(linear1) ─► relu ─► matmul(linear2)
matmul = for each out-row chunk (thread) × token tile (8) × 8-lane accumulators
```

**Against the recorded reason.** `specs/03-rust-head.md:180-190` rejected a parallel matmul for
two reasons. Both are argued against here explicitly, as the brief requires:

1. *"A parallel matmul means either `unsafe` or a dependency."* This does not hold for
   `std::thread::scope` (Rust 1.63+, safe, std-only). Each thread writes a disjoint
   `&mut [f32]` obtained from `chunks_mut`. There is no `unsafe`, no `Arc<Mutex<_>>` and no crate.
2. *"Parallel reduction changes floating-point summation ORDER … a threaded matmul can return a
   bit-different result run to run."* That is true of a parallel **reduction**, meaning splitting
   one dot product across threads. R2 instead partitions **output rows**: every output element is
   one dot product computed start to finish by one thread, in a fixed order. The result is
   bitwise independent of thread count, and N2 tests exactly that.

The same spec's latency estimate (`03-rust-head.md:172-176`: ~755 ms single-core) assumed
`L = head_max_len = 192`. The head actually receives the whole sequence, up to `max_len` 512
(`Budget::LAYA`, `crates/laya-head/src/end_to_end.rs:155-162`), and the kernel runs at about
3.5 GFLOP/s, not 20. Measured: 2.8 s at seq 128 and 10.6 s at seq 460.

**Key decision: give up index-order reduction, keep the differential tolerance.** The reference
does not sum in index order either, so the "contract" in `math.rs:214` had nothing to match. A
fixed, thread-count-independent order (N2) preserves the property that matters, which is
reproducibility of *our own* output.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | `matmul_equals_matvec_per_row_within_1e_5` | random W [64×48], 13 tokens | max abs diff ≤ 1e-5 vs per-row `matvec` |
| R1 | `matmul_handles_a_tail_shorter_than_the_tile` | seq = 1, 7, 9 | same as per-row |
| R2 | `forward_is_bitwise_identical_across_thread_counts` | real checkpoint absent → synthetic weights; threads 1 vs 8 | `==` on every logit |
| N1 | existing `forward_differential` | unchanged fixture | passes at `LOGIT_TOL = 1e-4` |
| R3 | `forward_bench` example | `--seq 460 --max-ms 1000` | exit 0 after; **exit 1 today** (≈10.6 s) |

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`:**

- [x] `cd /projects/laya-rs && cargo test -p laya-head --lib matmul_equals_matvec_per_row_within_1e_5 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [x] `cd /projects/laya-rs && cargo test -p laya-head --lib forward_is_bitwise_identical_across_thread_counts 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2 / N2
- [x] `cd /projects/laya-rs && cargo run --release -q -p laya-head --example forward_bench -- --seq 460 --max-ms 1000`: R3 / N4 (today: no such example; and a straight port of today's code measures 10 641 ms)

  **Control for the line above**: the bench must be able to fail, or the threshold proves nothing.

- [x] `cd /projects/laya-rs && test -f crates/laya-head/examples/forward_bench.rs && cargo build --release -q -p laya-head --example forward_bench && ! cargo run --release -q -p laya-head --example forward_bench -- --seq 460 --max-ms 1`: the same bench with an impossible budget must exit non-zero. **The `test -f` and build guards are mandatory.** Without them, `!` inverts "no such example" into success. That is exactly how this criterion behaved when first drafted for this review: it PASSED on a tree with no bench.

**Carried re-checks:**

- [x] `cd /projects/laya-rs && cargo test -p laya-head --test forward_differential`: N1
- [x] `cd /projects/laya-rs && cargo test -p laya-head --test decide_differential`: N1
- [x] `cd /projects/laya-rs && python3 scripts/live-corpus-parity.py`: N1, live, 16/16
- [x] `cd /projects/laya-rs && cargo clippy -p laya-head --all-targets -- -D warnings`

---

## Guardrails

- **G1: do NOT loosen `LOGIT_TOL`** or any differential tolerance to make this pass. If it fails, the kernel is wrong.
- **G2: no `unsafe`, no SIMD intrinsics, no BLAS crate.** Autovectorisation from independent accumulators is enough for N4 (measured 6.7× single-threaded).
- **G3: do NOT wire the head into `laya-mcp` in this spec.** That is a separate decision with its own premises (cold load, memory, supervision). This spec only removes the reason it could not be made.

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-head/src/math.rs` | `matmul` (tiled, 8-acc, scoped threads) + tests | R1, R2 |
| `crates/laya-head/src/forward.rs` | use `matmul` for the four projections; annotate the `nth` comment | R1, R4 |
| `crates/laya-head/examples/forward_bench.rs` | new | R3 |
| `HANDOFF.md` | annotate "The attention was O(seq^3)" with the corrected cause | R4 |
