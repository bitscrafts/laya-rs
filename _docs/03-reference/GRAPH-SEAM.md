# The graph seam — laya-rs drives a DAG's `route` node

**Status**: the *contract* is IMPLEMENTED and tested; the *end-to-end run* is not built yet
(see "What is missing" at the end — stated, not implied).

**See [`GRAPH-SPEC.md`](GRAPH-SPEC.md) for our flow format and the flow → graph
translation.** Read `specs/02-route-payload.md` for the field-by-field contract. This document answers the
wider question: *what exactly is the seam, and what does this project have against it?*

> **Flow-author hazards (2026-10-02, spec 19).** Two rules govern what a flow author can expect
> from the seam — a `decide`'s state is a literal, and node array order once changed which nodes
> ran. They are stated once, in
> [`GRAPH-SPEC.md`](GRAPH-SPEC.md#hard-rules-for-flow-authors-added-2026-10-02-spec-19); not repeated here.
>
> **Question shape (spec 16).** A `decide` can ask a `noul` as well as a `choice`; the field, its
> default and the measured reason to prefer `noul` are in
> [`GRAPH-SPEC.md`](GRAPH-SPEC.md#question_type-choice-or-noul-spec-16).

---

## What a `route` node is, and what it needs

A DAG node of kind `route` takes an expression and a set of rules, and sends the payload down
the port whose rule matches:

```toml
[[node]]
id = "triage"
kind = "route"
on = "$.payload.label"                  # an expr; member access only
rules = [
  { eq = "billing", port = "billing_flow" },
  { eq = "auth",    port = "auth_flow"    },
  { port = "human" },                   # NO `eq` -> the fallback rule
]
```

Two details from the engine's source that a graph author must know:

* **`eq` is optional.** A rule without it matches anything, which is how the catch-all is written.
* **The first matching rule wins** — rules are tried in order and the first match is taken, so
  the fallback must come **last**. Put it first and every answer takes it.

**`eq` is exact `serde_json::Value` equality.** The expression language supports **object member
access only** (`$.a.b`) — no arrays, no functions, no arithmetic, no ranges. So the graph cannot
compute "confidence ≥ 0.8"; it can only compare a value that *someone else* put in the payload.

That single constraint shapes the whole design: **the band must be a field, because the graph
cannot derive it.**

---

## The seam, in one picture

```
  caller (Hermes / pi / a hook)
        │  MCP stdio, tools/call
        ▼
   laya-mcp ──────────────────────────────────────────────────┐
        │                                                     │
        │ decide / gate / ask_file / ask_files / ask          │  the payload
        │   (the model supplies VALUES; our code owns the     │  is what crosses
        │    arithmetic, thresholds and the decision)         │  the seam
        ▼                                                     ▼
   laya sidecar (the head)                         { label, band, margin,
   + llamacpp-laya backbone (:8083)                  near_tie, confidence, … }
                                                              │
                                                              ▼
                                                    route on = "$.payload.label"
                                                              │
                                                    one port, exactly one edge
```

**The invariant the design rests on:** the **DAG owns every edge**. Laya selects among ports that
were *drawn at author time*; it can never invent one. The edge set is fixed before any decision
runs, which is what keeps a graph deterministic while a probabilistic model feeds it.

---

## Why `band` is a field and not a computation

`laya-core::policy` computes the band from Laya's confidence and puts it in the payload **as a
string** (`low` / `mid` / `high`). A graph author then writes:

```toml
on = "$.payload.band"
rules = [ { eq = "high", port = "act" }, { eq = "mid", port = "confirm" },
          { eq = "low",  port = "human" } ]
```

An uncertain answer therefore **escalates along a drawn edge** — no graph change, no new node,
no threshold in the TOML. Tuning a threshold changes a number in *our* config, never the graph.

This was review blocker **B5**, and it was the one that invalidated the original design: the plan
had been for the graph to compute the band, which its expression language makes impossible.

---

## What this project has against the seam (all IMPLEMENTED, all tested)

| piece | where | why it matters here |
|---|---|---|
| `label` | `route_payload.rs` | the field `route on = "$.payload.label"` matches |
| `band` | `route_payload.rs` | the field `route on = "$.payload.band"` matches (B5's resolution) |
| `margin` | `route_payload.rs` | top1−top2. Spec 04 F3 required it and it **did not exist** — without it a near tie was decided silently, inside a graph whose premise is determinism |
| `near_tie` | `route_payload.rs` | with `escalate_on_near_tie`, demotes the band so a tie routes to the escalate port with **no graph change**. Default **false**, so the routing change is opt-in and shows in a diff |
| `confidence`, `saturated` | `route_payload.rs` | carried for the journal and the ledger |
| the label is always a declared option | `route_payload.rs`, test `label_is_always_one_of_the_declared_options` | the property that makes the graph's edge set fixed: whatever the model returns, the label is one of the author's declared options |

**A defect worth recording:** `margin`, `near_tie` and the whole `DecisionPayload` were built and
**unit-tested** while `decide` returned raw `answers` — so a graph author following the documented
`route on = "$.payload.label"` had **nothing to match on**. Unit tests on a constructor cannot show
that anything *returns* the thing. An end-to-end proof through the real binary caught it.

---

## The other half: the graph engine itself

The graph engine is **not in this repository.** It is a separate project we do not modify and
**have no association with** (user, 2026-09-30) — so this document states the properties we rely
on as **facts**, and our code carries their consequence. No path, no line number, no dependency.

**These are stated as facts about the interface, not as references into another project** (user,
2026-09-30: no association with other projects). The consequence of each is enforced in our own
code, so nothing outside this repository can move our behaviour silently:

1. **The expression language is member access only** (`$.a.b`) — so a band must be a payload
   FIELD; no graph can compute one.
2. **`eq` is exact equality**, and a rule with no `eq` is the fallback — so the payload carries
   strings (`"high"`, `"low"`), never a numeric threshold.
3. **The first matching rule wins** — so a fallback must be written last.

`route_payload.rs` **pins, by test, the two names a graph matches on** (`label` and `band`) and the
property that the label is always a declared option. Those tests are the enforcement: if the shape
our side emits ever changes, they fail.

---

## What is missing (stated plainly)

**Nothing yet runs a graph with a real payload end to end.** This repository emits the payloads;
the pieces are proven individually against the live model. What has *not* been built is the
consumer that takes `$.payload.label` from a real `laya-mcp` response and drives a `route` node.

That runner belongs **here, in `laya-rs`** — not in the project we borrowed the contract from.
Until it exists, "laya-rs drives DAGs" is a **tested contract**, not a demonstrated end-to-end
capability, and it should be described that way.
