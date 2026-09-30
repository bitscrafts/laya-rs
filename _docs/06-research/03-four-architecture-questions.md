# 03 — Four architecture questions (discussion, nothing implemented)

**Date:** 2026-09-30 · **Status:** RESEARCH — a conversation record, no code changed.
**Subject:** four questions asked together, because they are one question wearing four hats:
*where does authority live, and what is data versus what is code?*

| # | question | short answer |
|---|---|---|
| 1 | Safe multithreading is a must | **Agreed — and it is safe, if you parallelise the OUTPUT, not the reduction.** I was wrong to frame this as single-threaded. Corrected below. |
| 2 | Config: hardcoded? JSON? redb? | **Neither, alone. Split by WHO authors the change:** hand-authored → text, machine-measured → redb. Also: today `from_toml` exists and nothing calls it. |
| 3 | Make our app a Laya-based graph runner? | **No — that is the one thing that would make it worse.** Its precise niche is the *predicate a graph cannot compute for itself*. |
| 4 | Can we supersede the workspace's DAG engine? | **No, and it is not close.** But we can supply the one thing it structurally lacks. |

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

## 1. Safe multithreading is a must — and I had the reason backwards

I previously wrote that the head should be single-threaded because "parallel
reduction changes floating-point summation order". **That is true of one
parallelisation strategy and false of another, and I collapsed the three.**

There are three places concurrency can go, and they have nothing in common:

| site | what is split | determinism | verdict |
|---|---|---|---|
| **A. across calls** | two independent decisions at once | untouched — no call's arithmetic changes | **safe, and the real win** |
| **B. split-K** (split the *reduction*) | one dot product's summation cut across threads | **broken** — partial sums combine in a different order each run | **must not** |
| **C. output-partitioned** (split M/N) | the *output* rows; each element's dot product stays whole, in one thread, in one order | **bit-identical to scalar** | **safe, and the other real win** |

I was describing **B**. The design should do **A and C**, which is why the
conclusion reverses: multithreading is a must **and** achievable without
sacrificing the determinism the ledger depends on.

**C is the important one**, because it is where the speed is. A matmul's output
element `(i,j)` is one dot product; give row `i` entirely to one worker and the
summation order is *identical* to the serial version — same operations, same
sequence, same rounding. Determinism is preserved by construction, not by
testing. This is also what the real BLAS libraries partition for.

### It needs no `unsafe` and no dependency

The crate forbids `unsafe`, which would normally kill a hand-written parallel
matmul. It does not, because `std::thread::scope` plus `chunks_mut` is **safe
Rust**: the borrow checker proves the disjoint output slices do not alias, and
the scope guarantees the threads join. So the choice was never
"parallel *or* no-unsafe" — it was "parallel *or* split-K".

```rust
// The shape of it: each output row belongs to exactly one worker, so the
// arithmetic inside each row is byte-for-byte what the serial version did.
std::thread::scope(|s| {
    for (out_row, w_row) in out.rows_mut().zip(weights.rows()) {
        s.spawn(move || compute_row(out_row, w_row, input));
    }
});
```

### The one remaining trap, stated so it is not discovered later

**FMA changes results too.** `a*b + c` computed with a fused multiply-add has a
single rounding step; computed separately it has two. So a "vectorised" kernel
that fuses is *not* bit-identical to the scalar reference even though its
summation order is perfect. Any SIMD/FMA work must therefore either (a) be
tested against the scalar reference for exactness, or (b) be declared as a
separate *numerical* mode. Silently enabling it is the same defect class as
split-K, arriving through a different door.

**Recommended design (supersedes what spec 03 says today):**

- the forward pass stays a **pure function** for a single call — that part of the old
  answer stands, and it is what makes C safe;
- **C ships by default**: output-partitioned rows via `std::thread::scope`,
  bounded by an explicit worker count (not `num_cpus` silently);
- **A is the caller's layer**: a fixed-size pool *in front of* the pure function —
  this is what removes the sidecar's serialisation, and it never touches one call's
  arithmetic;
- **B is forbidden**, and a test must pin bit-exactness against the scalar
  reference so it cannot creep in;
- the worker count is **reported**, and the default is 1 until the exactness test
  passes on the target machine — fail-safe, matching this project's readiness rule.

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

### The question underneath the question

"Hardcoded, JSON, or redb?" is really **two** questions, and they have different
answers:

1. **Who authors the change?** A human editing a threshold while reading a
   dashboard, or a measurement writing a number back?
2. **What has to be true of the store?** Reviewable and diffable, or atomic and
   queryable?

redb answers (2) one way and (1) the other, and that is the whole finding:

| | text file (TOML/JSON) | redb |
|---|---|---|
| hand-editable | **yes** | no — binary format |
| diffable / reviewable in a PR | **yes** | no |
| an agent can author it | **yes** | only through code |
| atomic multi-key update | no | **yes** |
| history / provenance of what was in force | only if you keep the files | **yes, naturally** |
| concurrent readers with a writer | fine | **a held reader blocks a writer** (already measured, and the reason the server never holds the ledger open) |

**Recommendation — split by author, not by format:**

- **Human-authored policy → text, and it is the source of truth.** Thresholds,
  the L7 questions, near-tie margins. These are *arguments about intent*: they
  need to be read, argued with, and diffed. This is the case `from_toml` was
  written for, and it should be wired to a real path (env → file → defaults) with
  the load path and the effective values **printed at startup**, exactly as the
  read-scope boundary already is.
- **Machine-measured values → redb.** Anything `calibrate` or a measurement
  produces: the ECE, the sample count, the recommended threshold, the date. A
  machine writes these and no human hand-edits them, so reviewability is not
  lost — *it was never available*.
- **A fingerprint of the effective policy goes into every ledger record.** This
  is the piece that makes the whole thing more than plumbing: today a decision is
  recorded with its confidence, but not with **the thresholds that were in force
  when it was made**. Without that, changing a threshold silently invalidates
  every prior outcome — and `calibrate`, whose entire job is comparing
  confidence against correctness, cannot tell which policy produced which
  correctness. With it, a threshold change becomes an experiment with a before
  and an after.

**So: redb for the numbers that are measured, text for the numbers that are
argued, and a hash of both in the ledger.** Not one store for everything — the
two kinds of number have genuinely different lifespans and owners.

**And the failure mode to design against now:** two sources of truth that drift.
If the file says 0.90 and redb says 0.95, which is in force? Pick one as
authoritative (text), make redb's copy *derived and stamped with the source
hash*, and refuse to start when the hashes disagree — the same fail-closed
posture as the Laya-readiness preflight.

---

## 3. Should we become a Laya-based graph runner?

The premise is fair: the video's subject *is* rules following JSON. But the
reconnaissance changes the answer, because **the workspace already has a graph
runner, and it already has a node type that calls an MCP server over stdio.**

```
pidag, measured:
  src/core/dag.rs      NodeType { Llm, Shell, Quorum }
  Node fields           for_each · quorum · subgraph · retry · on_fail
                        · verify · timeout · budget · after · mcp_call
  McpCallConfig         server: "stdio://path/to/bin" | "http://..."
                        transport: "http" | "stdio"
                        tool + arguments (templated)
  type_dispatch         a node with `mcp_call` set delegates to McpCallWorker
```

So a graph node **can already call `laya-mcp` over stdio today.** Building a
graph runner inside `laya-rs` would not add that capability — it would add a
*second, incompatible* one, and the cost is concrete: node scheduling, retries,
budgets, checkpointing, crash recovery and persistence are ~25k LOC in the thing
that already does them.

### Where the real gap is — and it is exactly ours

Here is the load-bearing finding, and it is a **constraint of the graph's own
language**, not a preference:

> the expression language supports **only object member access** — no arrays, no
> functions, no comparisons. A `route` matches by **exact `serde_json::Value`
> equality**, and there are **no ranges**. A `transform` node therefore *cannot*
> compute a band, and a `route` *cannot* express one.

So a graph runner can route on `"bug_in_code"` but **it cannot decide that the
answer is `"bug_in_code"`**. It has no predicate over a structured value, no
comparison, no way to turn a number into a decision — that is why our
`route_payload` emits a **band** (a discrete, exactly-matchable string) instead
of a float.

**That is the niche, and it is precise: `laya-rs` is the predicate provider for a
graph whose own language is deliberately too weak to express one.** Not a runner
— the thing a runner calls when it needs a judgement it cannot compute.

### Ranked recommendation

1. **(Recommended) Stay a decision layer; let graphs drive us.** Publish
   decisions as exactly the discrete values a `route` can match (already done in
   `route_payload`), and wire a `mcp_call` node → `laya-mcp`. The "flows are
   graphs" goal is *already satisfied* — the flows live in the runner.
2. **(If flexibility is the actual pain) extend the runner, do not fork it.** A
   `route`-over-payload primitive plus the existing MCP node gives graph-authored
   flexible flows with one implementation, one ledger, one place to fix a bug.
3. **(Not recommended) Build a graph runner in `laya-rs`.** It would duplicate
   ~25k LOC, split the decision ledger away from the runs that consumed the
   decisions, and put scheduling inside the crate whose value is being small,
   deterministic and checkable.

Note the coupling with §1: option 1 puts `laya-mcp` on a runner's critical path,
where the serial sidecar's 173–436 ms per call is a real cost. **The pool from
§1 is what makes this architecture viable**, which is the useful sense in which
these four questions are one question.

---

## 4. Could we supersede the workspace's DAG engine?

**No — and the comparison is not close enough to be interesting.** Measured,
both trees, same day:

| | the DAG engine | `laya-rs` |
|---|---|---|
| src LOC | **25 283** | 11 652 (8 crates) |
| `#[test]` markers | 116 | **284** |
| integration test files | **68** | — (in-crate) |
| scheduler / retries / budgets | **yes** | no |
| checkpoint + crash recovery | **yes** | no |
| run persistence | **yes** (redb) | a *decision* ledger only |
| UI, queue daemon, RPC, A2A | **yes** | no |
| MCP server | yes | **yes** (7 tools) |
| MCP client (stdio **and** http) | **yes** | no |
| expression / condition language | **none at all** | `route` payload contract |

The shapes are different, not unequal: the engine is a **scheduler** (it decides
*when* and *whether* work runs), we are a **judge** (we decide *what is true*).
Superseding would mean re-implementing scheduling, persistence, recovery and a UI
in the crate that currently has one dependency-free 106 MB forward pass — to end
up with a worse scheduler and a worse-documented judge.

**The test for "supersede" worth applying, now and later:** *same inputs, same
outputs, fewer moving parts.* We cannot pass it, because we do not accept the
engine's inputs — DAG JSON, run history, budgets, resumption state. A component
that cannot accept the thing's inputs has not superseded it; it has described a
different thing.

**What we can honestly claim instead:** the engine's nodes can be conditioned
only on *terminal state* (its `on_fail` edges) or matched by **placeholder text**
in a prompt. There is no predicate over a **structured value**. That is a real,
bounded, defensible gap — and we fill it. **Supply, do not supersede** — the same
conclusion as §3, from the opposite direction, which is mild evidence it is the
right one.

---

## 5. Recommended actions, ranked

Nothing here is implemented. Asked in order of value per unit of risk:

| # | action | why first | cost |
|---|---|---|---|
| 1 | **Reach `Policy::from_toml` from a real path** (env → file → defaults, effective values printed) | it is written, tested, and unreachable — the cheapest real capability in the tree; it also unblocks §2 entirely | small |
| 2 | **Output-partitioned parallel matmul** (site C) with a bit-exactness test against the scalar reference | the whole speed case, and it *cannot* be retrofitted cleanly once a kernel is written | medium |
| 3 | **Policy fingerprint in the ledger record** | makes `calibrate` meaningful across a threshold change; cheap now, impossible to backfill later | small |
| 4 | **The call pool** (site A), feature-gated, worker count reported | removes the sidecar's serialisation; the precondition for §3 option 1 | medium |
| 5 | **A `mcp_call` node → `laya-mcp`** proof, end to end | demonstrates the integration claim in §3 instead of asserting it | small |
| 6 | Machine-measured values in redb + source-hash drift check | completes §2, but only earns its keep once §3 exists | medium |

## 6. Open questions (deliberately unanswered)

- **What is the unit of a pool worker** — a whole `ask`, or one question within
  it? The head is a single forward pass per question, so call-level parallelism
  may leave the machine idle during one large `ask`.
- **Does `calibrate` change thresholds automatically, or only recommend?** The
  ledger makes both possible; only one is safe, and it is not obvious which.
- **Text-authoritative config versus redb-authoritative** — I recommend text, but
  the argument assumes a human is the one editing. If an agent becomes the
  primary editor, the reviewability argument weakens.
- **Bit-exactness across machines.** C guarantees bit-exactness *within* a
  machine. Two different CPUs may differ on the same input; nothing here
  addresses that, and NF3's determinism promise should be read as per-machine
  until it is measured.
