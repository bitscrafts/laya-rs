# ANALYSIS — why a generic graph cannot run today

**Written 2026-10-02 by the orchestrator**, from reading the engine source. This is my own
reasoning, offered for adversarial review — not a summary of measurements (those are in
`ENGINE-DEFECTS-2026-10-02.md`). Where I infer rather than measure, I say so.

---

## The claim

**A generic graph cannot run through this engine today, and the reason is not any single defect.
The engine has no data flow at all.**

Every `decide` node's input is a **literal string compiled into the flow file**. Nothing that happens
at run time can change it. Four independent mechanisms would each have to exist for data to reach a
decision, and none of them does:

| mechanism | status | evidence |
|---|---|---|
| template interpolation in `prompt` | **absent** | `lower.rs:104` — `"prompt": node.prompt`, verbatim; the word `prompt` appears once in the file |
| `Flow.inputs` supplied at run time | **absent** | `inputs` is read only in `expand.rs:224` (subflow child validation). `laya graph run` takes no `--input`/`--set` flag. `mcp_client.rs:99` admits it: *"nothing in this tree evaluates a `$.inputs.*` reference yet"* |
| shell output → a decision | **absent** | route upstream must be an `mcp` node (`route_exec.rs:70-76`); and the decide args are rebuilt from `question`/`options`/`prompt` (`mcp_client.rs:105-115`), so an `mcp` node cannot carry its own state either |
| route payload → a downstream decision | **absent** | the route pair is *removed* from `nodes`/`edges` after selection (`route_exec.rs` drain loop); the tail never sees the payload |

So the answer to *"what can a flow express?"* is narrow and exact: **a flow can express decisions
whose answers were already known when the file was written.** Anything that must be read, counted,
computed or received is out of reach.

## Why the `wc` example "works" and proves nothing about `wc`

This is the concrete instance, and it is why the example is dangerous rather than merely limited:

1. `count_words` runs **last** (ms 703, after every decide) — D1.
2. Its output goes nowhere — there is no channel from a shell node to anything.
3. Each `decide` node's `prompt` contains the count as **literal text** (`"The word count in the
   file is 13 words."`).
4. The classification is correct *because of (3)*, entirely independent of `wc`.

The flow validates, renders, runs, routes to different branches in the pair, and its control
collapses when broken — every property a healthy example should have — while the `wc` node is
decoration. **A reviewer checking outputs would approve it.** That is the failure mode this project
keeps paying for: the checks are green and the thing they name is not happening.

## The execution model, as it actually is

The engine runs in **two phases**, and this is the mental model a flow author must hold:

```
PHASE 1 — the drain (route_exec.rs:67)
  while a route node is live:
      find its upstream mcp node
      run the mcp node
      resolve the route, pick a port
      REMOVE BOTH from the graph          <- the payload's only consumer is removed with it
      prune the unselected ports
  (repeat until no route node remains)

PHASE 2 — the tail (sched.rs::run_from, line 110)
  topo_order the REMAINING nodes
  run them with the worker limit and the stop flag
```

Consequences a flow author must know, and none of which is stated in the schema docs:

- **Every decide runs before any shell.** Not "before the shells it depends on" — before **all** of
  them, because phase 1 completes first.
- **`depends_on` orders phase 2 only.** Within phase 1 it is ignored; `find_route_pair` picks route
  nodes in graph order, and a decide's own `depends_on` on a shell cannot be honoured because phase 2
  has not started.
- **A route's payload dies at the route.** Nothing downstream can read it, so a chain of decisions
  where the second depends on the first's answer is *expressible in the file* and impossible at run
  time. `wc-classify-chained` is exactly that chain, and it only appears to work because both
  decisions were pre-answered in their prompts.
- **A `decide` in phase 1 that depends on another `decide` is unordered.** They run in graph order,
  which is a property of the file, not of the data.

So the shape `shell → decide → route` is not a pipeline. It is **a static dispatch table plus a
separate graph of shells that runs afterwards.**

## What would have to change, ranked

*(This is my ranking, for review. Blast radius is my estimate, marked as inference where I have not
traced every caller.)*

**1. Let a `decide` node's state be built from its upstream's output. [highest value / highest scope]**
Without this, no generic graph is possible — every other fix is decoration on a static table.
An expression language is the expensive version. The cheap version: **allow `prompt` to be
`$.upstream.output`**, a single well-defined reference meaning "the output of the node I depend on",
resolved at phase-1 time. That needs (a) a field or a syntax, (b) the drain loop to run the upstream
*before* the decide rather than looking only for `mcp` upstreams, (c) a decision about what happens
when the upstream fails.
Blast radius: `route_exec.rs` (the drain loop, the non-`mcp` refusal), `lower.rs`, possibly
`run_flow`'s phase split. **This subsumes D1**: if the drain loop resolves an upstream first, its
`depends_on` is honoured by construction.

**2. Ask a `noul` from a flow. [high value / low scope]**
One schema field (`question_type: Option<String>`) plus `mcp_client.rs:105-115` reading it. Measured
value: `noul` 15/15 incl. boundary vs the 2-option `choice` 20/22 failing at both boundaries. Cheap
and self-contained — this is the one I would do **first** because it is a day's work and it unlocks
the head's correct arithmetic, and because a 2-option `choice` is a *degraded* ask that will keep
producing boundary errors in every flow anyone writes.

**3. Make `escalate_on_near_tie` work. [medium value / low scope]**
`lower.rs` ignores it; `validate.rs:79` demands its port. Either emit the port (small) or delete the
fields and the validation (smaller, and honest). Leaving a safety field that validates and does
nothing is the worst of the three options — it reads as protection.

**4. Fix the phase split so ordering is uniform. [needed for 1; not useful alone]**
Phase 1/phase 2 is the mechanism behind D1. If (1) is done by making the drain resolve upstreams,
this largely falls out. Done alone it fixes ordering and leaves the data flow absent, which does not
make a generic graph work — so it should not be attempted alone.

## Where I might be wrong

- **I have not traced whether the route payload could be read some other way** I have not seen —
  e.g. whether `RunOptions` or the CLI carries state between phases. My reading of the drain loop is
  that the pair is removed and the payload is dropped, but I did not follow every field of
  `RunReport`.
- **"No interpolation anywhere in the crate" is a grep-level claim** (`prompt` once, no `replace(`
  on it) plus the `mcp_client.rs:99` comment agreeing. It is strong but it is not an exhaustive proof
  that no other path injects text.
- **The 2-option `choice` at 20/22 failing exactly at boundaries may be phrasing-sensitive.** The
  phrasing sweep in `GRAPH-EXAMPLE-WORDCOUNT.md` is a worker's, and I have only spot-checked it.
  If a different phrasing tracks cleanly, then (2) ranks lower than I put it — the boundary errors
  would be a prompt problem, not a shape problem.
- **Blast-radius estimates are inference.** I have not read every caller of `lower_decide` or the
  drain loop.
