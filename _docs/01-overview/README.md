# laya-rs — what it is

**laya-rs** is a Rust **System-1 decision layer**: Laya over MCP stdio, feeding
deterministic DAG `route` nodes.

## The idea in four sentences

Laya is a small calibrated decision model — a 421M `ModernBERT` backbone plus a
26.5M parameter head. Unlike an LLM it generates no text: you send a **state** and
**typed questions**, and get **typed answers with probabilities** in one parallel
pass. `laya-rs` exposes it as **one Rust binary speaking MCP over stdio**, so
Hermes, pi, Claude Code, our skills and our hooks all consume the same tool surface
with **no network listener**. It supplies the **value** an existing `route` node
matches on, so **the graph owns every edge** and the model can only *select among
ports that already exist* — never invent one.

## Why it exists

We had already concluded, twice and independently (research docs 039 and 040), that
a probabilistic model must only *select among existing ports*, and that the graph
should stay the authority. A published ten-level treatment of System-1 decision models is a third independent
arrival at the same architecture — with a **paid cloud model and an OpenRouter key**
at its centre.

Laya is the open-source equivalent (Apache-2.0): same three question types, same
calibrated-probability contract, but **R$0 per call, no egress, no GPU** — we
already measured it serving on our own llama.cpp, with parity Δp = 0.0006 against
pure PyTorch.

So the project is: **their ten levels, our free model, our graph engine.**

## The three properties that make it safe

**1. The model cannot invent an edge.** Its answer is always one of the caller's
declared options. A graph validates at author time (every edge's port must be
declared) and errors loudly at run time if nothing matches — so with a catch-all
rule, an undefined edge is impossible.

**2. Thresholds live in the caller.** Every number a decision depends on is a
`Policy` field. The model never sees a weight, a band boundary or a token count.
Tuning is a one-line diff, never a prompt edit.

**3. An unavailable gate never means permission.** If Laya is not running, the
server **refuses to start** with an actionable diagnostic; while running, a lost
Laya makes `gate` answer `ask` — never `allow`.

## What is implemented today (2026-09-29)

| crate | state |
|---|---|
| `laya-abi` | **done** — the question/answer contract |
| `laya-core` | **done** — `Policy` (per-type bands, composite scoring), `DecisionPayload`, `readiness` |
| `laya-head` | **started** — `clamp_temperature` landed and tested; the forward pass is not |
| `xtask` | **done** — `cargo xtask gate` |
| `laya-backbone` | placeholder — the HTTP probe/client is not written |
| `laya-store` | placeholder — the ledger is specified, not built |
| `laya-mcp`, `laya-cli` | placeholders — both refuse to start rather than fake success |

**57 tests pass; clippy zero; fmt clean; `cargo xtask gate` PASS.**

## Read next, in this order

1. [`../04-development/DESIGN-AMENDMENTS-2026-09-29.md`](../04-development/DESIGN-AMENDMENTS-2026-09-29.md)
   — the corrections from the adversarial review. **These win** over the design.
2. [`../04-development/TEN-LEVELS-MAPPING.md`](../04-development/TEN-LEVELS-MAPPING.md)
   — the ten levels, mapped, with the failures we inherit as requirements.
3. [`../06-research/01-design.md`](../06-research/01-design.md) — the original design.
4. [`../05-operations/DECISION-LEDGER.md`](../05-operations/DECISION-LEDGER.md) —
   what every decision records, and why the store has one writer.
