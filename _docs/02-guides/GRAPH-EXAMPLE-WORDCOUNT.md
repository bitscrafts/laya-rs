# GRAPH EXAMPLE — the `wc` word count, and the two things it exposed

**Measured 2026-10-02.** Built by a dispatched `pi` + `deepseek-v4.1-flash:cloud` worker on the
ollama-cloud account, then verified here. The worker ran out of wall clock before writing its own
report, so its flows are on disk and **every claim below was re-run by the orchestrator**. Later
the same day the example was REBUILT as `wc-classify-chained.*` after the failure cause was
measured (section 7) — the worker's three flows are kept below as the record of the failure, not
as the current answer.

Five flows, in build order:

| file | routes on | status | what it does |
|---|---|---|---|
| `wc-classify.flow.json` | `label` | **FAILED, kept as the record** | worker's 3-option classify — MEDIUM for everything |
| `wc-classify.control.flow.json` | `label` | **FAILED, kept as the record** | worker's control — reached the SAME branch, caught the failure |
| `wc-classify-band.flow.json` | `band` | partial fix, label-blind | escalates a low band; cannot branch on the label at all |
| **`wc-classify-chained.flow.json`** | `band` gates + `label` asks | **CURRENT** | two chained threshold questions, gate→ask |
| **`wc-classify-chained.control.flow.json`** | `band` gates + `label` asks | **CURRENT** | identical shape, 412-word document, must differ |

---

## 1. The limitation the exercise was built to test — CONFIRMED

A `wc` result cannot reach a route on its own. Verified in the source:

- `crates/laya-graph/src/lower.rs:104` — `"prompt": node.prompt` is copied **verbatim** into
  `json!({...})`. The word `prompt` appears **once** in the entire file. There is no `${...}`, no
  `$.inputs`, no substitution of any kind.
- `crates/laya-graph/src/route_exec.rs:70-76` — a route's upstream **must be an `mcp` node**;
  anything else returns `RunError::UnknownNode` with the comment *"a non-`mcp` upstream means this
  graph was not produced by `lower`, which G21 forbids."*

**So a shell node's output can only reach a route by being routed through a `decide` node** — the
model reads the number and answers a label. The `wc` number must be written *into the decide node's
prompt* for the model to see it at all. That is why the example looks the way it does, and it is a
real constraint of the engine, not a mistake in the example.

## 2. The worker's finding — the control did not differ

The two flows ask the identical question about documents of **13 words** and **412 words**. Both
routed to the **same** branch:

```
13 words  -> label=MEDIUM -> selected_port=report_medium
412 words -> label=MEDIUM -> selected_port=report_medium
```

The worker said *"the model is not reading the number"*. Correct, and it reproduced here. **The
control was therefore doing its job** — it caught that the pair certified nothing.

## 3. The measurement the worker did not take — WHY it does not differ

The worker called it a capability limit. It is better described as the head telling the truth and
the flow ignoring it. Same question, same options, **only the number varies**:

```
 words   label     confidence   probabilities
    13   MEDIUM        0.1886   {SHORT 0.374, MEDIUM 0.551, LONG 0.076}
   100   MEDIUM        0.2213   {SHORT 0.219, MEDIUM 0.664, LONG 0.117}
   250   MEDIUM        0.1130   {SHORT 0.179, MEDIUM 0.571, LONG 0.250}
   412   MEDIUM        0.1642   {SHORT 0.210, MEDIUM 0.626, LONG 0.165}
  1000   MEDIUM        0.0966   {SHORT 0.213, MEDIUM 0.558, LONG 0.230}
```

Two facts, both load-bearing:

1. **The label never moves** across a 77× range (13 → 1000).
2. **The confidence is 0.09–0.26 every time** — far below the policy's `low_threshold` of 0.5, so
   the head bands every one of these as **`low`**.

Raw payload, verbatim:

```json
"payloads": { "q": { "label": "MEDIUM", "band": "low", "confidence": 0.1886, ... } }
```

**The head is not hiding its uncertainty. It states it, and the flow routed on `label` anyway.**

## 4. The fix — route on `band`, and it is measurable

Same state, both flows, run back to back:

```
wc-classify.flow.json       classify: band=low confidence=0.2533
                            classify/route: on=$.payload.label value=MEDIUM
                            -> selected_port = report_medium          "CLASSIFIED: MEDIUM"

wc-classify-band.flow.json  classify: band=low confidence=0.4061
                            classify/route: on=$.payload.band value=low
                            -> selected_port = escalate_low_confidence "ESCALATED: the head does NOT know"
```

**`band=low` in both.** One flow acted on a label the head had just disowned; the other escalated.

This is `AGENTS.md`'s **rule 4** in practice — a band is computed by us, and the payload arrives
ready to match — and its **rule 2** — the graph owns the edge. Routing on `label` and ignoring
`band` is the same failure as gating on a coin flip: the engine lets you do it, and nothing warns.

## 4b. The second measured limit, same day: a bare `noul` on an unbindable subject is a confident constant

The 3-option `choice` at least reported `band=low`. This limit is worse — **confidently wrong** —
and it is why the band gate is not a cure, only a bound. Source: `_docs/04-development/HEAD-QUESTION-SHAPE-LIMITS.md`
(section 2), where these rows were first recorded (2026-10-02):

| text | question | answer | confidence |
|---|---|---|---|
| "This update is fantastic — exactly the fix I needed, and it works perfectly." | `noul` "Is this feedback positive?" | **no** | **1.0** |
| "This broke everything and I am furious. Nothing works." | `noul` "Is this feedback positive?" | no | 1.0 |
| "This update is fantastic …" (same text) | 2-option `choice` positive/negative | positive (correct) | 0.8571 |
| "This broke everything …" (same text) | 2-option `choice` positive/negative | negative (correct) | 0.9140 |

A bare `noul` whose subject the head cannot bind answers `no` at confidence 1.0 for both — a
constant that reports maximum confidence, so a `band` route reports `high` and lets it through.
Where this head is used to select a branch, the branch must be selected on a fact the state states
explicitly (like a word count), or the verdict must be bounded by a deterministic companion
(`AGENTS.md` rule 5: a model gate is ONE signal).

## 5. What the first rebuild does not show

- **The head cannot compare numbers IN THE 3-OPTION FORM.** That sentence was true as measured and
  is **superseded the same day**: the `noul` threshold form (below, section 7.1) and the 2-option
  `choice` yes/no form (section 7.2) DO track the number. The claim above stands for the 3-option
  form; do not read it as a general capability claim. Whether any of this is a tokenizer artefact
  of digits or a genuine limit is **not measured here** and is not claimed.
- **`band`-routing is not a fix for the classification.** It converts a silent wrong answer into an
  honest escalation. The classification itself is still wrong; escalating is the correct response,
  not a cure.
- **The worker's own report was never written** — it hit the 120 s wall while probing phrasings. Its
  flows are its deliverable; its conclusions are re-derived above from the runs, not quoted from it.

## 6. The rebuild on a measured boundary — TWO chained threshold questions

(spec 15) The 3-option `choice` does not track the number; the threshold forms do. So the worked
example is `wc-classify-chained.flow.json` + its control:
**a 3-way classification built from TWO threshold questions**, chained, with every threshold
question asked as a **gate → ask pair**:

```
count_words (shell: wc -w on the document)
  -> gt300_gate  (decide: "Is the word count greater than 300 words?"  routes on BAND)
       mid|high -> gt300_ask  (the same question again, routes on LABEL)
                     yes -> class_long
                     no  -> gt100_gate (decide: "Is the word count greater than 100 words?"  BAND)
                             mid|high -> gt100_ask (same question, LABEL)
                                           yes -> class_medium
                                           no  -> class_short
       low|_ -> escalate_gt300
                             (gt100's low|_ -> escalate_gt100)
```

### 6.1 Why two nodes per threshold question — an engine fact, measured

One `decide` node owns exactly ONE route, and a route matches exactly ONE payload field
(`crates/laya-graph/src/lower.rs:112-117` — `on` lowers to `$.payload.label` or `$.payload.band`,
nothing else; `route_port` at `crates/laya-graph/src/graph.rs:61-69` is exact-equality,
first-match-wins, then `_`). A `band` value is one of
`low`/`mid`/`high` and can never equal `yes`/`no`; a `label` is the answer and carries no band.
**No single node can both branch on the label and escalate on a low band.** So each threshold
question is asked twice: the gate routes on `band` (low confidence prunes the ask — the answer is
never nominated), and the ask routes on `label`. Both calls send the identical state+question, and
each run report shows both calls' bands — a divergence between gate and ask would be visible in the
artifacts themselves. Measured: identical (the ask repeats the gate's answer bit-for-bit in every
run below).

### 6.2 What the wire actually carries — a flow decide is ALWAYS a `choice`

A flow-authored `decide` lowers to tool arguments `{question, options, prompt}`
(`crates/laya-graph/src/lower.rs:97-106`), and `LiveMcpCaller` REBUILDS that as
`{type: "choice", instructions: <question>, criteria: {<option>: <option>}}`
(`crates/laya-cli/src/mcp_client.rs:106-120`). Consequence, measured live: **a flow decide cannot
be a `noul`** — no options means `criteria: {}`, and laya-mcp refuses a choice with empty criteria
(`crates/laya-mcp/src/lib.rs:418-428`). Demonstrated: a decide node with no options passes
`laya graph validate`, and the run stops early at the node with
`question 'answer' is a 'choice' whose 'criteria' is empty, so there is no option to choose`.

So the spec's measured table — `noul` "greater than 300?" tracks, 15/15 (`scripts/check-wc-boundary.py`)
— describes a shape **unreachable from a flow**. The closest flow-reachable shape is the 2-option
`choice` over `yes`/`no`, measured next.

### 6.3 The 2-option `choice` tracks the number — measured on the wire path

Exactly the arguments `LiveMcpCaller` sends, over the same grid `scripts/check-wc-boundary.py` uses
(state `The word count is N.`, criteria `{"yes": "yes", "no": "no"}`):

```
threshold: greater than 300?
      5  want=no   got=no    band=mid   conf=0.8727  OK
    100  want=no   got=no    band=high  conf=0.9307  OK
    299  want=no   got=no    band=mid   conf=0.5651  OK
    300  want=no   got=no    band=low   conf=0.3483  OK
    301  want=yes  got=yes   band=low   conf=0.4736  OK
    412  want=yes  got=yes   band=low   conf=0.2953  OK
   1000  want=yes  got=yes   band=mid   conf=0.5076  OK
   5000  want=yes  got=yes   band=mid   conf=0.797  OK

threshold: greater than 100?
      5  want=no   got=no    band=mid   conf=0.8479  OK
     13  want=no   got=no    band=mid   conf=0.8764  OK
     50  want=no   got=no    band=mid   conf=0.8376  OK
    100  want=no   got=no    band=mid   conf=0.5421  OK
    101  want=yes  got=yes   band=mid   conf=0.8697  OK
    150  want=yes  got=yes   band=mid   conf=0.8714  OK
    299  want=yes  got=yes   band=high  conf=0.9159  OK

VERDICT: 15/15 correct
```

**15/15, including both exact boundaries — the labels track.** But look at the bands: they wobble
(`412 > 300` answers correctly at band **low**, 0.2953). Under this phrasing the band-gate would
escalate a CORRECT answer at exactly the document the example needs. That is not a defect to route
around — it is the reason the phrasing sweep in 6.4 exists.

### 6.4 The phrasing sweep — phrasing decides correctness AND band

Fourteen candidate (state, question) phrasings were measured on the calls the two flows actually
execute — the main flow's gates are `(13>300)` and `(13>100)`, the control's executed gate is
`(412>300)` (its `>100` subtree is pruned after `yes`). Probe scripts:
`/projects/_tmp/wc-example/probe-decide-shapes.py`, `probe-phrasing.py`, `probe-phrasing3.py`,
`probe-phrasing4.py`, `probe-v14c-grid.py` (scratch, not checked in). Labels `WRONG` disqualify;
`band=low` disqualifies for a gate (it escalates):

| variant (state / question) | 13>300 | 13>100 | 412>300 | verdict |
|---|---|---|---|---|
| v1 "The word count is 13." / "Is the word count greater than 300?" | no/mid 0.9173 | no/mid 0.8764 | yes/**low** 0.2953 | low-band |
| v2 rich criteria text ("the word count is greater than 300") | no/mid 0.852 | — | **no**/low 0.0344 | **WRONG + low-band** |
| v3 worker-style state ("A shell command counted … It printed: `13 …`") | no/mid 0.6464 | no/**low** 0.2067 | yes/mid 0.7011 | low-band |
| v7 "The document has {n} words." / "Is the word count greater than {T}?" | no/mid 0.7406 | no/mid 0.6247 | yes/**low** 0.2637 | low-band |
| v10 "The word count is {n}." / "Does the word count exceed {T}?" | no/high 0.9127 | no/mid 0.8785 | yes/**low** 0.0015 | low-band |
| v11 + " Answer yes or no." | no/high 0.9034 | no/mid 0.8646 | **no**/low 0.0062 | **WRONG + low-band** |
| v13 "The document has {n} words." / "Does the document exceed {T} words?" | no/mid 0.8649 | no/mid 0.8885 | yes/**low** 0.2999 | low-band |
| **v14c "The word count in the file is {n} words." / "Is the word count greater than {T} words?"** | no/mid 0.8518 | no/mid 0.8062 | **yes/mid 0.632** | **CHOSEN — all three gates proceed** |

(v5 count-first instruction and v9 wc-output states also produced 412 WRONG labels; v15path and
v14a were clean but with thinner 412>300 margins — 0.5728 and 0.5167.)

**The chosen phrasing's full grid** (`probe-v14c-grid.py`):

```
greater than 300 words?   5 no/mid .8025 | 100 no/high .9055 | 299 no/mid .829 | 300 no/low .3334 | 301 no/low .1628 WRONG | 412 yes/mid .632 | 1000 yes/low .2516 | 5000 yes/mid .8417
greater than 100 words?   5 no/mid .8806 | 13 no/mid .8062 | 50 no/mid .8582 | 100 no/low .2861 | 101 no/low .0144 WRONG | 150 yes/mid .7724 | 299 yes/high .9546
VERDICT: 13/15
```

Note what this grid costs: **the flow-reachable 2-option shape is NOT boundary-exact under the
chosen phrasing** — `301 > 300` and `101 > 100` both answer `no` (band low, so they escalate). The
noul form was 15/15 at the boundaries; this shape is 13/15 with exactly the off-by-one errors the
boundary script guards against. A document NEAR a boundary will escalate here — which is honest,
but do not mistake the escalation for correctness.

### 6.5 The two runs — the pair differs

Main flow (`wc-classify-chained.flow.json`, 13-word document) — key records, verbatim:

```
gt300_gate       {"payload":{"label":"no","band":"mid","confidence":0.8518,...}}
gt300_gate/route {"on":"$.payload.band","value":"mid","selected_port":"gt300_ask"}
gt300_ask        {"payload":{"label":"no","band":"mid","confidence":0.8518,...}}
gt300_ask/route  {"on":"$.payload.label","value":"no","selected_port":"gt100_gate"}
gt100_gate       {"payload":{"label":"no","band":"mid","confidence":0.8062,...}}
gt100_gate/route {"on":"$.payload.band","value":"mid","selected_port":"gt100_ask"}
gt100_ask        {"payload":{"label":"no","band":"mid","confidence":0.8062,...}}
gt100_ask/route  {"on":"$.payload.label","value":"no","selected_port":"class_short"}
class_short      "CLASSIFIED: SHORT (not more than 100 words)"
count_words      "13 /projects/_tmp/wc-example/short-note.txt"   (ran LAST — see 6.6)
```

Control (`wc-classify-chained.control.flow.json`, 412-word document) — key records, verbatim:

```
gt300_gate       {"payload":{"label":"yes","band":"mid","confidence":0.632,...}}
gt300_gate/route {"on":"$.payload.band","value":"mid","selected_port":"gt300_ask"}
gt300_ask        {"payload":{"label":"yes","band":"mid","confidence":0.632,...}}
gt300_ask/route  {"on":"$.payload.label","value":"yes","selected_port":"class_long"}
class_long       "CLASSIFIED: LONG (more than 300 words)"
count_words      "412 /projects/_tmp/wc-example/long-note.txt"
```

**`class_short` vs `class_long` — the pair is a real control.** The gate and ask bands agree
everywhere (the ask repeats the gate's answer exactly), and each `wc` line matches the count in
the prompt literal. Full JSON reports: `/projects/_tmp/wc-example/REPORT.md`.

### 6.6 The break-the-control check — the pair collapses and is restored

The control was then FORGED to the same count as the main flow (`wc` pointed at `short-note.txt`,
the four `412` literals replaced by `13`) and re-run: it walked the full chain — `no, no` — and
selected **`class_short`**, the SAME terminal as the main flow, with the two answers byte-identical
to the main flow's (0.8518 / 0.8062). **The pair collapsed; the difference between the real runs
came from the data (the count), not from the graph shape.** The file was restored from a byte-copy
(`/projects/_tmp/wc-example/control-flow-backup.json`, md5 `cd3f41e5e002778a8586300c5a812490` both
before and after) and re-run confirmed `class_long` again. Nothing was deleted.

### 6.7 The low-band escalation, live — and its near-miss control

`a state the head bands low` must reach an escalate port, not a nominated branch. Two probe flows
(scratch: `/projects/_tmp/wc-example/probe-low-band.flow.json` and its near-miss
`probe-mid-band.flow.json`; identical gate, only the count differs):

```
probe-low-band  (300 words, same phrasing)  band=low  0.3334  -> selected_port=escalated_low_band
probe-mid-band  (412 words, same phrasing)  band=mid  0.632   -> selected_port=proceeded_on_mid_or_high
```

The low-band state answered `no` — CORRECT — and the flow still refused to nominate it, because the
only route is on band. The mid-band control proceeds. This is the measured difference between
routing on `band` and routing on `label` that section 4 demonstrated with the 3-option form.

### 6.8 A shell cannot feed a route — two refusals, one hole

- **(a) A hand-authored `route` node is refused.** A flow with `"kind": "route"` passes
  `laya graph validate` (validate only refuses unknown kinds), and `laya graph run` refuses at
  lowering: `laya: node 'forged_route' has unsupported kind 'route'` (rc=1;
  `crates/laya-graph/src/lower.rs:62-67`).
- **(b) A shell directly upstream of a route is refused live.** Naming a shell node `ask/route`
  collides with the decide's lowered route node; listed BEFORE the decide, its dependency edge
  lands first and `find_route_pair` (`crates/laya-graph/src/route_exec.rs:154-178`) pairs the
  route with the SHELL — refused, rc=1:
  `laya: edge names node 'shell_step', which the graph does not declare` — that IS
  `RunError::UnknownNode` (`crates/laya-graph/src/error.rs:155`) from the route-pairing guard at
  `crates/laya-graph/src/route_exec.rs:68-76`.
- **(b2) The near-miss control exposes a hole.** The SAME collision with the shell listed AFTER the
  decide refuses nothing: the run succeeds (`no` → `t2`), and the colliding shell node **silently
  vanishes** — it never runs, appears in no record, and is not in `not_started`
  (`remove_node` removes by id, and both the route node and the shell node share the id). An id
  collision is neither validated against nor refused at lowering; depending on node order the run
  either refuses or silently swallows a node. REPORTED, not fixed — `crates/` is outside this
  spec's scope; the fix belongs in `lower` (refuse a flow node id containing `/`).

### 6.9 Three timing/mechanics facts found while building this

1. **A decide runs BEFORE the shell it depends on.** In every run above, `count_words` starts at
   ms 393–514 — AFTER all decides (ms 0) — despite `depends_on`. `run_flow` drains every
   mcp+route pair before the tail schedules the shells
   (`crates/laya-graph/src/route_exec.rs:67`-108). `depends_on` from a shell to a decide declares
   intent for `validate`; it does not order execution.
2. **`escalate_on_near_tie` / `near_tie_port` are declared but inert.** `validate` requires the
   port to exist when the flag is set (`crates/laya-graph/src/validate.rs:79`), but `lower` never
   reads either field — no near-tie port is emitted, so a near tie routes by label with no escape
   hatch from a flow. (The payload DOES carry `near_tie` — the fields are a stub.)
3. **The head's confidence is not bit-stable over time.** One identical call (`13 > 300`, chosen
   phrasing) measured 0.8518, then 0.8062, then 0.8518 across three processes in one hour — Δ up
   to 0.046; within one process three repeats were bit-identical. Labels were stable at every
   margin relevant here. `AGENTS.md` lists llama.cpp cross-thread/build determinism as
   known-unmeasured; this is one measured data point.

## 7. Reproduce

```sh
export PATH=/root/.local/bin:$PATH
cd /projects/laya-rs

# the threshold measurement the design rests on (noul shape, raw tool): 15/15
python3 scripts/check-wc-boundary.py

# validate + render + run the current pair
for f in wc-classify-chained wc-classify-chained.control; do
  laya graph validate "_docs/02-guides/$f.flow.json"
  laya graph render   "_docs/02-guides/$f.flow.json"
  laya graph run      "_docs/02-guides/$f.flow.json"
done

# the historical three (kept as the record of the failure)
for f in wc-classify wc-classify.control wc-classify-band; do
  laya graph run "_docs/02-guides/$f.flow.json"
done
```

`wc` runs and prints `13`, but **read section 6.10 before believing it drove the decision** — it did
not, and cannot:

**CORRECTION 2026-10-02, measured after this document was first written.** `count_words` starts at
**ms 703 — dead last**, after every `decide` node (ms 0, 185, 346, 505). The `wc` node is
**inert decoration**: its output goes nowhere, and the classification works only because the count
is written into each `decide` node's `prompt` text, which the model reads. Both facts together mean
the example demonstrates the graph's *shape* while proving nothing about `wc` — and an earlier
version of this document claimed otherwise.

```sh
export PATH=/root/.local/bin:$PATH
laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | python3 -c "
import sys, json
for line in sys.stdin:
    if line.startswith('{'):
        d = json.loads(line)
        for r in sorted(d['records'], key=lambda x: (x.get('started_at_ms') or 0, x['node_id'])):
            print(f\"  {r['node_id']:<20} started_at_ms={r.get('started_at_ms')}\")
        break
"
```

Expect `count_words` to be LAST, not first. That is the measurement below, and it is the reason a
`wc`-driven graph cannot be built on this engine as it stands.