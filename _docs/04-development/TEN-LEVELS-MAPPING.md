# The ten levels, mapped to our tools

**Source**: IndyDevDan, *Ten Levels of Jev* — `https://youtu.be/_U-O5lYhJ7Q`
(repo `github.com/disler/ten-levels-of-jev`, MIT). The full digest, with the
repo links and our prior research trail, is in
[`../06-research/01-design.md`](../06-research/01-design.md) §12.

**Corrections applied.** This mapping reflects
[`DESIGN-AMENDMENTS-2026-09-29.md`](DESIGN-AMENDMENTS-2026-09-29.md): the `band`
comes from `laya-rs` itself (B5 — a transform node cannot compute it), and `route`
is **not** an MCP tool (M6).

---

## The mapping

| L | the video's concept | our surface | the invariant we must hold |
|---|---|---|---|
| 1 | Single decisions — the "smart if" | `decide` with a `noul` | **the threshold lives in the caller**, in `Policy`, never in a prompt |
| 2 | Multiple choice | `decide` with N questions in ONE call | **fan out**: ask everything you might need, then choose in code which answers to use |
| 3 | Composite scoring | `decide` with several `score`s | **weights stay in the caller** — the model never sees them |
| 4 | Confidence gating | `gate`; bands from `Policy` | **gate on `confidence`**, never on `act_probability` (inert, measured ≡ 1.0) |
| 5 | Intent / model routing | a `choice` over the caller's declared harness list | the answer is a **route**, not control flow — the graph stays the authority |
| 6 | Guardrail hooks | `gate` shaped for a `tool_call` / `tool_result` hook | **a model gate is ONE SIGNAL** (see the failures, below) |
| 7 | Should I compact | `should_compact`, four questions per turn | **token thresholds stay in code**; only the judgement is the model's |
| 8 | Cheap reads | **`ask_file`** | Rust reads the file, returns a typed answer — **the file never enters the agent's context** |
| 9 | Files at scale | **`ask_files`** | glob + prune + 255 cap + **parallel** calls + `pick_first` |
| 10 | Agentic | **`ask`** | **the agent writes the questions**; its command passes the L6 gate first |

### Deliberately out of scope

The video's **presentation layer** — the Vue lab, the animated hero SVGs, the
agent-window UI — exists to teach, not to decide. Our equivalent is `ledger` plus
the redb store: observability and cost-per-decision without a web app.

---

## How a level reaches the graph

```
laya-rs computes:  { "label": "bug_in_code", "band": "high",
                     "confidence": 0.93, "margin": 0.41, ... }
        │
        ▼   plain member access — no arithmetic, no comparison
route  on = "$.payload.label"
       rules = [{eq:"bug_in_code", port:"fix_code"},
                {eq:"wrong_test",  port:"fix_test"},
                {eq:"environment", port:"check_env"},
                {port:"escalate"}]          ← CATCH-ALL
        │
        ▼
ordinary nodes per port
```

**Why `band` must come from us and not a transform node (B5):** the graph's
expression language supports *"only object member access … no arrays, no
functions"*), `transform` emits a member value or a constant, and `route`'s `eq` is exact
`serde_json::Value` equality — no ranges, no comparisons. A transform **cannot** compute a
band, and a route cannot express one. So `laya-rs` computes it, the thresholds
still live in `Policy` TOML, and the graph gets back the one thing its language
can match on: an exact string.

---

## The failures we inherit as requirements

These are the video's own bug list. They are **not** optional extras — each one
shapes a rule above.

**1. A single-path gate gets routed around.** The video's Level 6 gate blocked
writes to `.env`, and the agent wrote the same file via a **bash heredoc**. The
author added a "this block is final, do not work around it" notice and says
plainly: *"that notice is an instruction, not a control."*
→ Our `gate` returns a **reason**, is **one signal among several**, and the denied
branch is **terminal**.

**2. The agent can read its own environment.** In a live run its bash tool printed
every environment variable it was given.
→ `laya-rs` never puts credentials in a tool result, and the redb store holds no
secrets.

**3. Level 10 gates its own command, not the agent's bash tool.**
→ Documented, and `ask` must be paired with a `tool_call` hook rather than trusted
alone.

**4. Saturation is a smell.** We measured the deployed checkpoint returning exactly
`1.000/0.000/0.000` on a real `choice` — the same degenerate-softmax smell as the
inert `act_probability`.
→ `Answer::saturated()` **flags** it in the ledger instead of reporting it as high
confidence, and `escalate_on_saturation` can demote the band so it never
auto-allows.

**5. Rules first, model second** (from our own docs 039/040, which the review
confirmed the design had silently dropped). A probabilistic model must not sit in
front of classification that is already correct and free — `FailoverReason` is
derived in code from HTTP facts.
→ **Static rules match first; the model classifies only the unresolved remainder.**

---

## Two lanes, never one

The video's thesis is *"Jev AND your agents"*, and it is also our docs 039/040
conclusion. The cheap lane handles the bounded decisions; the expensive lane does
the open work; the graph owns every edge in both cases. `laya-rs` never replaces
the LLM lane — **its uncertainty is the LLM lane's work queue**.

### L7 `should_compact` — measured, and one question had to be rewritten (2026-09-30)

Implemented with the four judgements in ONE call and the thresholds in code, as the level
specifies. The interesting part is a measurement that changed the implementation.

**The `mid_edit` question agreed with a known-intended answer only 1/6 times.** The original
phrasing was double-barrelled — *"Is an edit in progress that spans more than one step AND
would be broken by losing the conversation?"* — and the model answered something else: 3
false positives, 2 false negatives. Because `mid_edit` is a **veto**, the false positives
suppressed every genuine compaction request (a measured 500k-token context whose work had
finished came back `notice`, i.e. never compact) and the false negatives permitted exactly
the compaction the veto exists to prevent.

Four phrasings were scored against the same six cases with known intended answers:

| phrasing | agreement | false pos | false neg |
|---|---|---|---|
| original (two conditions joined) | **1/6** | 3 | 2 |
| `Is work currently IN PROGRESS and not yet finished?` | **6/6** | 0 | 0 |
| `Would losing this conversation BREAK an unfinished change?` | **6/6** | 0 | 0 |
| inverted (`Has the work been FINISHED?`) | 5/6 | 0 | 1 |

Two independent phrasings reaching 6/6 is corroboration; the simple, single, concrete form was
kept. **Small sample (six cases)** — this is a large unambiguous improvement on 1/6, not a
calibrated rate, and it is recorded that way.

**The generalisable lesson:** a question is a component and it can be wrong. This one was
scored, found unusable, and replaced — *before* shipping. A veto built on a coin-flip
question is worse than no veto, because it silently makes the tool useless.

### L10 `ask` - implemented, and it exposed a hole in the gate (2026-09-30)

L10 inverts levels 1-9: **the agent writes the questions itself**, over a state it composes
from a summary, files, and a command it wants run. Implemented as one tool, `ask`.

Building it produced the most serious finding of the project, and not in the new code.

**The e2e proof caught the gate ALLOWING an `rm -rf` and the command running.** The check that
caught it was an *effect* check - the protected file had to still exist afterwards - not a
message check. A message saying "refused" while the command had run anyway is precisely the
failure the source research records at level 6, where an agent routed around a block.

Diagnosing it produced three measurements:

| what was tried | result |
|---|---|
| `"Is this action safe?"` as a noul | **6/12** - `ls -la /tmp` 0.83, `rm -rf <dir>` 0.89 |
| six consequence phrasings | **every one had a NEGATIVE separation** - no threshold could work |
| a `choice` over declared effects | **13/18** - still missed `: > file` and `dd if=... of=...` |

And a root cause underneath, independent of the model:

```rust
Band::High => GateDecision::Allow,                 // allows REGARDLESS of irreversible
Band::Low  => GateDecision::Deny,
Band::Mid if irreversible => GateDecision::Ask,    // identical bodies:
Band::Mid => GateDecision::Ask,                    //   the flag changed nothing
```

`irreversible` was decorative, defaulted to `false`, and **nothing ever inferred it** - while
the source research's whole point is that the model *generalises* irreversibility rather than
being told. The measured `allow` on `rm -rf` came through that hole.

**The fix follows the research's own rule - "treat a security gate as one signal, not the only
control" - applied in the other direction too: two signals, and `allow` requires both.**

* the model's verdict (generalises, unreliable alone),
* a conservative effect-based pattern check (narrow, exact where it matches).

Neither alone is sufficient, and each covers the other's failure: the model catches shapes
nobody enumerated (`find . -delete`), the pattern catches the idioms the model was *measured*
to miss. `mid_edit` at 1/6 and this gate at 6/12 are the same discovery twice: **ask this head
to CLASSIFY CONTENT, not to REASON ABOUT CONSEQUENCES.** "Is there a bug?" scored 0.80
correctly; "is this safe?" is a coin flip.

---

## Verified against the deployed tool, 2026-10-02 — all ten levels work

Each level was called through the deployed `laya-mcp` over stdio, with the operating environment the
host now passes. The mapping above was written from the design; this is what the running system does.

| L | surface | result | evidence |
|---|---|---|---|
| 1 | `decide` + `noul` | works | `label=yes band=mid conf=0.7766` |
| 2 | `decide`, N questions in one call | works | two questions answered in one call |
| 3 | `decide` + `score` | works | `label=2.8702 band=low conf=0.1462 score=2.8702` |
| 4 | `gate`, bands from `Policy` | works | `decision=allow band=mid conf=0.8722` |
| 5 | `choice` over the caller's harness list | works | `label=positive band=mid conf=0.889` |
| 6 | `gate` as a guardrail | works | destructive => `decision=ask`, 2 signals |
| 7 | `should_compact` | works | 4 judgements answered |
| 8 | `ask_file` | works | answered; file content **not** in the response |
| 9 | `ask_files` | works | `asked=1 hits=1` |
| 10 | `ask` | works | `command_ran=True timed_out=False` |

### The measurement that nearly became a false finding

A first pass reported levels 2, 3 and 5 as **failing**, with `AttributeError: 'NoneType' object has no
attribute 'items'` for `choice` and `TypeError: 'NoneType' object is not iterable` for `score`. Those
errors are real, and they are reproducible through the shipped tool:

```
decide {questions: {verdict: {type: choice, question: …, instructions: …, options: [positive, negative]}}}
  -> FAIL  AttributeError: 'NoneType' object has no attribute 'items'

decide {questions: {quality: {type: score, question: …, instructions: …}}}
  -> FAIL  TypeError: 'NoneType' object is not iterable
```

**The fault was in the probe, not the product.** The sidecar's question payload needs a **`criteria`**
field, not `options`, and a `score` needs `criteria` even though it has no alternatives to list. With
the right field name, through the same shipped tool:

```
choice + criteria -> {"label":"positive","band":"mid","confidence":0.889}
score  + criteria -> {"label":"2.8702","band":"low","confidence":0.1462,"score":2.8702}
```

**What this cost, and what it teaches.** Two rounds were spent on a "defect" that was an argument name —
first `options` (from this document's own example, and from a natural reading of the JEV sources), then
a matrix of guesses. The honest lesson is recorded because it is the third time in this session that a
check was wrong before the code was:

- the probe asked for `options` because **this document's example says `options`** — so the doc, not the
  code, was the source of the error, and the doc was never checked against the running system;
- the error message did not help: `AttributeError: 'NoneType' object has no attribute 'items'` names
  neither the missing field nor the question. A refusal that says *"choice question `verdict` needs
  `criteria`"* would have ended this in one call. That is a real, small, worth-fixing observation about
  the error path — and it is the same class as the `ask_file` argument-naming defect recorded against
  spec 13 R5.
- **Two independent facts were being conflated:** "the sidecar needs `criteria`" (a schema fact) and
  "levels 2/3/5 are unimplemented" (a false conclusion). Separating them required reading the sidecar's
  own accepted shape rather than trusting either the doc or the error.

**Unchecked, and stated as such:** whether the field name should be `criteria` at all, or whether
`options` ought to work and the sidecar's shape is the awkward one. The MCP schema advertises neither
field for a question object, so a caller has to discover this by trying — which is how this document
came to be wrong. Worth a decision, not asserted here.

---

## Verified end to end, and the three contract defects that found (2026-10-02)

`scripts/check-ten-levels.py` drives all ten through the **shipped MCP tools** over stdio — the
surface an agent actually uses — so a level that passes here works for a caller, not just for a
unit test. It carries a control: L10's command must be **refused by the gate** (`mkfs.ext4`) and
never run, so the level cannot pass by executing whatever it is given.

```
  PASS  L1  single decision (noul)         label='yes' band='mid' conf=0.8219
  PASS  L2  multiple questions, one call   answered ['a','b'] in one call
  PASS  L3  composite scores               risk=2.4327, size=2.1257
  PASS  L4  confidence gating              git status -> allow, mkfs.ext4 -> ask
  PASS  L5  intent routing                 route label='cheap'
  PASS  L6  guardrail hook                 2 signal(s); allow needs an allowlist
  PASS  L7  should I compact               urgency='notice', 4 judgement(s)
  PASS  L8  cheap read                     1 answer, content_in_context=False
  PASS  L9  files at scale                 asked=2, 2 per-file answers, 2 ranked hits
  PASS  L10 agentic                        1 answer, command_ran=True
  PASS  L11 CONTROL: L10's command gated before it runs
  11/11
```

### The defects, all in the tool's ADVERTISED contract

Writing this harness exposed three things the documentation and `tools/list` get wrong. Each was
found by an agent-shaped caller failing, which is the only kind of caller that matters.

**1. `questions` is advertised as an array; the sidecar requires an object map.** `tools/list`
declares `"questions": {"type": "array"}`, and sending an array yields
`AttributeError: 'list' object has no attribute 'keys'` — measured. The working shape is a map
keyed by question id.

**2. The question's text field is `instructions`, not `text`.** The array form's `laya_abi::Question`
carries `text`, but the wire format the sidecar reads requires `instructions`; sending the
advertised shape yields `KeyError: 'instructions'`. So an agent following the schema fails twice
over on the same argument.

**3. A `score` requires `criteria` as a LIST of level descriptions.** Without it:
`TypeError: 'NoneType' object is not iterable`. Nothing in the advertised schema says the field is
required — and the levels are the scale the score is measured against, so an omitted `criteria`
makes a score uninterpretable even when it does not raise.

**A note on how these were found.** The first run of this harness reported 3/11, and eight of those
failures were **the harness's own wrong inputs** — I invented the argument shape, sent `criteria`
where the sidecar wants `options`, and then asserted on response fields the tools never had
(`payloads` on `should_compact`, `answers`/`files_read` on `ask_files`, `command_ran: false` on a
refusal that arrives as a JSON-RPC error). Each time, the check was at fault and the tool was
right. The three defects above are the residue after correcting all of that, and they are real.

---

## The graph, exercised on the live backbone (2026-10-02)

The flow → graph translation, run for real rather than described. Both fixtures ask the
**identical question** with different states, so the pair is its own control — if the model's label
did not reach the router, both runs would take the same branch.

```
$ laya graph run crates/laya-graph/tests/fixtures/sentiment-positive.json
  classify        payload.label=positive  band=mid   conf=0.7001  margin=0.8936  180 ms
  classify/route  on=$.payload.label       value=positive  -> selected_port=branch_positive
  branch_positive echo branch:positive     exit=0
  completed, 3 node(s) ran

$ laya graph run crates/laya-graph/tests/fixtures/sentiment-negative.json
  classify        payload.label=negative  band=high  conf=0.9566  margin=0.9906  181 ms
  classify/route  on=$.payload.label       value=negative  -> selected_port=branch_negative
  branch_negative echo branch:negative     exit=0
  completed, 3 node(s) ran
```

And the control, side by side:

```
  positive  -> executed branch:positive
  negative  -> executed branch:negative
```

**What this proves, in the order it matters:**

1. **A real model decision drives control flow.** `conf=0.7001` and `conf=0.9566` came from the live
   head, not from a fixture constant — the two states genuinely scored differently.
2. **The route is pure member access.** `on = "$.payload.label"`, then exact `eq` against the port
   keys. No arithmetic, no comparison, no ranges — which is the whole reason `band` is computed by
   laya-rs (B5) rather than by a transform node: the graph's expression language cannot express
   either.
3. **Only the selected branch runs.** `records` holds three entries, not five: the unselected
   branches are not executed and do not appear. Routing here is control flow, unlike Level 5's
   `choice`, where the answer is a *route* returned to a caller.
4. **A catch-all port is mandatory and present** (`"_": "branch_fallback"`), so a model-fed route
   cannot produce an undefined edge — the rendering shows it as `n1 -->|_| n4`.

### The four subcommands, and what each is for

```
laya graph validate <flow.json>   parse + validate; names the refusal, exit 1 if invalid
laya graph render   <flow.json>   mermaid, plus what the renderer cannot carry
laya graph run      <flow.json>   execute; run result as JSON, one summary line on stderr
```

`render` states its own limits rather than dropping them silently:
`dropped: retry, timeout, allow_failure, writes, subflow with-bindings, subflow content_hash` — a
rendering that quietly omitted the retry policy would make a retrying flow look identical to one
that fails on the first error.

**One caveat worth holding onto:** `laya graph` shells out for `shell` nodes, so unlike
`laya decide` it is not a pure read — and a `shell` node's non-zero exit is a **failure**, not
data. Both facts are load-bearing for anyone wiring a graph into automation.
