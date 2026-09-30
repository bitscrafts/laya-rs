# The decision ledger

**Status**: DESIGN — the schema is settled; `laya-store` is not yet implemented
(spec 04 owns it). This document is the contract that spec implements.

---

## What it is for

Every decision lands in an embedded **redb** store as a JSON document, so that:

- a cost and latency habit is **visible** rather than assumed;
- a routing decision can be **joined to the journal entry that consumed it**;
- calibration has raw material (see the caveat below);
- a threshold change can be tested **after the fact** against recorded decisions.

It is emphatically **not** a place to put state or secrets.

---

## The single-writer problem, and the design that avoids it

The design's first draft had every consumer (Hermes, pi, Claude Code, a hook)
spawn its own `laya-mcp` against **one** embedded store. The adversarial review
found this collides with redb's model: redb is an embedded **single-process**
database, and a hook firing while an agent session holds the store is the *normal*
case, not an edge case.

**Resolution — one writer, N readers:**

```
  laya-mcp (per consumer)          laya-cli
  ┌───────────────────┐           ┌────────────────────────────┐
  │ READ-ONLY open    │           │ `laya ledger compact`       │
  │ appends to its    │           │  = THE ONLY WRITER          │
  │ own segment file  │──────────►│  merges segments, compacts  │
  │ ledger-<pid>.jsonl│           └────────────────────────────┘
  └───────────────────┘                        │
                                               ▼
                                        ledger.redb
```

- A server process **never needs write access** to the store, so the collision
  cannot occur.
- Segments are append-only JSONL, so a crash loses at most the in-flight line.
- Compaction is an explicit act (`laya ledger compact`), not a background task —
  easy to reason about, easy to test.

**This must be proven, not assumed.** P1 includes a one-line cross-process test:

```
open the store for writing in process A, attempt the same in process B,
assert that B fails cleanly (a typed error, not a panic or corruption),
and assert that the segment-file path succeeds in both.
```

**Version note (review finding F10 correction):** `amem` pins `redb = 2`,
`pidag` pins `1.0`, and these differ in lock/repair behaviour. This workspace pins
**`redb = 4.2`** (the same major as `pi-improved`, which is the sibling we most
resemble). That is a deliberate choice, recorded here because the review was right
that the design's original claim ("redb 2 — amem, pidag") was half wrong.

---

## A ledger row

One JSON object per decision. Field names are the contract.

| field | meaning |
|---|---|
| `ts` | RFC 3339 timestamp, UTC |
| `run_id`, `node_id`, `seq` | the **join key to the graph journal** (review: the design promised this and never defined it) |
| `question_id`, `question_type` | which question, and which of the three types |
| `label` | the answer as a routing label |
| `confidence` | Laya's calibrated confidence |
| `band` | `low` / `mid` / `high`, computed by us (B5) |
| `margin` | **top1 − top2 gap** — added by blocker B1, because a near-tie is where a port can flip |
| `probabilities` | the full distribution, so saturation can be re-derived |
| `saturated` | whether one option held > `saturation_threshold` |
| `p` / `score` | for a `noul` / `score` |
| `latency_ms` | measured, wall-clock, on the **real** path (review M2: three different paths were conflated) |
| `laya_version` | the pinned Laya version |
| `head_sha256`, `backbone_sha256` | **provenance** — a mismatched artifact must fail loudly, not silently produce different decisions (B1/M9) |
| `bucket_clamped` | true when the head's temperature clamp fired (B2); confidence from such a bucket is **uncalibrated** |
| `state_sha256` | hash of the state, **never the state itself** — see the secrets rule |
| `tool` | which tool produced it (`decide`, `ask_file`, …) |

### What is deliberately absent

- **The state, verbatim.** A state can contain file contents, and a file can
  contain a secret. The ledger stores `state_sha256`; reproducing a decision needs
  the original state supplied by the caller.
- **Credentials.** No field holds one, and `ask_file` must refuse to read paths it
  has no business reading.
- **`act_probability`.** The head is inert on this checkpoint (measured ≡ 1.0
  including on gibberish). Surfacing it invites someone to gate on it. If the Rust
  port drops `act_head`, this decision becomes permanent — and that trade is
  recorded rather than silently made (review M3).

---

## `calibrate` has no ground truth yet — stated plainly

The review's finding **m7** is correct and we are not papering over it: **ECE needs
`(confidence, correct)` pairs, and the ledger records decisions, not outcomes.**
Nothing currently records whether a decision was *right*.

So `calibrate` is **design-only until an outcome-recording path exists**. The path
must answer: *who* labels a decision, *when*, and with *what* tool. Candidate
answer (not yet decided): the caller that acted on a decision reports the outcome
back — `laya ledger outcome <run_id> <node_id> --correct|--wrong` — at the point
where reality is known (a test passed, a fix worked, a human overrode).

**Do not build `calibrate` before the outcome path exists.** An ECE computed from
decisions alone would be a number that looks authoritative and means nothing —
exactly the class of defect this project exists to avoid.

---

## Budgets

Per-tool budgets are a `laya-store` concern: a call count and a cumulative latency
per tool per window, so the latency habit from review M2 is **visible** rather than
discovered when a hook slows every tool call. A budget breach is a warning by
default (recorded in the ledger), and configurable to refuse.

---

## Operations

```bash
laya ledger compact          # the single writer: merge segments into ledger.redb
laya ledger stats            # counts, latency percentiles, saturation rate
laya ledger tail             # the most recent decisions, human-readable
```

The **saturation rate** is the metric to watch: it is the number that tells us
whether the model's confidence is worth gating on at all.
