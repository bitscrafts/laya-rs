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
crates/laya-graph      the generic DAG executor: flow, lowering, scheduler, executors
crates/laya-store      redb: the append-only decision ledger
crates/laya-mcp        the MCP stdio server  -> binary `laya-mcp`
crates/laya-cli        the terminal runner   -> binary `laya` (`doctor|decide|ledger|graph`)
xtask                  recurring tasks (`cargo xtask gate`)
```

`laya-graph` is deliberately **not** a dependency of `laya-mcp`: the judge does not depend on the
executor, and `route` stays out of the tool list. It depends on neither `laya-store` nor `redb` —
run records are pure data in `RunReport`, and `DecisionRow` already carries the `run_id`/`node_id`
join keys for the day a consumer persists a run. See [`specs/07-graph-executor.md`](specs/07-graph-executor.md).

## The graph executor

`laya graph validate|render|run <flow.json>` runs a flow end to end: a `decide` node's answer is
lowered to `mcp` + `route` nodes, the `route` node selects an edge by exact `label` match (with a
**mandatory catch-all**), and the selected branch runs. Independent nodes fan out concurrently up
to a declared limit; `writes: true` nodes are mutually exclusive; a failure stops the run and
reports `stopped_early` with the failing node id; a stopped run **exits non-zero**.

The acceptance test drives the identical path and spawns the real MCP server — it never stubs the
caller, because a stub would make it a re-enactment rather than a test. It runs as a gate step.

## The seven MCP tools

`decide` · `gate` · `ask_file` · `ask_files` · `should_compact` · `ask` · `ledger`

All seven are implemented. `route` is deliberately **not** a tool: exposing it would let an
agent select edges outside any graph, which is the injection failure mode the design exists to
prevent.

## The sidecar, and what it costs

The decision HEAD needs torch, so it runs as a **pinned, persistent Python sidecar** over a unix
socket — a per-call interpreter spawn measured 23–24 s, which is untenable. Two consequences
worth knowing before you rely on it:

* it is **strictly serial** (one connection, a blocking loop); the backbone is 5.8× parallel,
  the head is not. `ask_files` therefore asks sequentially and reports `parallel: false`;
* the head reads a **PREFIX** of the state — a signal at the start scored identically at 3,975
  and 199,951 bytes, and the same signal at the END was gone by 3,976. Both file tools return
  `bytes` and attach a `warning` above 1 KiB, because a verdict silently covering the first page
  of a 200 KiB file would be worse than a refusal.

Spec 03 (P4) removes both limits by porting the head to Rust.

## Ledger

```
LAYA_LEDGER_DIR   where a server appends its own segment file (unset = no ledgering)
laya ledger compact   merge segments into ledger.redb — THE ONLY WRITER
laya ledger stats     counts, latency percentiles, saturation rate
laya ledger tail [N]  the most recent records
laya ledger outcome   record whether a decision was right
```

`laya-mcp` appends to `ledger-<pid>.jsonl` and **never opens the store**: `append_segment`
takes a directory, so the single-writer collision cannot happen. Nothing reaches `ledger.redb`
until `compact` runs, and compaction **moves** segments into `ingested/` rather than deleting
them, so it is idempotent and the raw evidence survives.

## Gates

```bash
cargo test  --workspace          # unit contracts, offline, zero model calls
cargo clippy --workspace --all-targets   # zero warnings
cargo fmt    --workspace --check         # zero diffs
cargo xtask gate                 # all of the above
```

Offline tests must never need the model. Anything that needs Laya to judge
meaning is a *live* test and is kept separate.
