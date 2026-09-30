# 03 — Four architecture questions

**Date:** 2026-09-30 · **Status:** RESEARCH — discussion only, nothing implemented.
**Question:** where does authority live in this project, and what is *data* versus what is
*code*?

Four questions were asked together and turn out to be one question wearing four hats.
Every claim below is either **measured** or explicitly labelled a **judgement**; where a
round-1 conclusion was wrong it is **kept and corrected** in the [corrections
ledger](#corrections-ledger) rather than deleted, because the disagreement is the finding.

| # | question | answer |
|---|---|---|
| 1 | Safe multithreading; does the queue fix it; other structures; pub/sub? | **Yes — and parallelism is safe if you split the OUTPUT, never the reduction.** The existing priority queue is the pool's admission control. Pub/sub: never for ingest, yes for egress. |
| 2 | Config hardcoded, JSON, or redb? Faster loads? Export flows? Ship a filled redb? | **Split by author**: text for human-authored thresholds, redb for machine-measured values. **No storage format speeds the load** — ~24 s is the Python stack, not the 106 MB of weights. Ship flows as **text + an import command**. |
| 3 | Our own graph specification? | **Yes — and the feature is generic DAG execution.** The engine must not know what a "spec" or "test" is. Absorb it **standalone**. |
| 4 | Feasible to implement a scheduler? Supersede the other engine? | **Feasible** (it is ~1.5–3k LOC, not 25k). **Absorbing is not superseding** — and supersession is not our goal. |

---

## 0. The measurement everything rests on

Read from the real checkpoint's tensor shapes (`laya-head.safetensors`), not from the
config and not from the paper.

```
d_model 1024 · n_head 16 · head_max_len 192 · 2 layers · d_ff 4096

  in_proj_weight  [3072, 1024]   q,k,v fused  = 3·d × d
  out_proj.weight [1024, 1024]                =   d × d
  linear1.weight  [4096, 1024]   d_ff × d
  linear2.weight  [1024, 4096]   d × d_ff
```

Per layer `(4·d² + 2·d·d_ff)·n` = 2.42 G-MAC, so **two layers = 4.98 G-MAC ≈ 9.97 GFLOP per
call** at the real `n = 192`.

At a pessimistic scalar 2 GFLOP/s/core that is **≈ 4 983 ms on one core**, 1 246 ms on
four, **623 ms on eight**. The deployed Python sidecar answers in 173–436 ms, so the port
must be **both** parallel and vectorised merely to match what it replaces. This number is
the reason §1 is not an optimization discussion.

---

## 1. Parallelism, the queue, and pub/sub

### 1.1 Four places concurrency can go, and only four

The distinction that matters is **what gets split**, because determinism depends on it:

| site | what is split | determinism | verdict |
|---|---|---|---|
| **A. across calls** | two independent decisions | untouched — no call's arithmetic changes | **safe** — the queue's job |
| **B. split-K** | one dot product's *summation* | **reordered every run** | **forbidden** |
| **C. output-partitioned** | a matmul's *output rows* | **bit-identical to scalar** | **safe** — the kernel's job |
| **D. intra-call, independent matmuls** | different projections of one layer | **bit-identical** — no shared reduction | **safe**, and easy to miss |

**C is where the speed is.** A matmul's output element `(i,j)` is one dot product; give row
`i` entirely to one worker and the summation order is *identical* to the serial version —
determinism by construction, not by testing.

**And it needs no `unsafe`.** `std::thread::scope` plus `chunks_mut` is safe Rust: the
borrow checker proves the disjoint slices do not alias and the scope guarantees the join.
So the choice was never *parallel or no-unsafe*; it was *parallel or split-K*.

**D is the easy addition.** A layer's projections are separate matmuls feeding separate
outputs, so they parallelise for exactly the reason C does — and it costs almost nothing
once C exists, because it is the same proof at a coarser grain.

**The one remaining trap, stated so it is not discovered later: FMA changes results too.**
`a*b + c` fused has one rounding step; separate has two. So a vectorised kernel is *not*
bit-identical to the scalar reference even with perfect summation order. Any SIMD/FMA work
must either be exactness-tested or declared as a separate *numerical* mode. Silently
enabling it is split-K arriving through a different door.

**Recommended design:** the forward pass stays a **pure function** for one call (which is
what makes C safe); **C ships by default** via `thread::scope`, with an explicit worker
count that is **reported**; **D** follows; **A** is the caller's layer; **B is forbidden**
and pinned by a bit-exactness test. Default worker count is 1 until the exactness test
passes on the target machine — fail-safe, matching the project's readiness rule.

### 1.2 The priority queue is the pool's admission control

`laya-core::queue` already has the right shape and it was written for exactly this:

```
Priority::Gate  >  Interactive  >  Background      (served highest first)
Priority::shedable()                               the classes droppable under pressure
PriorityQueue::new(capacity)                       bounded — refuses, never grows
EnqueueError::Full                                 fail closed, not queue-forever
```

**But the queue cannot fix split-K, and the two cannot touch.** Split-K is how *one dot
product's summation* is partitioned; a queue is *which request runs next*. A queue has no
access to the inside of a matmul, so no ordering can repair an arithmetic property.

**Two real obstacles to using it, neither a wiring exercise:**

1. `PriorityQueue<T>` is a **plain struct** — `lanes: [VecDeque<T>; 3]`, no `Mutex`, no
   `Condvar`, no wake mechanism.
2. The server has **no concurrency to prioritise**: it runs
   `tokio::runtime::Builder::new_current_thread()`, and a test in `laya-mcp` **fails the
   build** if `PriorityQueue` appears in production code, with the reason stated: the queue
   *"could never fill and its 'queue is full, fail closed' error could never fire. It read
   as protection and provided none."*

**That test is correct for the current server and becomes the specification for the
change.** Adding the pool is what makes the queue meaningful, so the order is **pool
first, queue second** — and the test is what enforces that order.

### 1.3 The honest problem list — throughput is not on it

| problem | measured | real? |
|---|---|---|
| throughput | 8 concurrent calls complete in **121 ms**; one call is 173–436 ms | **no** — the machine idles for most of each call |
| head-of-line blocking | a `Gate` call behind 8 `Background` calls waits for all of them | **yes** |
| no shedding under load | a bounded queue exists but is unused | **yes** |
| **fault isolation** | one panicking call kills the whole process; the sidecar is single | **yes — the strongest argument, and it is not about speed** |
| cancellation | nothing can cancel an in-flight call | yes, unsolved |

**So the pool is justified by ordering, backpressure and fault isolation — not by speed.**
Justifying it by throughput would be justifying it with a measurement that does not support
it.

### 1.4 Structures, ranked

1. **(Recommended) one bounded channel per lane**, not a shared `Mutex<PriorityQueue>`. The
   same ordering semantics, the runtime owns the wake-up, and backpressure is free — a full
   channel *is* `EnqueueError::Full`. It also moves the queue struct out of the hot path,
   which is what lets the forbidding test be rewritten honestly instead of deleted.
2. `Mutex<PriorityQueue<T>>` — smallest diff, keeps the 16 tests meaningful, correct but
   contended on the one path that exists to remove bottlenecks.
3. Lock-free — **not considered.** No measurement demands it, and a mechanism without a
   measurement is unjustified here.

### 1.5 Pub/sub — right idea, wrong tool for half the job

| direction | shape needed | why |
|---|---|---|
| **ingest** (request → worker) | **work queue** | each request must be handled by **exactly one** worker. A topic gives *at-least-one-of-N*, which is how a decision is computed twice — or zero times |
| **egress** (decision made → whoever cares) | **pub/sub, or a tail-able stream** | many consumers, one event, no ownership |

**Ingest: never pub/sub. Egress: pub/sub, and it is genuinely missing today** — the only
record a decision happened is a ledger append, so anything wanting to react to a band
change must poll.

**One caveat decides the implementation:** in-process pub/sub dies with the process, and
supervision is unsolved. So egress must be **at-least-once from the ledger**, not a memory
bus — a subscriber asks "what happened since cursor N" and gets it after a restart.
Otherwise the notification path loses events exactly when it matters.

---

## 2. Configuration: what to store, and where

### 2.1 What is true today (measured)

Not scattered magic numbers, but **effectively compiled in**:

```
crates/laya-core/src/policy.rs:186   pub fn from_toml(text: &str) -> Result<...>
crates/laya-mcp/src/lib.rs:1129      let policy = laya_core::Policy::default();
```

`from_toml` parses TOML, validates ranges, rejects unknown fields — and **is called by
nothing outside its own tests.** Every production site takes `Policy::default()`. The `toml`
dependency is in the manifest for a parser no binary invokes. This is the project's
recurring shape: *a documented capability no caller can reach.*

### 2.2 Nothing in the storage layer can make loading faster — because storage is not the cost

Measured, and the measurement changes the question. Three separate things get called "the
load":

| what is loaded | size | what it actually costs |
|---|---|---|
| the head's **weights** | 106 MB, 26.5 M params | **1.6 s** warm. Raw read at SATA speeds is **193 ms**; the rest is tensor construction |
| the **Python stack** (torch, transformers, the `laya` package) | ~1.2 GB of torch, 87 `.so` files | **~23 s cold**, **~1.7 s warm** |
| **policy / thresholds** | a few KB | nothing measurable |

**The ~24 s is not the weights and not the config — it is the Python library stack.** The
1.2 GB of `.so` files must be read and relocated, and the only reason it looks affordable at
1.7 s is the page cache; the sidecar's own header records the cold figure as ~23 s. So the
question "can we load the weights faster with sqlite?" has no useful answer, because **the
weights were never the slow part.**

**Would sqlite help? No — and it is the same shape of argument as redb.**

| candidate | verdict |
|---|---|
| **redb** | no — a B-tree store is not a tensor loader; moving float32 weights into it adds a decode step and removes nothing |
| **sqlite** | no — same argument, plus a `BLOB` read *plus* a Python-object materialisation the raw file does not need. It also brings a C library into a crate that forbids `unsafe` and C bindings |
| **mmap the safetensors** | **yes, and only this one works** — the format is already page-aligned and offsets are absolute, so the kernel pages tensors in on demand. It does not make the read *faster*, it makes the read *avoidable* for tensors a given call never touches |

**And this is exactly what the Rust port already does.** `laya-head::safetensors` stores
tensor offsets into the buffer and materialises an `f32` vector only when a caller asks for
one. Since the file is **106 MB** and any single decision reads a fraction of it, mmap is the
right shape and it is already the implementation. It is also worth noting what the *other*
106 MB is: 106 048 536 bytes of payload against 106.1 MB nominal **is** the safetensors
format, and the header — 4 296 bytes for all 36 tensors — is negligible.

**The real load win is P4, and it is not about faster reading — it is about removing the
1.2 GB.** In-process Rust means there is no Python stack to import at all, so the ~23 s
ceases to exist rather than being optimised. The head's own 1.6 s becomes a once-paid
startup cost.

**What redb can still do here is make the load HONEST**, which is a different and real
benefit: store a fingerprint of the checkpoint it expects and verify it before use — the same
fail-closed posture as the readiness preflight. That is worth having and is not about speed.

### 2.3 Flows must be exportable — and the renderer comes FIRST

**This is the strongest single idea in the discussion, and it inverts the design order:**

> **If a flow lives in redb, a text renderer is mandatory, not optional** — because a
> binary store removes the only human-readable view that would otherwise exist.

So: **renderer first, store second.** Storing first and rendering later is how a binary
container becomes unauditable.

- **Mermaid is a review aid, not a serialisation format.** It shows nodes, edges and
  labels; it cannot round-trip every field. So the canonical form must stay text, and the
  render must be **provably lossy-and-labelled** rather than quietly dropping a field.
- **The test that makes it trustworthy: render → re-parse → compare.** A renderer that
  silently drops an edge is worse than none, and only a round-trip test catches it.
- It also pays immediately: `graph TD` output is **diffable**, so a flow change becomes a
  reviewable diff. Nothing in the tree renders a graph today.

**Storage format for flows: JSON, not TOML.** Flows nest (subgraphs, per-node argument
maps), and TOML's syntax is unkind to deep nesting. Text stays canonical; redb is a
**cache** with a source hash.

### 2.4 Starter flows: ship text, not a pre-filled database

Yes to the goal, **no to shipping a `.redb`**. Four reasons in order of force:

1. **A committed `.redb` makes the content unauditable** — the exact property §2.3 exists
   to protect.
2. **Format versioning.** A store carries a `schema_version` and needs a migration path for
   every change; a binary pre-filled store breaks every installation on an upgrade with
   nothing a human can read.
3. **It collides with the single-writer rule.** The store admits one writer
   (`DatabaseAlreadyOpen`), so a pre-filled store that anything must update is the wrong
   shape. A text source that *builds* the store is not.
4. **The precedent exists:** initialisation is a command that *creates* the store, not a
   file that is copied. Starter flows should be the same: `laya flows import starter/`.

### 2.5 The question underneath the question — split by AUTHOR

"Hardcoded, JSON, or redb?" is really two questions:

1. **Who authors the change?** A human editing a threshold, or a measurement writing back?
2. **What must be true of the store?** Reviewable and diffable, or atomic and queryable?

| | text (TOML/JSON) | redb |
|---|---|---|
| hand-editable | **yes** | no — binary |
| diffable / reviewable in a PR | **yes** | no |
| an agent can author it | **yes** | only through code |
| atomic multi-key update | no | **yes** |
| history of what was in force | only if files are kept | **yes, naturally** |
| concurrent reader + writer | fine | **a held reader blocks a writer** (measured) |

**Recommendation:**

- **Human-authored policy → text, and it is the source of truth.** Thresholds, the L7
  questions, near-tie margins. These are *arguments about intent*: they need to be read,
  argued with, diffed. Wire `from_toml` to a real path (env → file → defaults) and **print
  the effective values at startup**, as the read-scope boundary already does.
- **Machine-measured values → redb.** `calibrate`'s output, sample counts, dates. A machine
  writes these and no human hand-edits them, so reviewability is not lost — *it was never
  available*.
- **A fingerprint of the effective policy goes into every ledger record.** This is what
  makes it more than plumbing: today a decision records its confidence but **not the
  thresholds in force when it was made**, so editing a threshold silently invalidates every
  prior outcome — and `calibrate`, whose whole job is comparing confidence against
  correctness, cannot tell which policy produced which correctness. With the fingerprint, a
  threshold change becomes an experiment with a before and an after.

**The failure mode to design against now:** two sources of truth drifting. Pick text as
authoritative, make redb's copy **derived and stamped with the source hash**, and refuse to
start when they disagree — the same fail-closed posture as the readiness preflight.

---

## 3. The graph capability: execute any generic DAG

### 3.1 The feature, named precisely

**Absorbed capability: execute ANY generic DAG.** Not a scheduler needed for convenience —
the core capability itself.

**And the design principle follows from it: an SDD+TDD flow is not a feature of the engine,
it is a GRAPH the engine runs.** The engine must not know what "spec" or "test" means. An
SDD+TDD pipeline, a build-and-verify flow, a research fan-out, a migration — all identical
to the executor: nodes, edges, ports, calls.

That is the only way to get the flexibility originally asked for. **The moment SDD is built
in, the engine has a use case baked into it and the flexibility is gone.** So the absorbed
capability is deliberately **narrow**, and **SDD+TDD ships as a flow in the starter
library** — expressible, not special-cased.

**Why this fits us naturally: the graph owns every edge; Laya supplies the values the edges
are chosen on.** An engine with no predicate gets one by calling us, and never needs to know
what the question meant.

### 3.2 Absorb standalone — no dependency

The pieces, all **in our own crates**, depending on nothing outside:

| piece | note |
|---|---|
| graph model (nodes, edges, ports) | our spec, JSON |
| topological order + ready queue + run loop | ours |
| node executors: shell, **MCP call**, **agent** | ours — the agent node **spawns a process** and speaks its protocol, which is adoptable rather than a library dependency |
| **run persistence + resume** | ours, in **redb — already a dependency for the ledger** |
| retries, timeouts, budgets | ours |
| mermaid renderer | ours (§2.3) |
| **decision nodes (the band predicate)** | ours — the differentiator |

**Deliberately NOT absorbed:** A2A, the SDD *driver*, the web UI, the queue daemon,
workflow templating, spec front-matter parsing. Those serve that project's use cases, not
ours.

**One entry was removed from this refusal list after checking it, and the correction
matters:** a first draft listed "the external agent backend" as *not* absorbed, which
contradicted §3.1 — an SDD+TDD flow needs an **agent node**, so refusing it would leave the
flow unexpressible. Inspecting how the backend is actually reached shows it is an **RPC
transport to a spawned process**, not an in-process library integration. So an agent node is
**ours**, implemented the same way a shell node is: spawn the configured CLI and speak its
protocol. Adoptable without a dependency — which is the distinction the refusal list is
supposed to make.

Note the shape of that refusal, because it *is* the principle: **the SDD driver is not
absorbed while an SDD flow is expressible.**

### 3.3 The load-bearing constraint of a graph language

The expression language of the engine we studied supports **only object member access** —
no arrays, no functions, no comparisons — and its `route` matches by **exact
`serde_json::Value` equality**, with no ranges. So a `transform` node **cannot** compute a
band and a `route` **cannot** express one.

That is why `route_payload` emits a **discrete, exactly-matchable band** rather than a
float. It is a constraint of the language, not a preference, and it is the reason the
decision fields have to exist in a spec at all.

### 3.4 Parallel execution is the defining feature, not an optimization

A DAG engine that runs one node at a time is a sequence with extra steps: the only reason
to express a flow as a graph rather than a shell script is that independent branches run at
once. So it ships in v1.

1. **Fan out the ready set, bounded.** Every node whose dependencies are satisfied is
   runnable; run them concurrently up to a declared limit. Unbounded is not acceptable — a
   16-node fan-out is 16 concurrent calls.

2. **The pool is therefore CO-REQUISITE with the scheduler, not a later optimization.** The
   sidecar is **strictly serial**, so an 8-node fan-out would serialise behind it and the
   engine would be slower than eight shell commands in parallel — the one outcome that makes
   the whole design pointless. **The graph fans out; the pool is what makes the fan-out
   real.** This is the clearest case in the document of the four questions being one.

3. **Determinism must be restated, because parallelism changes what it can promise.** The
   *set* of outcomes stays deterministic for independent branches — that is what
   independence means — but the **interleaving does not**:
   - the ledger must attribute every record to `run_id` + `node_id`, because append order is
     no longer execution order and a consumer reading "the last row" breaks;
   - no replayability claim extends to a parallel run's *ordering*;
   - the store still has exactly **one writer** (the scheduler appends for all concurrent
     runs), so the single-writer rule survives parallelism untouched.

**Failure policy, chosen explicitly rather than defaulted:** if one of five parallel nodes
fails, **fail fast and cancel siblings**, with `allow_failure` and the quorum node as
explicit opt-outs. Draining spends real work on an abandoned branch, and **a partial result
that looks complete is the failure mode this project keeps finding.**

### 3.5 Worktrees: right mechanism, different bottleneck

Tested in this repo (git 2.47.3), not reasoned about:

```
git worktree add <dir> master      -> fatal: 'master' is already used by worktree
git worktree add -b wt/x <dir> ..  -> works; main tree stays 0 dirty
git worktree add <dir2> wt/x       -> fatal: 'wt/x' is already used by worktree
```

**A branch can be checked out in exactly ONE worktree.** So worktrees give **isolation per
writer**, never shared concurrent writes — and the deeper reason is not git's bookkeeping
but that two agents editing one file tree is a race whatever the bookkeeping says.

**They do not mitigate the fan-out that parallel execution needs to speed up.** That fan-out
is the **Laya calls**: in-process reads that touch no repository. A worktree adds isolation
and **zero** concurrency. The bottleneck is the serial sidecar; worktrees multiply
write-targets, not compute. **The pool is still what makes a fan-out real.**

**They do solve a real case** — the moment flows contain **repo-editing** nodes (an SDD+TDD
flow, a build-and-fix loop), parallel execution has a genuine shared-resource problem, and
worktrees are the answer: one worktree on a **fresh branch** per writing node, in a
**sibling** directory, with the **scheduler as the only committer**, reviewing and merging
branch diffs in order.

**Which yields a design axis the executor needs: classify nodes by whether they WRITE.**

| node kind | parallelised how | isolation |
|---|---|---|
| compute / decision (Laya call, MCP call) | straight bounded fan-out | none — read-only |
| **repo-editing** (an agent that edits files) | fan-out **per worktree** | **worktree on its own branch** |
| shell with shared side effects | serialise, or declare the resource | explicit, per-node |

Worktrees cover **the middle row only**, and they make that row's parallelism *safe* rather
than *fast*.

**Traps, carried from field experience:**
- the worktree must be a **sibling**, never nested inside the repo. A path-sandboxed agent
  whose cwd is the worktree resolves the main repo as "outside the working directory", every
  call is rejected, it writes nothing, and the gate then **passes on an unchanged tree** — a
  false green.
- **merge-back is a new failure mode.** Parallel writers produce divergent branches and
  therefore conflicts, and a conflict is a *semantic* problem the scheduler cannot resolve
  alone. This is the honest cost of parallel writers.
- a worktree requires the work to be **in a git repo**; non-repo flows cannot use it.

### 3.6 Composition: subgraphs that expand, with arguments and a hash

- **Expand at load, not nest at run time.** A child's nodes join the parent's ready queue,
  so the child's parallelism is fully available. Nesting a child as one schedulable unit
  would serialise the whole of it, defeating §3.4.
- **Bind arguments explicitly** (`with`), so a child is parameterised rather than copied. A
  child reading the parent's variables implicitly cannot be reused.
- **Hash the child and record the hash in the parent.** A subgraph edited in place silently
  changes the parent's behaviour; the hash makes that visible and refusable. Same principle
  as the policy fingerprint (§2.5).

**And the validation rule that must exist before the feature: cycles are rejected at load.**
A subgraph that includes itself, directly or through a chain, is infinite expansion — it
must fail with a named error, not hang. Same class as the `for_each` and quorum-bound checks
that already exist.

---

## 4. Absorbing features versus superseding

These are different claims, and only one of them is ours.

**Absorption (what we are doing): we gain the capability, standalone.** By building the
graph executor in our own crates and depending on nothing, `laya-rs` can run a flow end to
end. Two consequences follow, and they matter more than the option swap that produced them:

- **The two-ledger objection dissolves.** It only bites when we claim to take over another
  system's history. We are not: we have **one** store, ours, and nothing to migrate. The
  objection was made against *supersession* and imported into a decision that never required
  it.
- **Nothing outside becomes a dependency.**

**Supersession (not our goal): the other system can be retired.** That is a *migration*
claim, and it needs three things we are not attempting: take over its existing **function**,
accept its existing **data** (run history, resumption state, node records, events), and let
it be **retired with no run lost**. Absorbing core features gets us a working executor; it
does not retire anything, and it does not need to.

**So: this does not supersede the other engine — and under the standalone rule it should not
try.** What it supersedes is the *need* for it **in our flows**. Whether it can be retired
is a question about *its* users, and is outside this project's scope by design.

**The residual risk, stated rather than deferred:** crash recovery is the hardest thing
being absorbed. A scheduler that has never crashed in production has untested recovery, and
that is now ours to earn rather than borrow. That is the real price of refusing the
dependency — and it is worth paying, because the alternative was coupling the judge to an
executor when the judge does not care who executes.

---

## 5. Recommended actions, ranked

Nothing here is implemented. Ordered by value per unit of risk, and each step is useful on
its own.

| # | action | why here | cost |
|---|---|---|---|
| 1 | **Reach `Policy::from_toml` from a real path** (env → file → defaults; effective values printed) | written, tested, unreachable; unblocks §2 entirely | small |
| 2 | **Output-partitioned parallel matmul (C)** + bit-exactness test vs the scalar reference | the speed case, and it cannot be retrofitted once a kernel exists | medium |
| 3 | **Policy fingerprint in the ledger record** | makes `calibrate` meaningful across a threshold change; cheap now, impossible to backfill | small |
| 4 | **Our graph spec + mermaid renderer + the executor** (§3), incl. parallel fan-out and composition | the capability itself, standalone | medium |
| 5 | **Pool with per-lane bounded channels (A + D)**, worker count reported, feature-gated | **co-requisite with 4, not after it**: without the pool a node fan-out serialises on the sidecar and the engine is slower than a shell script | medium |
| 6 | **One real flow end to end with `laya-mcp` as judge** | converts §3 and §4 from argument into evidence | small |
| 7 | Machine-measured values in redb + source-hash drift check | completes §2 once there is something measured | medium |
| 8 | Egress pub/sub from the ledger (at-least-once, cursor-based) | fills the missing react-to-a-band-change path | medium |

**Ordering notes.** The pool (5) moves up to land **with** the executor (4), because either
alone is unmeasurable: an executor without a pool serialises its own fan-outs, and a pool
without an executor has nothing to schedule. And `calibrate` stays deliberately late despite
being cheap, because it needs **recorded outcomes** before its number means anything —
the ledger has none yet.

## 6. Open questions (deliberately unanswered)

- **Cancellation.** Nothing can cancel an in-flight call. With a pool this stops being
  theoretical: a `Gate` request cannot preempt anything, so priority orders work but never
  interrupts it.
- **Does `calibrate` change thresholds, or only recommend?** The ledger makes both possible;
  only one is safe, and it is not obvious which.
- **Text-authoritative config versus redb-authoritative.** I recommend text, but the
  argument assumes a human is the editor. If an agent becomes the primary author, the
  reviewability argument weakens.
- **Bit-exactness across machines.** C guarantees bit-exactness *within* a machine; two CPUs
  may differ on identical input. Determinism should be read as per-machine until measured.
- **How much of the decision field set belongs in the spec?** Too little and the spec cannot
  express the routing; too much and it becomes a programming language with a validator
  nobody wrote.
- **Fan-out width.** The bound is a config value, but nothing yet says what it should be —
  and with the pool in place, the right number depends on the head's own worker count.

---

## Where the discussion stands — the settled set

Recorded so a later session does not have to re-derive it. **Agreed and fixed:**

| # | settled | evidence |
|---|---|---|
| 1 | Concurrency by **output-partitioning (C) and independent matmuls (D)**; **split-K (B) forbidden**; FMA must be exactness-tested | §1.1, the four sites |
| 2 | The **pool is co-requisite with the executor**, and justified by ordering / backpressure / fault isolation — **not** throughput | §1.3, 8 calls = 121 ms |
| 3 | The **priority queue is the pool's admission control**, pool before queue, and the `laya-mcp` forbidding test is the gate | §1.2 |
| 4 | Pub/sub **never ingest, yes egress** — at-least-once from the ledger with a cursor | §1.5 |
| 5 | Config **split by author**: text for human-authored, redb for machine-measured, **policy fingerprint in every ledger record** | §2.5 |
| 6 | **No storage format speeds the load**; mmap works and is already the implementation; the ~24 s is the Python stack | §2.2 |
| 7 | Flows: **JSON, text canonical, renderer before store**, starter library as **text + an import command** | §2.3, §2.4 |
| 8 | The capability is **generic DAG execution**; **SDD+TDD is a flow, not a built-in** | §3.1 |
| 9 | **Absorb standalone** — no dependency of any kind, and no change proposed to another project | §3.2 |
| 10 | Parallel node execution is the **defining feature**, bounded fan-out, **fail fast + cancel siblings** | §3.4 |
| 11 | **Composition**: expand at load, explicit arguments, hash the child, **reject cycles at load** | §3.6 |
| 12 | Worktrees isolate **repo-editing** nodes only; nodes are classified by **whether they write** | §3.5 |
| 13 | **Absorbing ≠ superseding** — we gain the capability; nothing outside is displaced | §4 |

**Still open** (§6): cancellation; whether `calibrate` recommends or changes; text- vs
redb-authoritative under an agent editor; bit-exactness across machines; how much of the
decision field set belongs in the spec; and the fan-out width bound.

### The executor we would build is generic by construction

The point worth stating once, plainly, because it is the answer to *"does this make us a
particular system's successor?"*:

> **A generic DAG executor that is fed our own spec cannot be a successor to any specific
> engine, because it is not specific to one.** It runs whatever flow it is given.

That is the design working as intended, not a shortfall. And it is the sharper form of the
§4 answer: **the capability generalises; a supersession claim does not.** Superseding names a
particular system and requires its data; absorbing a generic capability names none.

**What this means for the near term.** The executor is **not yet a spec** — the inventory in
`specs/` runs 01–06 and has no graph-execution spec, and nothing in `crates/` can execute a
graph today. It becomes the seventh, after the pieces that make it usable: the spec format
and renderer (§2.3), and the decision-node contract that is already implemented.

**Two corrections owed to earlier text, now made:** the executor's table includes an
**agent node** (§3.2 — it spawns a process, so it is adoptable), and the load claim is
storage-independent (§2.2).

## Corrections ledger

Seven claims made in earlier rounds were wrong. They are kept here rather than deleted, because in each case
the *disagreement* is the finding. Newest correction last; the table is complete as of
2026-09-30.

| # | claim | what invalidated it | corrected to |
|---|---|---|---|
| 1 | the head costs **7.55 G-MAC ≈ 15.1 GFLOP** per call | recomputed from the checkpoint's real tensor shapes; 7.55 is the value at `seq=300`, and `head_max_len` is **192** | **4.98 G-MAC ≈ 9.97 GFLOP** (§0). The number had units and looked derived, which is why it survived a commit |
| 2 | the head should be **single-threaded**, because parallel reduction reorders summation | true of split-K (B) only; output-partitioning (C) is bit-identical and needs no `unsafe` | threads are **safe and required**; only split-K is forbidden (§1.1) |
| 3 | site C was the only intra-call parallelism worth having | the projections of a layer are independent matmuls with no shared reduction | **axis D** added (§1.1) |
| 4 | the pool is justified by **throughput** | measured: 8 concurrent calls complete in 121 ms, so the machine idles | justified by **ordering, backpressure and fault isolation** (§1.3) |
| 5 | our spec should **compile to the other engine's schema**, executed by its scheduler | it made another project a **runtime dependency**, and proposed changing it — both forbidden by the standalone rule; the argument for it ("the executor is already tested") traded a defining constraint for convenience | **withdrawn**; absorb the executor into our own crates instead (§3.2) |
| 6 | building a scheduler is **deferrable**, and its cost is ~25 283 LOC | 25k is the *engine*, not a scheduler (~1.5–3k); and the deferral assumed the other executor, which is exactly the dependency being refused | the executor is **part of the absorption**, ordered with the pool (§3.2, §5) |

| 7 | the 106 MB checkpoint is what costs ~24 s to load | measured: the weights load in **1.6 s** warm and read in **193 ms** raw; the ~23 s cold figure is the **Python library stack** (1.2 GB of torch, 87 `.so` files) | §2.2 corrected — and the practical consequence is that **no storage format can help**, because storage was never the cost |

**One that was not a correction but a refinement:** the priority queue does not touch
split-K, but the question exposed that site A had been specified with *no mechanism* — and
the queue already in the tree is that mechanism (§1.2).
