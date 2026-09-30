# laya-rs

A Rust **System-1 decision layer**: Laya over MCP stdio, feeding deterministic DAG
`route` nodes.

Laya is a small (421M backbone + 26.5M head) calibrated decision model. Unlike an
LLM it does not generate text — you send a state and typed questions, and get typed
answers with probabilities in one parallel pass. This workspace wraps it so that:

* **one Rust binary speaks MCP over stdio**, so Hermes, pi, Claude Code, skills and
  hooks all consume the *same* tool surface with no network listener;
* every decision lands in a **redb ledger** as a JSON document;
* the decision payload is **route-shaped** (`label`, `band`, `confidence`), so a DAG
  branch is `route on = "$.payload.label"` and the **graph owns every edge** — the
  model can only select among ports that already exist.

Design: [`_docs/06-research/01-design.md`](_docs/06-research/01-design.md).
Corrections (read first): [`_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md`](_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md).
Analysis: [`_docs/06-research/02-analysis-glm-5.3.md`](_docs/06-research/02-analysis-glm-5.3.md).

## Layout

```
crates/laya-abi        the question/answer/payload contract (no I/O)
crates/laya-core       pure logic: policy, bands, composite scoring (no I/O)
crates/laya-backbone   the only crate that talks to the network
crates/laya-head       the 26.5M decision head in pure Rust (no Python, no torch)
crates/laya-store      redb: the append-only decision ledger
crates/laya-mcp        the MCP stdio server  -> binary `laya-rs`
crates/laya-cli        the terminal runner   -> binary `laya`
xtask                  recurring tasks (`cargo xtask gate`)
```

## Gates

```bash
cargo test  --workspace          # unit contracts, offline, zero model calls
cargo clippy --workspace --all-targets   # zero warnings
cargo fmt    --workspace --check         # zero diffs
cargo xtask gate                 # all of the above
```

Offline tests must never need the model. Anything that needs Laya to judge
meaning is a *live* test and is kept separate.
