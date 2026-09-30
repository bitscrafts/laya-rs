# 03 — Four architecture questions (discussion, nothing implemented)

**Date:** 2026-09-30 · **Status:** RESEARCH — a conversation record, no code changed.
**Subject:** four questions asked together, because they are one question wearing four hats:
*where does authority live, and what is data versus what is code?*

> **REVISION 2 (2026-09-30).** The user pushed back on all four, and **three of the four
> corrections were right and are accepted below.** Round 1 is kept where it was wrong, with the
> superseded wording visible, because the disagreement is the finding. §1 gained a whole axis I
> had missed (D) and the admission-control role the existing queue was written for; §2 gained the
> load-time question and two good ideas of the user's (mermaid export, starter flows); §3 was
> based on a misreading and is rewritten; §4's feasibility answer was the *wrong test* and is
> corrected. Summary of what changed, at the end of each section under **"What the pushback
> changed"**.

| # | question | round 1 answer | round 2 answer |
|---|---|---|---|
| 1 | Safe multithreading is a must | yes, A+C, not B | **yes — and the queue is the mechanism for A; plus axis D, which round 1 missed entirely** |
| 2 | Config: hardcoded? JSON? redb? | split by author | unchanged, **plus**: redb does *not* speed the load; mermaid export is mandatory; ship starter flows as text, not a `.redb` |
| 3 | Make our app a Laya-based graph runner? | no | **round 1 answered a different question — building OUR spec is sound; see the corrected design** |
| 4 | Can we supersede the DAG engine? | no, not close | **feasible; the objection is data (two ledgers), not LOC — and the judge role is executor-independent, so this is deferrable** |

---

## 0. The measurement everything below rests on

Read from the real checkpoint's tensor shapes (`laya-head.safetensors`), not from
the config or from the paper.

```
d_model 1024 · n_head 16 · head_max_len 192 · 2 layers · d_ff 4096

  in_proj_weight  [3072, 1024]   q,k,v fused  = 3·d × d
  out_proj.weight [1024, 1024]                =   d × d
  linear1.weight  [4096, 1024]   d_ff × d
  linear2.weight  [1024, 4096]   d × d_ff
```

Per layer: `(4·d² + 2·d·d_ff)·n` = 2.42 G-MAC, so **two layers = 4.98 G-MAC
≈ 9.97 GFLOP per call** at the real `n = 192`.

At a pessimistic scalar 2 GFLOP/s/core, that is **≈ 4 983 ms on one core**,
1 246 ms on four, **623 ms on eight**. The currently-deployed Python sidecar
answers in 173–436 ms, so the port does not merely have to be correct — to beat
what it replaces it needs **both** parallelism and vectorisation.

### A correction to a number I already committed

`specs/03-rust-head.md` and commit `73232d3` state "7.55 G-MAC = 15.1 GFLOP per
call". **That figure is 1.5× too high.** Traced: 7.55 G-MAC is what the formula
gives at `n = 300`, and it was carried across from an earlier scratch estimate
instead of being recomputed from the real shapes. The measured value at
`head_max_len = 192` is **4.98 G-MAC**.

The error is left visible here rather than quietly overwritten, per this
project's own rule (*never erase a superseded claim; the disagreement is the
finding*) — and because the failure mode is instructive: the number *looked*
derived, had units, and went unchallenged for a commit, which is exactly the
"an instrument reporting a plausible number" trap. **A constant that is not
recomputed from the live artefact is a claim, not a measurement.** The spec is
annotated in the same pass.

---

## 1. Multithreading, the priority queue, and other data structures

### Round 1, and what was wrong with it

Round 1 said: parallelise **A** (across calls) and **C** (output-partitioned
rows), forbid **B** (split-K, which reorders a summation and breaks determinism).
That part stands. But it left **A** implemented by "a pool in front of the pure
function" with **no named mechanism and no data structure** — and this project
already contains the mechanism, sitting unwired in `laya-core::queue`. Asking
"doesn't the priority queue fix this?" is the right question, and the honest
answer has three parts.

### First: does the queue fix split-K? No — they are orthogonal, and cannot touch

**Split-K is about how one dot product's summation is partitioned. A queue is
about which request runs next.** A queue has no access to the inside of a
matmul, so nothing about ordering can fix an arithmetic property. Concretely:
`PriorityQueue` orders `T`; split-K changes the *result* of a single call. They
live at different layers and neither can substitute for the other.

### Second, and this is the correction: the queue IS the missing half of A

Round 1's A was "parallelise across calls" with no mechanism. The queue is
exactly the mechanism for A, and it was written for precisely this:

```
Priority::Gate        > Interactive > Background
Priority::shedable()    — the classes that may be dropped under pressure
PriorityQueue::new(capacity)  — bounded, refuses rather than growing without limit
EnqueueError::Full      — fail closed, not queue-forever
```

So the design should be: **the pool is the worker set; the queue is its
admission control.** Requests arrive, are classified (`Gate` for a gate call,
`Interactive`, `Background`), and a bounded capacity provides backpressure. That
is the shape round 1 was gesturing at and never named.

**Two things stand in the way, and they are real work, not a wiring exercise:**

1. `PriorityQueue<T>` is **a plain struct** — `lanes: [VecDeque<T>; 3]`, no
   `Mutex`, no `Condvar`, no wake mechanism. Sharing it across worker threads
   needs synchronisation it does not have.
2. The server does **not** currently have concurrency to prioritise: it runs
   `tokio::runtime::Builder::new_current_thread()`, and there is a test in
   `laya-mcp` that **fails the build if `PriorityQueue` appears in production
   code**:

   > *"the queue could never fill and its 'queue is full, fail closed' error
   > could never fire. It read as protection and provided none. The queue is
   > gone; this test exists so a future contributor who adds one back has to
   > think about whether this server has any concurrency to prioritise (it does
   > not: one client over stdio, current-thread runtime)."*

   **That test is right for the current server, and it becomes the specification
   for the change.** Adding the pool is what makes the queue meaningful; the test
   is what forces that order. Pool first, queue second — not the reverse.

### Third: the axis round 1 missed entirely — D, inside one call

Round 1's split was A/B/C. There is a fourth site:

| site | what is split | determinism | verdict |
|---|---|---|---|
| **A. across calls** | two independent decisions | untouched | safe — the queue's job |
| **B. split-K** | one dot product's summation | **reordered every run** | **forbidden** |
| **C. output-partitioned** | a matmul's output rows | **bit-identical to scalar** | safe — the kernel's job |
| **D. intra-call, across independent matmuls** | *different* matmuls of one layer | **bit-identical** — different outputs, no shared reduction | safe, and round 1 overlooked it |

A layer's projections are separate, independent matmuls feeding separate
outputs; and there are two layers. D is safe for exactly the reason C is — no
shared reduction — and it costs almost nothing to add once C exists, because it
is the same proof applied at a coarser grain.

### Fourth: what is actually wrong today (the honest problem list)

Measured, and it reframes the whole question: **throughput is not the problem.**

| problem | measured | is it real? |
|---|---|---|
| throughput | 8 concurrent calls complete in **121 ms**; one call is 173–436 ms | **no** — the machine is idle most of each call |
| head-of-line blocking | a `Gate` call arriving behind 8 `Background` calls waits for all of them | **yes** — latency, and it is the queue's purpose |
| no shedding under load | a bounded queue exists but is unused | **yes** |
| **fault isolation** | one panicking call takes the whole process, and the sidecar is a single process | **yes, and this is the strongest argument for the pool — not speed** |
| cancellation | nothing can cancel an in-flight call | yes, and unsolved |

So the pool's justification is **ordering, backpressure and fault isolation**,
and only then latency. Round 1 justified it by speed, which the measurement does
not support.

### Other data structures, and pub/sub — a ranked answer

**The queue's shape is already right** (three lanes, bounded). What it lacks is
concurrency, not structure. Ranked:

1. **(Recommended) One bounded channel per lane**, not a `Mutex<PriorityQueue>`.
   A shared lock makes every worker contend on one mutex and needs a `Condvar`
   to wake it; a bounded `mpsc` per lane gives the same ordering semantics, hands
   the wake-up to the runtime, and provides backpressure for free (a full channel
   *is* `EnqueueError::Full`). It also removes the queue's own struct from the
   hot path, which is what lets the forbidding test be rewritten honestly rather
   than deleted.
2. **`Mutex<PriorityQueue<T>>`** — works, simplest diff, keeps the 16 tests
   meaningful as-is. Correct but contended, and it makes the queue a global
   bottleneck on the one path that exists to remove bottlenecks.
3. **A lock-free structure** — not considered. There is no measurement demanding
   it, and this project's rule is that a mechanism is justified by a measurement.

**Pub/sub — right idea, and it is the wrong tool for half the job.** The two
directions have opposite requirements:

| direction | shape needed | why |
|---|---|---|
| **ingest** (request → worker) | **work queue** | each request must be handled by **exactly one** worker. A topic gives you *at-least-one-of-N* semantics, which is how a decision gets computed twice, or zero times |
| **egress** (decision made / band changed → whoever cares) | **pub/sub or a tail-able stream** | many consumers, one event, no ownership |

**Ingest: never pub/sub.** **Egress: pub/sub, and it is genuinely missing
today.** Right now the only record a decision happened is a ledger **append**;
anything wanting to *react* to a band change must poll or re-read. That is an
unfilled gap, and redb already supplies the durable substrate — the segments are
append-only and `laya ledger tail` exists.

**One caveat that decides the implementation:** in-process pub/sub dies with the
process, and supervision is unsolved. So egress must be **at-least-once from the
ledger**, not a memory bus — a subscriber should be able to ask "what happened
since cursor N" and get it after a restart. Otherwise the notification path
silently loses events exactly when it matters, which is the "read as protection
and provided none" failure mode this project has already been bitten by twice.

### What the pushback changed

- **A is now named**: the priority queue is the pool's admission control, and the
  existing `Gate/Interactive/Background` classes are the right ones.
- **D is added** — intra-call parallelism across independent matmuls, which
  round 1 missed.
- **The justification moved** from throughput (unsupported by measurement) to
  ordering, backpressure and fault isolation (all measured or structural).
- **Order is now fixed**: pool → then queue. The forbidding test is the gate.
- **Ingest vs egress is now split**: channels for one, pub/sub from the ledger
  for the other.

---

## 2. Where should the configuration live?

### What is true today (measured)

Not "hardcoded" in the sense of scattered magic numbers — but **effectively
compiled in**:

```
crates/laya-core/src/policy.rs:186   pub fn from_toml(text: &str) -> Result<...>
crates/laya-mcp/src/lib.rs:1129      let policy = laya_core::Policy::default();
```

`from_toml` exists, handles TOML, validates ranges, rejects unknown fields — and
**is called by nothing outside its own tests.** Every production site takes
`Policy::default()`. So the config system is another instance of this project's
recurring shape: *a documented capability no caller can reach*. The `toml`
dependency is in the manifest for a parser no binary invokes.

### "Won't redb allow faster loads?"

**No — redb is not a loader, and the 24 s is not a config cost.** Separating the
two, because the question assumes one thing where there are three:

| what is loaded | size | where it should live | does redb help? |
|---|---|---|---|
| the head's **weights** | 106 MB, 26.5 M params, ~24 s | a file (safetensors), memory-mapped or read once | **no** — redb is a B-tree store, not a tensor loader; putting 106 MB of floats in it would make the load *slower* |
| **policy / thresholds** | a few KB | text (see below) | no measurable gain at this size |
| **measured values** (ECE, sample counts) | a few rows | redb | not a load-time win; a *correctness* win |

So the load-time question has a different answer than expected: **redb cannot
make the head load faster, and should not be asked to.** What it *can* do is make
the load **honest** — store a fingerprint of the checkpoint it expects and verify
it before use, which is the same fail-closed posture as the Laya-readiness
preflight. That is worth having, and it is not about speed.

The real load win is already planned and is **P4**: today the 24 s is paid by a
long-lived Python sidecar; once the head is in-process it becomes a startup cost
paid once, with no socket and no serialisation. redb does not participate.

### "Can the flows be exportable — for human check, like mermaid?"

**Yes, and this is the strongest of the four suggestions.** It also inverts the
design order, and that is the important part:

> **If a flow lives in redb, a text renderer is mandatory, not optional** —
> because a binary store removes the only human-readable view that would
> otherwise exist.

So the order becomes: **text format / renderer first, store second.** Storing
first and rendering later is how a binary container becomes unauditable.

Design notes that follow from it:

- **Mermaid is a review aid, not a serialisation format.** It shows nodes, edges
  and labels; it cannot round-trip full field values, so the canonical form must
  stay text (or a text projection of the stored row). Rendering must therefore be
  **provably lossy-and-labelled** rather than quietly dropping a field.
- **It is cheap and it pays immediately**: `graph TD` output is diffable, so a
  flow change becomes a reviewable diff, and a reviewer can see the graph without
  running anything. Nothing in the tree renders this today.
- The check worth building alongside it: **render → re-parse → compare**. A
  renderer that silently drops an edge is worse than none, and this is exactly
  the class of bug that only a round-trip test catches.

### "Could we ship a pre-filled redb with common flows?"

**Yes to the goal, no to shipping the `.redb` file — ship the text plus a
builder.** Four reasons, in order of force:

1. **A `.redb` in the repo makes the content unauditable.** The first two
   suggestions in this section are *about* reviewability; committing a binary
   store gives that up in the same breath.
2. **Format versioning.** The engine's store carries a `schema_version` and needs
   `migrate_v1_to_v2` for exactly this reason. A pre-filled store shipped as a
   binary means every format change breaks every installation, with no migration
   path a human can read.
3. **It collides with the single-writer rule.** The store admits one writer
   (`DatabaseAlreadyOpen` enforces it, by design). A pre-filled store that
   anything must update is the wrong shape; a text source that *builds* the store
   is not.
4. **The precedent already exists**: initialisation is a command that *creates*
   the store, not a file that is copied. Starter flows should be the same thing —
   `laya flows import starter/` → text in, store out.

Same bytes at the end, reviewable and migratable on the way.

### The question underneath the question

"Hardcoded, JSON, or redb?" is really **two** questions with different answers:

1. **Who authors the change?** A human editing a threshold while reading a
   dashboard, or a measurement writing a number back?
2. **What has to be true of the store?** Reviewable and diffable, or atomic and
   queryable?

| | text file (TOML/JSON) | redb |
|---|---|---|
| hand-editable | **yes** | no — binary |
| diffable / reviewable in a PR | **yes** | no |
| an agent can author it | **yes** | only through code |
| atomic multi-key update | no | **yes** |
| history of what was in force | only if the files are kept | **yes, naturally** |
| concurrent reader + writer | fine | **a held reader blocks a writer** (measured; why the server never holds the ledger open) |

**Recommendation — split by author, not by format:**

- **Human-authored policy → text, and it is the source of truth.** Thresholds,
  the L7 questions, near-tie margins. These are *arguments about intent*: they
  need to be read, argued with, diffed. Wire `from_toml` to a real path (env →
  file → defaults) and **print the effective values at startup**, as the
  read-scope boundary already does.
- **Machine-measured values → redb.** Anything `calibrate` or a measurement
  produces. A machine writes these and no human hand-edits them, so
  reviewability is not lost — *it was never available*.
- **A fingerprint of the effective policy goes into every ledger record.** This
  is what makes it more than plumbing. Today a decision records its confidence
  but **not the thresholds in force when it was made**, so changing a threshold
  silently invalidates every prior outcome — and `calibrate`, whose whole job is
  comparing confidence against correctness, cannot tell which policy produced
  which correctness. With the fingerprint, a threshold change becomes an
  experiment with a before and an after.

**And the failure mode to design against now:** two sources of truth that drift.
Pick one as authoritative (text), make redb's copy **derived and stamped with the
source hash**, and refuse to start when they disagree — the same fail-closed
posture as the Laya-readiness preflight.

### What the pushback changed

- **Load time**: redb cannot speed it and should not be asked to; the honest use
  is verifying the checkpoint it expects. The speed win is P4, not storage.
- **Mermaid export is promoted from "nice" to "mandatory, and FIRST"** — a binary
  store obliges a text renderer, and the renderer needs a round-trip test.
- **Starter flows: ship text + an import command, not a `.redb`** — for
  auditability, migration, and the single-writer rule.

---

## 3. Should we have our own graph specification?

### Round 1 answered the wrong question

Round 1 read the suggestion as "copy a known project's graph format" and argued
against it. **That was a misreading, and the correction is accepted: defining
*our own* specification — with the fields our decisions actually need — is
sound.** The useful version of the question is therefore not *"copy or not"* but
**"what does our spec contain, and who executes it?"**

### What our spec has to carry

Two families of field, and the second is the whole reason we would have one:

| family | fields | whose vocabulary |
|---|---|---|
| execution | node id, `depends_on`, retry, timeout, `for_each`, `quorum`, `on_fail`, subgraph | already exists in the engine's model |
| **decision** | `laya_call` (tool + arguments), the `band`/`label` predicate, the payload contract, `escalate_on_near_tie` | **ours — and the second is what a graph language cannot express for itself** |

So the spec is **their execution fields plus our decision fields**, and the
refactor is real but bounded: it is an *extension* of a shape that exists, not an
invention.

### The load-bearing constraint, unchanged from round 1

The graph's expression language supports **only object member access** — no
arrays, no functions, no comparisons — and `route` matches by **exact
`serde_json::Value` equality**, with no ranges. So a `transform` node **cannot**
compute a band and a `route` **cannot** express one.

That is why `route_payload` emits a **discrete, exactly-matchable band** instead
of a float, and it is the reason the decision fields have to exist at all. It is
a constraint of the language, not a preference, and it survives every
reformulation of this question.

### The fact that decides the design

**There is no path to hand the executor an externally-authored graph today.**
`serde_json::from_str::<Dag>` does exist, but it parses the `dag_json` **stored
inside a run** (it is how a resume re-reads its own graph). There is no
`--dag <file>` and no import path. So:

| option | what it means | verdict |
|---|---|---|
| **(a) our spec compiles to the engine's schema** | exporter emits DAG JSON satisfying the engine's `validate()`, executed by **the existing scheduler** | **WITHDRAWN (see below)** — it makes the other project a **dependency**, which the standalone rule forbids |
| (b) our spec + our own executor | fully ours, end to end | legitimate, and it is §4 — but it is a scheduler, so decide it as a scheduler |
| (c) our spec, nobody runs it | a format with no executor | a drawing, not a system |

**CORRECTION (2026-09-30, user: *"no dependency!!!! our implementation is
standalone!!! i am taking about absorbing features!"*).** Option (a) is **withdrawn**,
and the reason it was wrong is worth stating precisely: it would have made another
project a **runtime dependency**, which this project forbids; it also proposed a change
*to that other project* to accept our file. Both violate the standing rule. The earlier
argument for (a) — "the executor is already tested" — was arguing for convenience at the
cost of the project's defining constraint, which is the wrong trade.

**The feature, named precisely: execute ANY generic DAG.** That is the absorbed
capability, and it is the core one — not a scheduler we happen to need for convenience.
Everything else is a detail of it.

**And the consequence is the design principle: an SDD+TDD flow is not a feature of the
engine, it is a GRAPH the engine can run.** The engine must not know what "spec" or
"test" means. An SDD+TDD pipeline, a build-and-verify flow, a research fan-out, a
migration — all are the same thing to the executor: nodes, edges, ports, and calls. That
is exactly the flexibility the original question asked for ("all the flows could be
represented in a graph"), and it is only achievable if the engine stays generic. The
moment SDD is built in, the engine has a use case baked into it and the flexibility is
gone.

So the absorbed capability is deliberately **narrow**: a generic graph executor.
**SDD+TDD ships as a flow in the starter library** (§2), written in our spec, compiled
by our compiler, rendered by our mermaid renderer — expressible, not special-cased.

Why this sits naturally with us: **Laya is made for exactly this role.** The graph owns
every edge; Laya supplies the *values* the edges are chosen on. An engine with no
predicate gets one by calling us, and it never needs to know what the question meant.

The pieces, all **in our own crates**, depending on nothing outside:

| feature to absorb | why we need it | our own version |
|---|---|---|
| graph model (nodes, edges, ports) | the authoring surface | our spec (§3 above), JSON |
| topological order + ready queue + run loop | executing a flow | ours |
| node executors: shell, **MCP call** | run a step; call the judge | ours, calling our own `laya-mcp` |
| **run persistence + resume** | a flow that survives a restart | **redb — already a dependency here** |
| retries, timeouts, budgets | not failing on the first error | ours |
| mermaid renderer | the human view (§2) | ours |
| **decision nodes (the band predicate)** | the differentiator | ours — see below |

Note the asymmetry: the first six are *absorbed because we need them*; the last is the
one thing the other side structurally cannot express, which is why it is ours already.

**Deliberately NOT absorbed:** A2A, the SDD/spec driver, the web UI, the queue daemon,
workflow templating, spec front-matter parsing, and the `pi` backend integration. Those
serve that project's use cases, not ours — and note the shape of the refusal: the SDD
*driver* is not absorbed while an SDD *flow* is expressible. That is the generic-engine
principle stated as a decision, rather than as an aspiration.

A useful side effect: **redb is already a dependency for the ledger**, so persisting runs
and node records reuses the store we already own instead of adding a new one.

### What the pushback changed

- **Accepted**: the goal is *our* spec, not a copy. Round 1's framing was wrong.
- **The spec is now specified**: execution fields + decision fields, and the
  decision fields are the ones the graph language structurally cannot express.
- **A new decisive fact**: the executor cannot be fed a graph from outside today,
  so option (a) carries one small engine-side change and that is the honest cost.

---

## 4. Implementing a scheduler — feasible? And "laya as judge" as the goal

### Round 1 used the wrong test

Round 1 argued "we cannot supersede it" with the test *same inputs, same outputs,
fewer moving parts* — we do not accept their inputs, so we fail. **That test is
the right test for *superseding* and the wrong test for *replacing***, which is
what was actually being proposed. Replacing does not have to accept their inputs
at all. So: **yes, it is feasible**, and the honest question is what it costs and
what it gives up.

### The cost, sized honestly

The engine is ~25 283 src LOC, but that is **not** the size of a scheduler. Most
of it is not scheduling: retries, budgets, checkpointing, crash recovery, run
persistence, the UI, RPC, A2A, an MCP client, a queue daemon, an SDD driver, and
workflow templates. A minimum viable scheduler is much smaller:

```
topological order over depends_on      an MCP call node                 resume from a checkpoint
a ready-queue of runnable nodes        a shell node                     record node results
run each node to completion            persist runs + events            report a terminal state
```

A defensible estimate is **1.5–3k LOC** — not 25k. So feasibility is not the
objection, and saying "25k LOC" as if it were the cost would be misleading.

### The real objection, and it is a data objection

**Scheduling is easy. Recovery is not, and the *ledger split* is worse than
either.**

1. **Two schedulers means two run histories and two ledgers.** A decision's record
   and the run that consumed it would live in different stores, so *"which
   decisions did run X make, and were they right?"* has **no single query**. That
   is the strongest argument, and it is about the shape of the data, not the size
   of the code. It is also the one that gets harder to fix later, because history
   cannot be backfilled.
2. **Crash recovery is the hard-won part.** A scheduler that has never crashed in
   production has untested recovery, and recovery bugs appear *only* in
   production. The engine's 68 integration test files are largely about this.
3. **Two things that can drift** — the same failure mode as the config question.
   Two schedulers will disagree about node semantics within a release or two.

### Where the pushback is right, and what follows

**"Laya acting as a judge on our graphs is a winning choice" — agreed, and it is
the architectural insight of this whole document.** The reason it settles the
question is this:

> **The judge role does not depend on who executes.** Deciding *what is true* is
> independent of deciding *when work runs*.

Which means the executor is **swappable**, and the whole §4 decision becomes
**deferrable** — provided the interface is drawn in the right place now:

```
        our spec (authoring)                    <- ours, §3
              │
              ▼
        compile → executor's schema             <- the seam: stable, small
              │
              ├── executor A: the existing scheduler      (today)
              └── executor B: ours, later, if ever        (possible, not now)
              ▲
              │  every node that needs a judgement calls:
        laya-mcp  (the judge — 7 tools, over MCP stdio)  <- never changes
```

**So the recommendation is not "don't build a scheduler". It is: do the cheap
things first, and let the pilot decide.**

1. **Define our spec and its compiler** (§3) — including the mermaid renderer
   (§2). This is the part that is ours regardless of who executes.
2. **Run ONE real flow through the existing executor with `laya-mcp` as a judge.**
   The `mcp_call` node type exists, so this is a wiring exercise, not a build.
3. **Then decide the scheduler with real evidence**: if the seam holds and the
   only remaining pain is that the executor is not ours, (b) becomes a normal
   engineering choice with a known interface — and by then the two-ledger
   objection is *measured* rather than predicted, which is the honest way to
   settle it.

Committing to a scheduler now buys flexibility we cannot yet demonstrate a need
for, at the price of the one thing that is hard to recover: a single queryable
history.

### Absorbing features vs superseding: what each one actually claims

These are different claims and only one of them is ours.

**Absorption (what we are doing): we gain the capability, standalone.**
By building the graph runner in our own crates and depending on nothing, `laya-rs`
becomes able to run a flow end to end. Two consequences follow, and they are the ones
that matter:

- **The two-ledger objection dissolves.** It only bites if we claim to take over another
  system's history. We are not: we have **one** store, ours, and nothing to migrate. The
  earlier §4 objection to building a scheduler was an objection to *supersession*,
  imported into a decision that never required it. Corrected: under absorption, the
  cost of the scheduler is simply its own cost.
- **Nothing outside becomes a dependency**, which is the constraint that decided (a)
  above.

**Supersession (not our goal): the other system can be retired.**
That is a *migration* claim and needs three things we are not attempting: take over its
existing **function**, accept its existing **data** (run history, resumption state, node
records, events), and let it be **retired with no run lost**. Absorbing the core features
gets us a working runner; it does not retire anything, and it does not need to.

**So: does this supersede it? No — and under the standalone rule it should not try.**
What it supersedes is the *need* for it **in our flows**. Asking whether the other
system can be retired is a question about *its* users, and is outside this project's
scope by design.

**The residual risk, stated honestly rather than deferred:** crash recovery is the
hardest thing being absorbed. A scheduler that has never crashed in production has
untested recovery, and that is now ours to earn rather than borrow. That is the real
price of refusing the dependency, and it is a price worth paying — the alternative was
coupling the judge to an executor when the judge does not care who executes.

### What the pushback changed

- **Feasibility conceded**: it is feasible, and round 1's "25 283 LOC" overstated
  the cost by conflating a scheduler with the engine around it (real estimate
  1.5–3k LOC).
- **The objection is restated as data, not size**: two ledgers, and history
  cannot be backfilled.
- **The winning insight recorded**: judge and executor are independent, so the
  decision is deferrable if the seam is drawn now.
- **The recommendation becomes a pilot**, ordered so that every step is useful
  even if the scheduler is never built.

---

## 5. Recommended actions, ranked

Nothing here is implemented. Ordered by value per unit of risk, and revised so
that each step is useful on its own:

| # | action | why here | cost |
|---|---|---|---|
| 1 | **Reach `Policy::from_toml` from a real path** (env → file → defaults; effective values printed) | written, tested, unreachable; unblocks §2 entirely | small |
| 2 | **Output-partitioned parallel matmul (C)** + a bit-exactness test against the scalar reference | the speed case, and it cannot be retrofitted cleanly once a kernel exists | medium |
| 3 | **Policy fingerprint in the ledger record** | makes `calibrate` meaningful across a threshold change; cheap now, impossible to backfill | small |
| 4 | **Our graph spec + mermaid renderer** (§3) and **the absorbed scheduler** | the graph capability itself, standalone; §2's reviewability requirement | medium |
| 5 | **Pool with per-lane bounded channels (A + D)**, worker count reported, feature-gated | ordering, backpressure, fault isolation; rewrites the forbidding test honestly | medium |
| 6 | **One real flow through the existing executor with `laya-mcp` as judge** | converts §3/§4 from argument into evidence | small |
| 7 | Machine-measured values in redb + source-hash drift check | completes §2 once there is something measured | medium |
| 8 | Egress pub/sub from the ledger (at-least-once, cursor-based) | fills the missing react-to-a-band-change path | medium |

**No longer deferred.** Building the scheduler is part of the absorption (§3), because
the alternative — handing a graph to the other project — is a dependency. Ordered after
the spec and renderer, and before the pool, because a scheduler with an empty library of
flows has nothing to run.

## 6. Open questions (deliberately unanswered)

- **Cancellation.** Nothing can cancel an in-flight call. With a pool this stops
  being theoretical: a `Gate` request cannot preempt anything, so priority only
  orders work, it never interrupts it.
- **Does `calibrate` change thresholds automatically, or only recommend?** The
  ledger makes both possible; only one is safe, and it is not obvious which.
- **Text-authoritative config vs redb-authoritative** — I recommend text, but the
  argument assumes a human is the editor. If an agent becomes the primary author,
  the reviewability argument weakens.
- **Bit-exactness across machines.** C guarantees bit-exactness *within* a
  machine. Two CPUs may differ on identical input; NF3's determinism promise
  should be read as per-machine until that is measured.
- **How much of the decision field set belongs in the spec?** Too little and the
  spec cannot express the routing; too much and it becomes a programming language
  with a validator nobody wrote.
