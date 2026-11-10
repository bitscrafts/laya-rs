# The decision ledger

**Status**: IMPLEMENTED (2026-09-30). `laya-store` implements this schema; the server appends a
row per decision to its own segment file; `laya ledger compact` merges them. Proven end to end by
`scripts/archive/prove-ledger.py`.

> ## What is in the store TODAY
>
> **Only what has been compacted.** A `laya-mcp` process appends to `ledger-<pid>.jsonl` in
> `LAYA_LEDGER_DIR` (unset = no ledgering, deliberately); nothing reaches `ledger.redb` until
> `laya ledger compact` runs. So "the ledger is empty" is the normal state of a fresh install, not
> a fault — and `laya ledger stats` says so rather than reporting zeros as if they were measured.
>
> **`calibrate` still cannot run**, and the reason is now concrete rather than architectural: the
> ledger records what was decided, and outcomes only arrive when someone records them with
> `laya ledger outcome <id> --correct|--wrong --source <who>`. `stats` reports
> `outcomes: 0` and prints **NO OUTCOMES RECORDED** while that is true.

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

**Version note (review finding F10 correction):** alternate `redb` majors differ in lock and
repair behaviour, so this project pins **`redb = 4.2`** — chosen here, not inherited. The
review was right that the original version claim was half wrong; the corrections it produced
are recorded in the review doc.

---

## A ledger row

One JSON object per decision. Field names are the contract.

| field | meaning |
|---|---|
| `ts` | a human-readable stamp — **`epoch+{secs}`, not RFC 3339**. This table claimed RFC 3339 and the function never produced it; the claim is corrected rather than the wire format changed |
| `ts_nanos` | monotonic nanoseconds **from the writing process's start**. Recorded, but **not** an ordering key: two processes measure from their own start |
| `wall_nanos` | **epoch nanoseconds — the ordering key.** Added 2026-10-02 after the live store showed server rows at `65..171` against CLI outcome rows near `1.79e18`; a ~10^16 gap put every outcome after every decision whatever the real time |
| `run_id`, `node_id` | the **join key to the caller's own run and node**. Threaded from the tool call (2026-10-02); every row before that carried `None`, so no decision could be joined to its run |
| `question_id`, `question_type` | which question, and which of the three types. For a `refused`/`error` row the `question_id` is the reason, with any command text replaced by `<command>` |
| `label` | the answer as a routing label. A call that produced **no answer** is a row too (spec 11 R2): `refused` (the caller asked for something the layer will not do — an out-of-scope file, a command the gate did not allow) or `error` (the layer could not answer — the head is down). These are the rows an incident review reaches for first |
| `confidence` | Laya's calibrated confidence — **`Option<f64>`**. It was `unwrap_or(0.0)`, so a missing value recorded as `0.0`: indistinguishable from "maximally unsure", and it poisons any calibration built on the column |
| `band` | `low` / `mid` / `high`, computed by us (B5) |
| `margin` | **top1 − top2 gap** — added by blocker B1, because a near-tie is where a port can flip |
| `near_tie` | whether `margin` fell under the policy floor. **Added 2026-09-30** — it was a payload field while this table omitted it, and with `escalate_on_near_tie` it is what demotes a band, so a recorded decision without it cannot be re-explained |
| `probabilities` | the full distribution, so saturation can be re-derived |
| `saturated` | whether one option held > `saturation_threshold` |
| `p`, `score` | for a `noul` question the raw belief, for a `score` question the rubric position. **Annotated 2026-10-02 (spec 11 R9):** this table first listed them and `DecisionRow` did not have them — a reader auditing the ledger hunted for columns that were never built — and an interim edit struck them through as "not fields". Spec 11 R9 chose the other resolution: `DecisionPayload` already carried both and the recorder simply did not copy them, so it now does, and a `noul`'s raw probability is recoverable from the row. Rows written before this carry neither |
| `gate_decision`, `model_decision`, `destructive_signals` | **added 2026-10-02 (spec 11 R2)** for `gate` and `ask` rows: the final `allow`/`ask`/`deny`, what the model alone said (`yes`/`no`/`unknown`), and how many destructive-command signals the guardrail found. An `ask` row's `label` is the model's answer to one of the caller's questions, so the verdict needs a field of its own. Absent on rows from tools with no verdict |
| `command_sha256` | **added 2026-10-02 (spec 11 R2/N3)**: hash of the command a `gate` or `ask` row judged. **Never the command** — a command can carry a token, and the test greps the whole segment for the plaintext |
| `latency_ms` | measured, wall-clock, on the **real** path (review M2: three different paths were conflated) |
| `laya_version` | the pinned Laya version |
| `head_sha256`, `backbone_sha256` | **provenance** — a mismatched artifact must fail loudly, not silently produce different decisions (B1/M9) |
| `bucket_clamped` | whether the head's temperature clamp fired (B2); confidence from such a bucket is **uncalibrated**. Now `Option<bool>` — it used to be copied from `saturated`, a different fact, so an unknown clamp state was recorded as a known `false` |
| `executed` | `Option<bool>` since 2026-10-02 (spec 11 R2) — `None` for a tool with no such notion and for every older row, so "not applicable" is not recorded as "did not run". Whether the action judged was **carried out**. For a `gate` row this separates "allowed" from "allowed and then run" — a permission and an event are different things, and the ledger needs to tell them apart |
| `state_sha256` | hash of the state, **never the state itself** — see the secrets rule |
| `tool` | which tool produced it (`decide`, `ask_file`, …) |

### Row identity (changed 2026-10-02)

A row's identity — what makes `compact` idempotent — now includes `wall_nanos`. It was
`(ts_nanos, tool, question_id, label)`, and `ts_nanos` is process-relative, so two servers answering
the same question with the same label at the same offset into their own lives (a gate verdict at
`ts_nanos` 65 is entirely ordinary) produced the same identity and the second row was dropped as a
"duplicate" of a different decision. Measured by a test that fails on the old identity. Old rows
carry `wall_nanos` 0, so their identities are unchanged in effect.

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
