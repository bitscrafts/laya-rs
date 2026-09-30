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

**Why `band` must come from us and not a transform node (B5):** pii-graph's
expression language supports *"only object member access … no arrays, no
functions"* (`expr.rs:3-8`), `transform` emits a member value or a constant
(`nodes.rs:43-62`), and `route`'s `eq` is exact `serde_json::Value` equality
(`nodes.rs:113`) — no ranges, no comparisons. A transform **cannot** compute a
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
