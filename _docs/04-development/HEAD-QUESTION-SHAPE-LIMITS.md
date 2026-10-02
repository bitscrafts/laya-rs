# The head's QUESTION-SHAPE limits — measured, not assumed

**Measured 2026-10-02.** These are boundaries of the deployed head on a specific host, found while
building a worked graph example. They are recorded because the first example failed silently: it
*ran*, its checks passed, and its answer was meaningless.

Unit of measurement: one question, one state, the `label`/`band`/`confidence` the head returned.
All figures come from live `decide` calls against the shipped binary. Re-run with
`scripts/check-wc-boundary.py` (thresholds) — 15/15 as of this writing.

---

> **SUPERSEDED IN PART — 2026-10-02.** An independent review re-ran this grid **through
> `laya graph run`** and found the opposite: a 2-option `choice` scores **15/15 under the v1
> phrasing** ("The word count is N."), and the boundary failures at 301/101 came from **the v14c
> phrasing** chosen later in the worked example, not from the shape. So the sentence *"the engine's
> only reachable question form is the weaker one"* in `OPUS`-reviewed material is **withdrawn**.
> What survives: `noul`'s advantage is **confidence** (0.875–1.0 vs 0.35–0.54 at boundaries), which is
> what makes band-gating usable — it does not win on label correctness. And `noul` only holds that
> advantage through `decide`'s state envelope: the same `noul` through `ask` with file state was
> 13/15 and got `5 > 300` wrong at confidence 0.84. Any claim that "noul tracks the count" must name
> the path that builds the state. See `OPUS-REVIEW-GENERIC-GRAPHS.md` §5.

## 1. A 3-option `choice` does not track a number. A `noul` threshold does.

The failing shape, and the fix, measured on the same head:

| question shape | 13 words | 412 words | verdict |
|---|---|---|---|
| `choice` "SHORT / MEDIUM / LONG" | MEDIUM | MEDIUM | **does not track** — conf 0.09–0.26 |
| `noul` "greater than 300?" | no (1.0) | yes (0.9189) | **tracks**, incl. the 300/301 boundary |
| `noul` "greater than 100?" | no (1.0) | yes (1.0) | **tracks**, incl. the 100/101 boundary |

Across 13 → 1000 words the 3-option label was **MEDIUM every time**, so a 77× change in the number
produced no change in the answer. The `noul` thresholds were **15/15 correct** over
5/13/50/100/101/150/299/300/301/412/1000/5000, including both exact boundaries.

**A 3-way classification on this head must be built from TWO threshold questions**, chained. That
is what `wc-classify-chained.flow.json` does.

## 2. The dangerous one: a `noul` on a SUBJECTIVE question returns a confident constant

This is worse than the 3-option case, because the 3-option case at least reported
`band=low` — the head said it did not know. This one is **confidently wrong**:

| text | question | answer | confidence |
|---|---|---|---|
| "This update is fantastic — exactly the fix I needed, and it works perfectly." | `choice` positive/negative | **positive** | 0.8571 |
| "This broke everything and I am furious. Nothing works." | `choice` positive/negative | **negative** | 0.9140 |
| "This update is fantastic — exactly the fix I needed, and it works perfectly." | `noul` "Is this feedback positive?" | **no** | **1.0** |
| "This broke everything and I am furious. Nothing works." | `noul` "Is this feedback positive?" | no | 1.0 |

The bare `noul` answered **`no` at confidence 1.0 for both** — positive text and negative text. It is
a constant, and it reports maximum confidence. The two-option `choice` on the same text was correct
both times.

**The pattern that fits every measurement above:** the head answers reliably where the state
**states the fact** ("The word count is 13" → "greater than 300?"), and degenerates where the answer
requires an **implied or subjective judgement** ("is this feedback positive?"). Two independent
observations support this:

- A `noul` threshold over an explicit numeric statement: correct, including the boundary.
- A `noul` over subjective sentiment with no explicit marker: a confident constant.

## 3. This alignment already exists in the code — and it is the right response

`crates/laya-mcp/src/lib.rs:1150` records the same class of failure for the gate:

> THE MODEL MAY ONLY DOWNGRADE. MEASURED 2026-09-30: this head cannot classify command danger —
> "is this action safe?" as a `noul` scored 6/12, with `ls -la /tmp` and `rm -rf <dir>` both coming
> back ~0.85–0.89.

"Is this action safe?" is subjective in exactly the way "is this feedback positive?" is. The gate
does not fix the head; it **bounds the damage**: the model may only downgrade a verdict, and `allow`
additionally requires a positive allowlist match. `AGENTS.md` rule 5 calls a model gate *one signal*.

**So the correct handling of a confident constant is not a better prompt. It is not to let one
signal decide.** Where this head is used to select a branch, the branch must be selected on a fact
the state states explicitly, or the verdict must be bounded by a deterministic companion.

## 4. What this does NOT establish

- **Only one host, one head build.** These are measurements of the deployed artifact, not a claim
  about the architecture or about a different checkpoint.
- **"Subjective vs explicit" is a hypothesis that fits the data, not a proven mechanism.** It is
  stated as a hypothesis because it is one; the measurements above are the evidence, and two
  independent cases support it. A tokenizer-level explanation is plausible and **untested**.
- **The bare-`noul` constant was probed on 2 states.** That is enough to establish it is not
  tracking those two, not enough to characterise where its boundary lies. A wider sweep is the
  obvious next measurement.
