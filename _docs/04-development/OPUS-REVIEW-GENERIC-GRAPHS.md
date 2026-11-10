# OPUS REVIEW — running generic graphs through laya

**Review by claude + opus, 2026-10-02**, session `d58b577f-fc19-48d5-b690-f6d7524ea4f8` (resumable).
40 turns, $2.45. Report: `_tmp/opus-graph-analysis/REPORT.md` (20,786 B), with 25+ probe
flows and their run reports beside it.

It was asked to **attack my analysis**
(`04-development/GENERIC-GRAPHS-ANALYSIS.md`), not restate it, and to back every claim with a real
`laya graph run` (user instruction: *"graph must be ran from laya!"*). It did both.

---

## It found five contradictions. I re-ran all three of the severe ones myself.

**C1 — routing is not a function of the graph; it is a function of JSON ARRAY ORDER. [SEVERE]**

Same graph, same content, only the order of nodes in the array differs. **Re-verified here:**

```
inner_B listed first:   outer_A -> A_no        AND B_yes RAN   (a port outer_A did NOT select)
                        inner_B ran at all     (behind a port never selected)
                        not_started = []       (the wrong execution is INVISIBLE)
outer_A listed first:   outer_A -> A_no only                   <- correct
```

Three distinct mechanisms, from the code and consistent with every run:
`prune_unreachable` deletes the unselected port targets **without checking other inbound edges**;
`remove_node` deletes a node's edges so a **pruned dependency counts as satisfied**; and
`find_route_pair` takes the first `Route` in `Vec` order.

**This breaks "the graph owns every edge" directly, and `RunReport` reports nothing skipped.** It is
worse than the D1 I recorded, and it is the defect that matters most.

**C2 — `escalate_on_near_tie` is not inert. Its target runs UNCONDITIONALLY. [SEVERE]**

My `ENGINE-DEFECTS` doc says the field is inert. **Re-verified here:** `near_tie_port` is never
lowered into an edge, so its target has **no inbound edge** and becomes a root of phase 2. The render
shows `human` unconnected; the run starts it anyway:

```
0 ms    d        {"label":"no","band":"mid","confidence":0.8518}
243 ms  b        b
243 ms  human    human        <- the escalation target, unconditionally, with NO near tie
```

So a flow that sets the flag runs its escalation branch on **every** run. My "inert" was wrong in the
safe-sounding direction, which is the worst way to be wrong.

**C3 — "no data flow at all" is WRONG as I stated it. Run-time data CAN reach the head.**
**It cannot reach an EDGE.** **Re-verified here:** a shell writes a state file, an `mcp ask` noul
reads it, and the head answers **4/4 correctly with confidence 0.92–0.999**, in the right order —
because phase 2 honours `depends_on`:

```
0 ms  count_long      The word count in the file is 412 words.
0 ms  count_short     The word count in the file is 13 words.
3 ms  ask_long_gt300  noul 0.9513     3 ms  ask_long_gt100   0.9199
3 ms  ask_short_gt300 noul 0.0008     3 ms  ask_short_gt100  0.0398
```

The missing piece is **data → route**, not data flow. `ask`'s answers carry no `label`/`band`
payload, and a route's upstream must be a lowered `decide`.

**C4 — the 2-option `choice` is not "the weaker shape". The 20/22 came from PHRASING.**

It re-ran the 15-point grid **through `laya graph run`**:

| shape | phrasing v1 "The word count is N." | phrasing v14c "…in the file is N words." |
|---|---|---|
| 2-option `choice` (a flow decide) | **15/15**, boundary conf 0.35–0.87 | 13/15, wrong at 301 and 101 |
| `noul` via `ask` + file state | 13/15, wrong at 100 and **5 > 300** | 14/15, wrong at 300 |
| `noul` via raw `decide` | **15/15**, conf 0.875–1.0 | not run |

So the flow's only reachable shape scores **15/15** under v1 — and the doc's own §6.3 showed this,
while §6.4 traded boundary correctness for bands. **My sentence "the engine's only reachable
question form is the weaker one" must be withdrawn.** The defensible claim: *choice is
label-correct but low-confidence at boundaries, so band-gating escalates correct answers.*

**C5 — D1 confirmed, and worse: the terminal branch and the shell start in the same millisecond.**
`class_short` and `count_words` both at ms 575 — the classification and the `wc` it supposedly
depends on are simultaneous.

---

## Its verdict

> A generic graph cannot run through laya today. The first blocker is not missing data flow. It is
> that **the engine's routing is not a function of the graph.**

**The two-phase model should be removed, not repaired.** The replacement already exists:
`sched::run_from` is a correct DAG scheduler that already has a `NodeBody::Route` arm
(`sched.rs:372`) which today succeeds as a no-op. Making that one scheduler route-aware fixes D1,
C1 and C2 together.

## Its ranked recommendations

| # | change | why this rank |
|---|---|---|
| **R1** | **One route-aware scheduler.** Delete the drain loop; make `run_from` handle routes: on a Route record, read its upstream's output, run `extract_on_value` + `route_port`, decrement indegree **only along the selected edge**, mark other ports **dead**. Two edge classes: `depends_on` = AND (dead predecessor ⇒ skip), port edges = OR (any selection ⇒ live). Add `skipped: Vec<String>` to `RunReport` so a pruned node is visible. | C1 is a correctness defect in the thing every other fix depends on. ~150 lines net, removing more than it adds. |
| **R2** | **`{{node_id}}` placeholder in `decide.prompt`** — the trimmed stdout of a node in its `depends_on`. Not an expression evaluator. | This is what makes graphs *general*. Needs R1. |
| **R3** | **`question_type: choice\|noul`** on `Node`. | Real but third: the measured gain is **confidence, not correctness** (C4). Never the default. |
| **R4** | **D2.** Now: refuse the flag in `validate`. Later, on R1: a guard rule checked before the `on` rules. | Stops C2 immediately. |
| **R5** | **Refuse node ids containing `/`.** | The collision hole where a node silently vanishes. One check. |

**Its interim measure if R1 waits:** a ~10-line change so `find_route_pair` picks only a route whose
upstream has **no live inbound edge**, plus validate refusals for the three broken shapes (a decide
depending on a non-decide, a port target shared by two decides, a `depends_on` on a port target) — so
the broken shapes fail **loudly** instead of running wrong.

**And it declined my ranking.** I put `question_type` first as "a day's work, high value". It ranks it
third, because C4 shows a choice at v1 phrasing is 15/15 — the field buys confidence, not correctness,
and correctness is what R1 fixes. **It is right and I was wrong.**

## Its hard-rule vs code-fix line

| finding | verdict |
|---|---|
| D1, C1, C2, `/` in ids | **Defects.** Fix in code. |
| choice-only | **Limitation, not a bug.** The wire shape is honest. Document it, drop the "weaker form" framing. |
| no interpolation | **Limitation, and the real gap.** Hard rule until R2: *a decide's state is a literal; a shell in the same flow cannot inform it.* The `wc` example's `count_words` node is a **witness, not an input**. |

## What it could not determine

Whether a route-upstream decide honours its node `timeout` (code suggests not — inference); whether
the ledger could carry a payload between nodes as a workaround (untested, and it could not drive an
edge anyway); a **live** near tie (never found one, so that routing path is unexercised — though C2
holds regardless); subjective-question behaviour under `noul` vs `choice` (taken from the brief, not
re-measured).
