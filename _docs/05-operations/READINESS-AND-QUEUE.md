# Readiness and the priority queue

Two mechanisms that exist because a decision layer has failure modes a normal
service does not. Both are implemented and tested in `laya-core`.

---

## 1. Readiness — "is Laya actually running?"

**User directive (verbatim):** *"it will be mandatory the app to check for laya be
running! and it must inform/stop if not!"*

### The rule

**A decision layer that cannot decide must not run, and an unavailable gate must
never mean permission.** Those are two halves of one rule, and both are enforced.

- **At startup**: the server probes Laya and, if it is not usable, writes the
  diagnostic to stderr and **exits non-zero**. It does not serve in a degraded state
  pretending to work.
- **While running**: readiness is re-checked through a short cache. If Laya dies,
  **`gate` answers `ask`** — never `allow` — and every other tool returns a protocol
  error rather than a defaulted success.

### What counts as "usable"

| probe result | verdict | why |
|---|---|---|
| `/health` → 200 `{"status":"ok"}` | healthy | the documented shape |
| `/health` → 200 with junk or no `status` | **not ready** | **a 200 is not proof of health** — this is how a server reports ready and then fails every decision |
| `/health` → 200 `{"status":"loading"}` | **not ready**, quoting `"loading"` | it parsed and has the field; it just is not ready |
| any non-200 | **not ready**, with the code | a **404** means the *wrong endpoint*, not an unhealthy server — saying "unhealthy" sends you to restart a healthy container |
| connection refused / timeout | **not ready** | two different remedies (restart vs network), so two different outcomes |
| `/v1/models` empty, or our model absent | **not ready**, naming the model | the server is up but serving the wrong weights |

### Two traps we hit, both pinned by tests

**The capability string is not a valid signal.** The served model reports
`capabilities: ["completion"]` — **not** `"embedding"` — because the GGUF holds the
backbone only. A check that required an `"embedding"` capability would report a
perfectly working server as broken. We check the **model id** instead.

**A 404 must not read as "connection failed".** Measured: run against the live
server with a wrong path, the first version printed
`reason: connection failed: http status: 404` — which points the reader at the
network when the real problem is the URL. `ureq` reports HTTP errors as `Err`, so
the status is now recovered and reported as `unexpected HTTP status 404`.

### Using it

```bash
laya doctor                    # exit 0 ready, exit 1 not ready (stderr has the reason)
laya doctor --url http://...:8083
```

The exit code is the point: a hook, a cron job or a human can check without parsing
prose. The diagnostic names the endpoint tried, the `curl` to verify, and the
container to start — because the reader has not read this source.

---

## 2. The priority queue

**User question (verbatim):** *"on redb collisions, could be used a priority queue?"*

### The measurement that shapes the answer

```
Laya /embedding          : 35 ms median
8 concurrent /embedding  : 121 ms wall   => 5.8x parallel, NOT serialised
redb, 2nd process        : Error: DatabaseAlreadyOpen
```

**The backbone is not the bottleneck.** So a queue in front of it buys no
throughput, and building one "to scale" would add a scheduling layer for nothing.

The queue has **two distinct jobs**, and conflating them is the mistake:

| job | needed for | priority's value |
|---|---|---|
| **Work scheduling** in front of the decision path | fairness: a blocking `gate` must not wait behind a bulk file scan | **high** — the latency is user-visible |
| **Ledger write serialisation** | solving the redb collision (one writer opens the store) | **low** — records are records |

So priority is justified by the **work** lane. For the store it is a
serialisation queue whose ordering is a nicety.

### The design

```
   Gate        ─┐
   Interactive ─┤──► bounded priority queue ──► worker(s) ──► Laya /embedding
   Background  ─┘        (class, then FIFO)
```

Three classes, highest first: **`Gate`** (a hook blocking an agent's tool call),
**`Interactive`** (a human or agent waiting on a `decide`), **`Background`** (bulk
scans, calibration, maintenance).

Three `VecDeque`s rather than a heap, because **FIFO within a class is then
structural** (push back, pop front) instead of depending on a sequence number to
break ties — there is no way to get the tie-breaking wrong. FIFO within a class is
also what bounds a class's wait once it is being served.

**Shedding, when full — and the fail-closed rule:**

- a victim is only ever **strictly less urgent** than the item being admitted, so
  bulk traffic can never evict the work it should be waiting behind
- **a `Gate` is never shed.** If a full queue cannot admit one, the enqueue is
  **refused** with a typed error, and the caller answers **`ask`** — never `allow`.
  Silently dropping a gate reads as *permission*, which is the failure this project
  exists to prevent.
- depth, refusals and sheds are **counted**, so a saturated system is visible rather
  than merely slow

### Using it

```rust
let mut q: PriorityQueue<Job> = PriorityQueue::new(256);
q.push(Priority::Gate, job)?;          // preempts background work
match q.push(Priority::Background, j) {
    Err(EnqueueError::Full { .. }) => { /* a disabled/degraded path, not a silent drop */ }
    Ok(_) => {}
}
```

---

## What is NOT designed here, deliberately

**No queue in front of the backbone for throughput.** 5.8× parallel headroom at 8
concurrent means there is no throughput problem to fix. If a future measurement
shows contention, that is the time to revisit it — with the number in hand.
