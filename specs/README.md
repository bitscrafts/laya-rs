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
| [`03-rust-head.md`](03-rust-head.md) | P4 | NOT STARTED — the 26.5M head in pure Rust (`clamp_temperature` already landed) |
| [`04-mcp-server-and-ledger.md`](04-mcp-server-and-ledger.md) | P1 | NOT STARTED — the stdio server + the single-writer ledger |
| [`05-laya-must-be-running.md`](05-laya-must-be-running.md) | P1 | **IMPLEMENTED** — judgement + probe parsers + `laya doctor`, all tested |
| [`06-priority-queue.md`](06-priority-queue.md) | P1 | **IMPLEMENTED** — the bounded priority queue, 16 tests |

**103 tests pass across the workspace, clippy is at zero warnings, fmt is clean,
and `cargo xtask gate` reports PASS on all three steps.**

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
