# ENGINE DEFECTS — findings in the graph executor

> **SUPERSEDED IN PART — 2026-10-02, after an independent review.**
> A claude+opus review (`04-development/OPUS-REVIEW-GENERIC-GRAPHS.md`, 25+ probe flows, every claim
> run) found that two of the three items below are **worse than recorded here**, and it found a
> more severe defect this document misses entirely. The original wording is kept below, unedited,
> because the disagreement is the finding.
>
> - **D1 is correct but UNDERSTATED.** It is one case of a bigger defect: routing is not a function
>   of the graph at all. Which nodes run depends on **node order in the JSON array**. Re-verified
>   here on `p6-file-order`: with `inner_B` listed first, `outer_A` routes to `A_no` **and `B_yes`
>   still runs** — reached only through a port `outer_A` did not select — while `RunReport` reports
>   `not_started: []`, so the wrong execution is invisible. Three mechanisms: `prune_unreachable`
>   deletes unselected port targets without checking other inbound edges; `remove_node` makes a
>   pruned dependency count as satisfied; `find_route_pair` takes the first `Route` in `Vec` order.
> - **D2 is WRONG in the safe-sounding direction, which is the worst way to be wrong.** The field is
>   not "inert". `near_tie_port` is never lowered into an edge, so its target has **no inbound edge**
>   and becomes a root of phase 2: **it runs unconditionally on every run.** Re-verified on
>   `p12-near-tie` — `human` is unconnected in the render and ran anyway.
> - **D3 stands**, with its corollary (an `mcp` node calling `decide` gets overwritten) confirmed by
>   a live refusal: ``decide` requires `state``.
>
> The review also **withdrew a claim this project made in `HEAD-QUESTION-SHAPE-LIMITS.md`**: the
> 2-option `choice` is not "the weaker shape". Measured through `laya graph run`, a choice is
> **15/15 under the v1 phrasing**; the boundary failures came from the v14c phrasing chosen later.
> `noul`'s real advantage is **confidence** (0.875–1.0 against 0.35–0.54), not label correctness.

---

**Originally recorded 2026-10-02**, while building the `wc` example. Each was reported by a delegated
worker's source analysis and **independently confirmed here** against the tree and a live run.

---

## D1 — `depends_on` from a shell to a `decide` does not order execution

**The finding.** `run_flow` drains **every** mcp+route pair before the tail schedules anything else,
so a `decide` node runs before the shell node it declares as a dependency.

**Verified in source** — `crates/laya-graph/src/route_exec.rs:67`:

```rust
while let Some((route_node, upstream_node)) = find_route_pair(&nodes, &edges)? {
```

That loop consumes every route pair first. The shell tail is only reached after it empties.

**Verified live** — `wc-classify-chained.flow.json`, ordered by `started_at_ms`:

| node | started_at_ms |
|---|---|
| `gt300_gate` | 0 |
| `gt300_ask` | 185 |
| `gt100_gate` | 346 |
| `gt100_ask` | 505 |
| **`count_words`** | **703** ← last |

`count_words` declares `depends_on` on nothing and is depended on by `gt300_gate`. It still ran
**last**, 703 ms after the first decision.

**Why it matters.** A flow author reads `depends_on` as ordering. It is not. It declares intent for
`validate` (and feeds `topo_order` for the tail), but a `decide`'s pair is pulled out and run first
regardless. **A flow whose decision depends on a shell result will decide on stale or absent data
and look correct doing it.**

**The consequence for the example.** The `wc` node in `wc-classify-chained.flow.json` is **inert
decoration**: its output goes nowhere, and the classification is driven entirely by the count
written into each `decide` node's `prompt` text. The example demonstrates the graph's shape and
proves nothing about `wc`. `GRAPH-EXAMPLE-WORDCOUNT.md` carries the correction.

## D2 — `escalate_on_near_tie` and `near_tie_port` are declared, validated, and inert

**The finding.** `validate` requires a destination when the flag is set, but nothing ever routes to
it.

**Verified in source:**

- `crates/laya-graph/src/validate.rs:79` — `if node.escalate_on_near_tie && node.near_tie_port.is_none()` → refused. So a flow author MUST supply a port.
- `crates/laya-graph/src/lower.rs` — **no reference to `near_tie` at all.** No near-tie port is ever emitted.

**Why it matters.** The field pair reads as a safety feature: *"if the head is near a tie, don't act
on the label — go here instead."* `validate` accepts it and enforces the port, which makes it look
implemented. It does nothing. A near tie routes by `label` with no escape hatch, and the only
warning is silence — the failure mode this project has paid for repeatedly.

The payload **does** carry `near_tie`, so the information exists; only the routing is missing.

## D3 — the `decide` question type is hardcoded to `choice`, so a graph can never ask a `noul`

**Verified in source** — `crates/laya-cli/src/mcp_client.rs:105-115`:

```rust
"questions": {
    "answer": {
        "type": "choice",              // ← hardcoded
        "instructions": question,
        "criteria": criteria,          // {option: option}
    }
}
```

Every graph `decide` becomes a `choice` over its `options`, whatever the node declares. A `Node` has
no question-type field to set (checked `flow.rs`: `question`, `options`, `prompt`, `on`, `ports`,
`near_tie_*` — no type).

**Why it matters, with the measurement.** The same threshold question measured on this head:

| shape | result |
|---|---|
| `noul` "greater than 300?" | **15/15** correct, including the 300/301 boundary (conf 0.89–1.0) |
| 2-option `choice`, the graph's only form | **20/22**, failing AT both boundaries (conf 0.16 and 0.014) |

So the graph's *only* available question form is the weaker one, and the head's clean arithmetic is
unreachable from a flow. That is a design limitation worth deciding about, not a bug to patch blindly.

**Corollary, also measured:** because `tool == "decide"` is rebuilt from `question`/`options`/
`prompt`, a hand-written `mcp` node calling `decide` gets its `state`/`questions` **overwritten**.
The `mcp` node type cannot be used to bypass D3.

---

## What is NOT claimed

- **These are properties of the engine as committed**, not of the head. The head compares numbers
  correctly when asked a `noul`; D3 is what prevents a flow from asking one.
- **D1's ordering is only shown for shell→decide.** Whether other combinations reorder was not
  measured.
- **D2's inertness is established by absence of a reference in `lower.rs`** plus the live behaviour
  of every run so far (no near-tie port ever selected). A run that deliberately forced a near tie
  was not constructed.
- **No fix is proposed here.** D1 and D3 change graph semantics or the wire contract; that is a
  design decision for the project, not a drive-by patch.
