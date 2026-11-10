# Spec 24 — the guessing game runs on the graph
#: depends=- status=- block=-

## Overview

A worked example of the graph driving a **guessing game**: a shell node produces the facts of a round, a
decision node asks the head to judge them, and the graph routes on that judgement to the advice a player
would be given ("go higher" / "go lower"). It is the second end-to-end example beside the word-count one,
and it is chosen because it exercises the surface *through the interface a person would actually use*:
state from a command reaches a decision, and the decision's own answer selects which branch runs.

It also makes the engine's central limitation **explicit rather than surprising**: there are no loops.
A cycle is refused (`RunError::Cycle`, measured 2026-10-02), `retry` is refused, and no iteration
construct exists. So **the graph plays exactly one round.** Where the iteration lives is a design
decision this example must state, not hide.

**Project**: /projects/laya-rs · **Phase**: 24 — the graph as a usable surface

## Requirements

### Functional

- **R1 — a round is a graph, not a script.** `_docs/02-guides/guess-the-number.flow.json`:
  - `setup` (shell) writes the round's secret and guess and prints them as one line
  - `judge` (decide) whose `prompt` **references `{{setup}}`** — the head judges the round
  - `judge/route` on `label`, with ports `yes` / `no` and a `_` catch-all
  - `advise_lower` (shell) on `yes`, `advise_higher` (shell) on `no`, both converging on `emit_move`
- **R2 — the decision is driven by the command's output, not by a literal.** The prompt must contain the
  placeholder. A prompt with the numbers hardcoded is the defect this example exists to avoid (it is
  present in earlier committed examples, measured 2026-10-02, and must not be repeated here).
- **R3 — the control flow proves the branch is real.** `guess-the-number.control.flow.json` runs the same
  graph against a guess on the **other side** of the secret, and the two runs must reach **different**
  advice nodes. A committed control is required: without it, "the branch works" is unfalsifiable.
- **R4 — the no-loop boundary is documented, measured.** `_docs/02-guides/GRAPH-EXAMPLE-GUESSING.md` must
  state plainly: one round per run; a cycle is refused (name the error and the measured date); and give
  the two honest ways to play N rounds — instantiate N copies, or drive the loop outside the graph.
  **Do not claim the graph loops.**
- **R5 — both documents run from the CLI.** `laya graph validate` and `laya graph run` must succeed on
  both flows with the **freshly built** binary, and the run report must show the two flows reaching
  different advice nodes.

### Non-Functional

- **NF1** — deterministic: the round's numbers come from the flow's own shell command, not from the
  clock. Re-running a flow must produce the same branch every time.
- **NF2** — no new crate or dependency.
- **NF3** — the example must not require `laya-mcp` on `PATH` (spec 22 resolves it as a sibling).
- **NF4** — the documents it reads must be **in the repository** (a `_tmp` path is gitignored scratch and
  is absent in a clone; measured 2026-10-02 — this was a real defect in the word-count example).

## Architecture

```
setup (shell: secret + guess -> one fact line)
   |
   +--> judge (decide, prompt contains {{setup}})      <- the head judges the round
          |
          *-- yes --> advise_lower (shell) --+
          *-- no  --> advise_higher (shell) -+--> emit_move (shell, prints the move)
          *-- _   --> escalate  (unparsed answer is named, not silently routed)
```

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `the_prompt_references_the_fact_node` | the flow JSON | `{{setup}}` present in the decide's `prompt` (R2) |
| `the_two_flows_reach_different_advice` | both flows, run | run reports name **different** advice nodes (R3) |
| `the_control_names_the_other_branch` | control flow | `skipped` contains the branch the first flow took |
| `the_round_is_deterministic` | one flow, run twice | identical advice node and identical skipped set |
| `a_cycle_is_still_refused` | the 3-node cycle probe | error names the path `a -> c -> b -> a` (R4) |

The last row is the **control on the claim in R4**: the example documents that loops are refused, so a
test must show the refusal still fires. A document asserting a limitation nobody re-checks is how a
limitation quietly stops being true.

## Exit Criteria

- [x] `./target/debug/laya graph validate _docs/02-guides/guess-the-number.flow.json` : exit 0
- [x] `./target/debug/laya graph validate _docs/02-guides/guess-the-number.control.flow.json` : exit 0
- [x] `./target/debug/laya graph run _docs/02-guides/guess-the-number.flow.json` : exit 0, and the report's `records` show `setup` ok and every node ok
- [x] `grep -c '{{setup}}' _docs/02-guides/guess-the-number.flow.json` : at least 1 (R2)
- [x] `python3 scripts/check-guess-branches.py` : prints `BRANCHES DIFFER` (R3)
- [x] `./target/debug/laya graph run _docs/02-guides/guess-the-number.flow.json` twice : the advice node named in both reports is identical (NF1)
- [x] `! grep -n '_tmp' _docs/02-guides/guess-the-number*.flow.json` : every path the flows read lies inside the repository (NF4)
  _(CORRECTED 2026-10-02: required a `guess-the-number*/` DIRECTORY that never existed -- the flows read committed files directly. The assertion was wrong about the artefact.)_
- [x] `! grep -n '_tmp' _docs/02-guides/guess-the-number*.flow.json` : **no matches** (NF4)
  _(CORRECTED 2026-10-02: without the `!` this returned grep's exit status, which is 1 exactly when there are NO matches -- the criterion failed when it should have passed)_
- [x] `cargo test --workspace` : 0 failed
- [x] `python3 scripts/check-doc-citations.py` : every citation resolves
- [x] `python3 scripts/regen-index.py` : 0 broken links

## Guardrails

- Do NOT add a loop, a cycle, `retry`, or any iteration construct to make the game play many rounds.
  **The refusal is correct and the example must respect it.** N rounds = N instantiations, or outside.
- Do NOT hardcode the round's numbers in the decide's prompt. Use the placeholder (R2).
- Do NOT claim in any document that the graph loops, retries, or plays until it is won.
- Do NOT put an example document under a path outside the repository (NF4).
- Do NOT weaken or re-pin an existing test to make this pass.
- Do NOT touch `crates/` unless a criterion cannot pass without it — and if it cannot, say so in the
  report rather than widening the change silently.
- Do NOT commit. Leave the work in the tree.
- Do NOT delete anything pre-existing; `mv` aside and say so.

---

## Verification of this spec (run here, 2026-10-02, after the worker finished)

### 0. The worker did not commit
HEAD `237cf08`, deleted tracked files **0**.

### 1. Both flows validate and run, every node ok

```
guess-the-number.flow.json          setup  secret 41 / guess 57
  setup         ok   The secret is 41. The guess is 57.
  judge         ok   label=yes band=low conf=0.2484
  judge/route   ok   value=yes -> selected_port=advise_lower
  advise_lower  ok   ADVICE: the guess is too high -- go lower
  emit_move     ok   MOVE: the round is judged ...
  failed: NONE   stopped_early: False   not_started: []

guess-the-number.control.flow.json  setup  secret 41 / guess 3
  judge         ok   label=no  band=low conf=0.2680
  judge/route   ok   value=no  -> selected_port=advise_higher
  advise_higher ok   ADVICE: the guess is not higher -- go higher
  failed: NONE   stopped_early: False   not_started: []
```

### 2. R2 — the prompt references the node, it does not state a literal

```
decide prompt : '{{setup}}'
decide question: 'Is the guess higher than the secret?'
setup run     : echo "The secret is 41. The guess is 57."
```

Both fields reach the head and do different jobs: `question` lowers to
`questions/answer/instructions` and `prompt` to `state` (confirmed in `mcp_client.rs` and pinned by the
wire test `every_decide_field_reaches_the_wire`). **The example does not repeat the defect the earlier
committed examples still carry** — their prompts state the value as a literal while the node that
computes it runs and is discarded.

### 3. R3 — the control genuinely controls

```
flow    reached: ['advise_lower']    skipped: ['advise_higher', 'escalate']
control reached: ['advise_higher']   skipped: ['advise_lower', 'escalate']
BRANCHES DIFFER        (exit 0)
```

The two runs differ only in the guess, and they take **opposite branches**, each naming the other as
skipped. Without this control, "the branch works" would be unfalsifiable.

### 4. NF1 / NF4 / R4

| requirement | measurement |
|---|---|
| NF1 deterministic | two runs → `advice=['advise_lower'] skipped=['advise_higher','escalate']` both times |
| NF4 in-repo paths | `_tmp` occurrences in the flows: **0**; the flows read nothing outside the repo |
| R4 no-loop claim still true | the 3-node cycle is still refused: `laya: dependency cycle: a -> c -> b -> a`, exit 1 |

R4 is the control on the document's own claim: the example says the engine has no loops, so a test must
show the refusal still fires. **A limitation nobody re-checks is how a limitation quietly stops being
true.**

### 5. The gate

`cargo test --workspace` **612 passed · 0 failed** · clippy **0** · fmt **clean** ·
`check-doc-citations.py` **71 docs, every citation resolves** · `regen-index.py` **124 links, 0 broken**.

### 6. What this example does NOT claim, and one honest observation

**It does not claim the graph loops.** It plays one round per run, and the document says so with the
measured refusal above.

**Observed, not a defect:** the head answers this question at **band `low`** (conf 0.2484 / 0.2680) in
both flows. The labels are still correct and the branches differ, so the example works — but the
confidence is low on a question this specific, and the earlier word-count measurements showed the head
stronger on a coarser comparison. Recorded because a future reader would reasonably want to know that
this example's routing rests on a low-confidence answer.

**Also observed:** on the `no` branch the advice node's text reads *"the guess is not higher -- go
higher"*, which is the **negation of the question** rather than a natural game hint. It is correct as a
consequence of the answer and the graph routing is right; the wording is what a reader may find awkward.
