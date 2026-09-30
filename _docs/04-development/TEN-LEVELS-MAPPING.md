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
