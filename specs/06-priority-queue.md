# Spec: 06 — Priority scheduling and the single-writer queue

## Overview
**User question (verbatim):** *"on redb collisions, could be used a priority queue?"*

Answer: **yes — but the measurement changes where it earns its place.** We measured
two things before designing this:

```
Laya /embedding   : 35 ms median (sequential) · 8 concurrent = 121 ms wall (5.8× parallel)
redb, 2nd process : Error: DatabaseAlreadyOpen        <- blocker B4, now PROVEN
```

**What that tells us:**

1. **The backbone is not the bottleneck.** It is fast and it parallelises, so a
   queue in front of it is *not* a throughput necessity at plausible load. Ordering
   it would be premature.
2. **The redb collision is real and it is a clean typed error** (`DatabaseAlreadyOpen`,
   not corruption) — exactly what spec 04's guardrail requires. So the store needs
   **exactly one writer**, and that is a *serialisation* problem.

**Therefore the queue has two distinct jobs, and they must not be conflated:**

| job | needed for | priority's value |
|---|---|---|
| **Work scheduling** in front of the decision path | fairness: a blocking `gate` must not wait behind a bulk file scan | **high** — this is where latency is user-visible |
| **Ledger write serialisation** | solving B4: one writer opens the store | **low** — records are records; ordering barely matters |

So: a priority queue **yes**, and its real justification is the *work* lane. For the
store it is a **serialisation queue** whose ordering is a nicety, not the point.

**Status: IMPLEMENTED, AND DELIBERATELY UNWIRED.** `laya-core::queue` has the bounded queue and 16 tests, and nothing calls it: the server serves one client over stdio on a single thread, so dispatch is already serial. The queue is kept for a future multi-client server and this line is why its absence from the build is not a defect.

**Project**: /projects/laya-rs
**Phase**: P1

## Requirements

### Functional
- F1. Three priority classes, highest first:
  - `Gate` — a hook blocking an agent's tool call. **The most latency-critical
    thing in the system.**
  - `Interactive` — a human or an agent waiting on a `decide`.
  - `Background` — bulk scans (`ask_files`), calibration, maintenance.
- F2. **Ordering**: strictly by class, then FIFO within a class. No starvation
  (FIFO within class guarantees a bounded wait once a class is being served).
- F3. **Bounded**: the queue has a maximum depth; enqueueing beyond it is refused
  with a typed error, never an unbounded allocation.
- F4. **Shedding under saturation**: when full, shed `Background` first, then
  `Interactive`. **Never shed a `Gate` by default** — but if one *must* be shed,
  the answer is `ask`, never `allow` (the fail-closed rule).
- F5. **The single-writer queue**: a server process enqueues a decision record; the
  writer (`laya ledger compact`) drains. The server **never opens the store for
  writing** (B4).
- F6. A drained record is removed only after a durable write, so a crash between
  the two loses at most the in-flight record and never corrupts the store.
- F7. Queue depth and shed counts are observable (the ledger's `stats`), so a
  saturated system is **visible** rather than merely slow.

### Non-Functional
- NF1. The **ordering and shedding policy is pure** and tested with zero I/O —
  every class, every overflow case, and the never-shed-a-gate property.
- NF2. Enqueue is **non-blocking** for the caller where the class allows it; a
  `Gate` waits only as long as it must.
- NF3. No `unwrap`/`expect`/`panic`; a full queue is a returned error.

## Architecture

```
   Gate        ─┐
   Interactive ─┤──► bounded priority queue ──► worker(s) ──► Laya /embedding
   Background  ─┘        (class, then FIFO)          │         (~35ms, parallel)
                                                     ▼
                                          decision records ──► queue file
                                                                  │
                                                       `laya ledger compact`
                                                       = THE ONLY redb WRITER
```

**Why not a queue in front of the backbone (yet):** measured 8 concurrent calls
complete in 121 ms. Ordering a path that is not contended adds a scheduling layer
and buys nothing. The queue exists to give the *gate* lane precedence when both
lanes are busy, and to bound the system — not to increase throughput.

**Why the store needs no priority:** a decision record is an append. The compactor
drains the queue in whatever order it likes; the only requirement is that it is the
**only** process opening the store for writing, which is what
`DatabaseAlreadyOpen` enforces for us.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_higher_class_is_served_first` | Background then Gate | Gate popped first |
| `within_a_class_order_is_fifo` | 3 Background items | insertion order preserved |
| `a_gate_never_waits_behind_background` | 100 Background, then 1 Gate | Gate is the next pop |
| `classes_are_ordered_gate_then_interactive_then_background` | one of each, enqueued in reverse | popped Gate, Interactive, Background |
| `enqueueing_beyond_capacity_is_refused` | capacity N, N+1 items | typed `Full` error, no growth |
| `background_is_shed_first_when_full` | full of Background, enqueue Interactive | shed Background |
| `interactive_is_shed_before_a_gate` | full, enqueue a Gate | an Interactive is shed, the Gate admitted |
| `a_gate_is_never_shed_by_default` | full of Gates, enqueue a Gate | the new Gate is REFUSED (not silently dropped, not admitted by dropping another Gate) |
| `shedding_a_gate_yields_ask_never_allow` | a shed `Gate` | the decision is `Ask` |
| `depth_and_sheds_are_reported` | mixed traffic | counts match what was enqueued and shed |
| `an_empty_queue_pops_nothing` | empty | `None`, no panic |
| `a_drained_record_is_only_removed_after_a_durable_write` | a write that fails | the record is still queued |

## Exit Criteria
- [x] `cd /projects/laya-rs && cargo test -p laya-core --lib queue` — all pass
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` — zero
- [x] `cd /projects/laya-rs && cargo run -q -p laya-core --example queue_demo` — prints the ordering

## Guardrails
- Do NOT add a queue in front of the backbone to increase throughput; measured 5.8×
  parallel headroom means there is no throughput problem to fix.
- Do NOT let a server process open the store for writing (B4 — `DatabaseAlreadyOpen`).
- Do NOT silently drop a `Gate`. Refuse it (so the caller can fail closed) rather
  than pretend it was served — a dropped gate reads as permission.
- Do NOT grow unboundedly. A full queue is a returned error and a **visible** count.
- Do NOT let priority become a way to starve `Background` forever: FIFO **within** a
  class bounds the wait once that class is served.
