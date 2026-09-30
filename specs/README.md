# laya-rs — specs

Specs are the **implementation contract**, not documentation. Each one carries:

- **Requirements** (functional + non-functional)
- **Architecture** (the shape being built)
- **TDD Contract** — a table of `test name → input → expected output`. The test is
  written and watched to FAIL before the code exists.
- **Exit Criteria** — checkbox lines containing a runnable command that returns 0.
- **Guardrails** — what must not be done, and *why*, drawn from the adversarial
  review where applicable.

## Index

| spec | phase | status |
|---|---|---|
| [`01-decision-payload.md`](01-decision-payload.md) | P1 | **IMPLEMENTED** — policy, per-type bands, composite scoring, payload |
| [`02-route-payload.md`](02-route-payload.md) | P1 | **IMPLEMENTED** — the route-shaped wire contract |
| [`03-rust-head.md`](03-rust-head.md) | P4 | **COMPLETE** — the 26.5M head in pure Rust, reproduced against PyTorch (4.05e-6), the reference tokenizer (exact), and the reference's post-processing (5.55e-17). Live corpus label agreement **16/16 = 1.0000**. |
| [`04-mcp-server-and-ledger.md`](04-mcp-server-and-ledger.md) | P1 | **COMPLETE (2026-09-30)** — the stdio server (all 7 tools) + the redb ledger |
| [`05-laya-must-be-running.md`](05-laya-must-be-running.md) | P1 | **IMPLEMENTED** — judgement + probe parsers + `laya doctor`, all tested |
| [`06-priority-queue.md`](06-priority-queue.md) | P1 | **IMPLEMENTED** — the bounded priority queue, 16 tests |
| [`07-graph-executor.md`](07-graph-executor.md) | P5 | **SPECIFIED** — the generic DAG executor, split into 4 parts: [1](07-graph-executor-part1.md) the flow+lowering+R15 · [2](07-graph-executor-part2.md) composition+render · [3](07-graph-executor-part3.md) the scheduler · [4](07-graph-executor-part4.md) CLI+acceptance |

**345 tests pass across the workspace, clippy is at zero warnings, fmt is clean, and
`cargo xtask gate` passes all three steps.** Run `scripts/verify-all.sh` for the whole picture,
including the doc-citation check and the live corpus parity check (§6c).

## What is left

**Nothing outstanding in specs 01–06.** (`EXIT CRITERIA: pass=22 fail=0 prose=2`, and no
unchecked box remains anywhere in `specs/` for those six — the runner now also reports
`pending=62`, which is spec 07's criteria, counted and named rather than skipped.) What comes
next is **spec 07**, written and split into four parts — the generic graph executor, absorbing a
DAG runner as our own capability rather than a gap in P4.
Everything else is implemented and proven:

- P1 is complete. The MCP server exposes all seven tools, the redb ledger is written by
  `laya ledger compact` and nothing else, and `scripts/prove-ledger.py` proves the whole
  lifecycle end to end through the real binaries.
- **One thing is deliberately not built, and it is not a gap:** `calibrate`. It is *possible*
  now — the outcome path exists — but it needs outcomes actually recorded before it can mean
  anything. `laya ledger stats` reports no outcomes recorded while there are none, so the gap
  is visible rather than implied.
- **Spec 07 supersedes the older note here that "nothing yet feeds a payload through a real
  `route` node".** That was true when written; spec 07 is the runner, and it lands in this
  repository (`AGENTS.md`, SCOPE).

## Order

P1 (specs 01, 02, 04, **05**) before P4 (spec 03). The pure core is testable offline
with no model, so it lands first and stays green while the harder work proceeds.

**Spec 05 is mandatory and blocks the others** — a decision layer that cannot decide
must not run, so the readiness precondition comes before the server that needs it.

## The rule that matters most here

**RED before GREEN.** A spec's TDD table is not a wish list — each row is a test
that must be seen to fail for the *expected reason* before the implementation
starts. A test that passes the first time it runs is testing nothing.

And **controls are mandatory**: every rule that skips, flags or suppresses
something needs a near-miss case that must take the normal path. Examples in this
repo:

- spec 02 — saturation: `[1.000,0,0]` must be flagged, `[0.998,0.001,0.001]` must
  not be (or the flag becomes noise nobody reads)
- spec 05 — a 5xx must be not-ready *and* a 404 must be not-ready, but for a
  different stated reason (wrong endpoint is not an unhealthy server)
- `laya-head` — `clamp_temperature`: 0.1006 clamps to 0.5, but an in-range 1.906
  must pass through untouched
