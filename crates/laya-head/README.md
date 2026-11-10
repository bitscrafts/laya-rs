# laya-head

The 26.5M-parameter Laya **decision head in pure Rust**: no Python, no torch. Status: complete
(spec 03), reproduced against the reference implementation.

The head is two transformer layers plus a scorer: attention, `LayerNorm`, `GELU`, matmul and
softmax, nothing exotic. This crate loads the weights from a `.safetensors` file, tokenises the
input, runs the forward pass over the backbone's hidden states, and applies the post-processing
(temperature clamp, margin, near-tie flag).

## Modules

`safetensors` (weights), `tokenizer` (the pure-Rust tokenizer and sequence builder), `forward` (the
head), `math` (kernels; `LAYA_HEAD_THREADS` sets the thread count), `decide` (post-processing),
`end_to_end` (text in, decision out).

## What it needs at run time

The head file (`laya-head.safetensors`), the tokenizer (`tokenizer.json`) and a backbone server that
answers `/embedding`. See the [install guide](../../_docs/02-guides/RUNNING-LAYA.md). The files are
**not** in this repository.

## Test

```bash
cargo test -p laya-head                      # 75 tests listed, 6 of them #[ignore] (live)
cargo test -p laya-head -- --ignored         # the 6 live tests: need the checkpoint on this machine
```

The ignored tests are differential checks against the reference; they read a checkpoint tokenizer
from the local Hugging Face cache and so cannot run on a clean CI runner.

## Depends on

`laya-abi`, `serde_json`, `thiserror`, `fancy-regex`, `unicode-normalization-alignments`.
