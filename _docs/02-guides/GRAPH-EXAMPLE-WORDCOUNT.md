# GRAPH EXAMPLE — the `wc` word count, and the two things it exposed

**Measured 2026-10-02.** Built by a dispatched `pi` + `deepseek-v4.1-flash:cloud` worker on the
ollama-cloud account, then verified here. The worker ran out of wall clock before writing its own
report, so its flows are on disk and **every claim below was re-run by the orchestrator**.

Three flows, and the third is the answer:

| file | routes on | what it does |
|---|---|---|
| `wc-classify.flow.json` | `label` | the worker's version — classifies and picks a branch |
| `wc-classify.control.flow.json` | `label` | the worker's control — a second document |
| **`wc-classify-band.flow.json`** | **`band`** | **the fix** — a low-confidence answer escalates |

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

## 5. What this does NOT show

- **The head cannot compare numbers here.** It answers MEDIUM across a 77× range with low
  confidence. Whether that is a tokenizer artefact of digits or a genuine limit is **not measured
  here** and is not claimed. What is measured is that it *reports* low confidence.
- **`band`-routing is not a fix for the classification.** It converts a silent wrong answer into an
  honest escalation. The classification itself is still wrong; escalating is the correct response to
  that, not a cure.
- **The worker's own report was never written** — it hit the 120 s wall while probing phrasings. Its
  flows are its deliverable; its conclusions are re-derived above from the runs, not quoted from it.

## 6. Reproduce

```sh
export PATH=/root/.local/bin:$PATH
cd /projects/laya-rs
for f in wc-classify wc-classify.control wc-classify-band; do
  laya graph validate "_docs/02-guides/$f.flow.json"
  laya graph run      "_docs/02-guides/$f.flow.json"
done
```

`wc` itself is the shell step, not the decision: `wc -w /projects/_tmp/wc-example/short-note.txt`
prints `13`. The decision is the model's, and that is the point — the graph gives the count a
branch to land on, and `band` decides whether acting on it is honest.
