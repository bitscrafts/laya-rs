# Spec 18 — `{{node_id}}`: a shell's output can inform a decision

## Overview

A `decide` node's state is a **literal string compiled into the flow file** (`lower.rs:104` copies
`node.prompt` verbatim; the word `prompt` appears once in that file). Nothing computed, read or
received at run time can inform a decision. That is the gap that makes graphs **specific** rather than
**general** — every decision was already answered by hand when the file was written.

This spec adds the smallest mechanism that closes it: **one restricted placeholder, `{{node_id}}`**,
meaning *the trimmed stdout of a node in this decide's `depends_on`*. Not an expression language.

**Project**: `/projects/laya-rs`
**Phase**: 18 — graphs become general
**Depends on**: spec 17 (a scheduler that honours `depends_on` before a decide runs).
**Source**: `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md` R2.

## Why a placeholder and not an expression evaluator

**If the graph can evaluate `count > 300`, Laya is not needed for that decision.** The evaluator
would quietly *become* the decision layer, outside the measured head, and an injection surface
besides. The placeholder keeps the head as the only thing that decides, and it keeps project rule 2
(Laya supplies a value, the graph owns the edge): the worst a hostile stdout can do is pick another
**declared** port, which is exactly what a hostile literal prompt can do today.

**Why not a bare `prompt_from: <id>`.** A review measured that the head is only reliable on a
measured sentence shape — "The word count is N." scored 15/15 while raw `wc` output and
file-wrapped state answered **wrong**. A placeholder lets the flow own the phrasing and the shell
emit only the value (`wc -w < f`). `prompt_from` would push phrasing into `echo`s in the shell.

## Requirements

### R1 — `{{node_id}}` is substituted at dispatch
When the scheduler is about to run a lowered decide node, each `{{id}}` in its `prompt` is replaced
by the **trimmed stdout** of the node `id`, whose record is already available because spec 17 made
the edge ordering real. Substitution happens **once**, just before the call.

### R2 — `validate` refuses a reference it cannot honour, by name
Each `{{id}}` must name a node in **this** decide's `depends_on`. Refuse, naming the offending id and
the node, when:
- the id names no node in the flow (`MissingPlaceholderTarget`);
- the id names a node **not** in this decide's `depends_on` (it would not have run yet);
- the id names this decide itself.

A silent empty substitution is the failure mode this spec exists to prevent: a decision made over an
empty string looks exactly like a decision made over real data.

### R3 — a failed or skipped referent skips the decide, and says so
If any referenced node failed, was skipped, or produced no record, the decide is **skipped**. **This includes a referent with `allow_failure: true` that failed** -- the decide is skipped either way, because a placeholder with no value must never be substituted as empty — it is
never asked over an empty string. The decide appears in `RunReport.skipped` (spec 17, R3) with a
reason. Never substitute `""`.

### R4 — the substituted value is bounded
The substituted text is capped at a documented limit (the head's state limit; find the real number in
the code or the MCP layer rather than inventing one) and the cap is stated in the refusal or the doc.
An unbounded substitution turns a large file into a truncated-anyway call; state what happens.

### R5 — the contract is documented where a flow author looks
`_docs/03-reference/GRAPH-SPEC.md`: the placeholder syntax, that it must name a `depends_on` entry,
what happens on failure, the cap, and the honest note that a literal prompt containing `{{` is now
ambiguous — which is why the refusal in R2 exists.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_shell_output_reaches_the_decision_and_changes_the_branch` | a shell `echo`ing a value, a decide whose prompt is `The value is {{n}}. Is it over 100?` | the substituted prompt is what the head sees, and the branch follows the value |
| `the_control_the_same_flow_with_the_other_value_takes_the_other_branch` | the same flow, the shell emitting a different number | a DIFFERENT branch — without this, a hardcoded substitution passes row 1 |
| `an_unlisted_reference_is_refused_by_name` | `{{x}}` where `x` is not in `depends_on` | `FlowError` naming `x` and the node |
| `an_unknown_node_in_a_placeholder_is_refused_by_name` | `{{nosuch}}` | `FlowError` naming `nosuch` |
| `a_failed_referent_skips_the_decide_rather_than_asking_over_empty` | the referenced shell exits 1 | the decide does not run, and appears in `skipped`; **the head is never called** |
| `the_control_a_successful_referent_does_not_skip` | the same flow with a succeeding shell | the decide runs normally (near-miss: the skip rule must not fire on success) |
| `a_literal_prompt_without_placeholders_is_unchanged` | an existing committed fixture | the wire payload is byte-identical to before |

The last row is the compatibility guarantee: R1 must not alter a flow that uses no placeholder.
**Verify it can fail** by making an empty-string substitution unconditional in the literal path,
watching it go red, and restoring.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_shell_output_reaches_the_decision_and_changes_the_branch 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_the_same_flow_with_the_other_value_takes_the_other_branch 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib an_unlisted_reference_is_refused_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_failed_referent_skips_the_decide_rather_than_asking_over_empty 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_literal_prompt_without_placeholders_is_unchanged 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] **a real end-to-end `wc` example that genuinely depends on `wc`** exists and is run: write `/projects/laya-rs/_docs/02-guides/wc-classify-measured.flow.json` where the shell emits ONLY the number (`wc -w < file`) and the decide's **`prompt` is the state** `The word count in the file is {{count}} words.` and its **`question` is separate**: `Is it greater than 300?`. Do NOT put the question in the prompt -- `prompt` is the state handed to the head, `question` is what it is asked. Run it on both documents and paste both reports showing the branches **differ**.
- [ ] `cd /projects/laya-rs && laya graph run _docs/02-guides/wc-classify-chained.flow.json 2>&1 | grep -q selected_port` : the older committed example still runs
- [ ] `cd /projects/laya-rs && cargo test --workspace`
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves

The end-to-end criterion is the one that matters: **a `wc` whose value actually drives the branch**,
which no flow in this repo can do today. If the head answers wrongly on the substituted shape, that
is a FINDING — report it, including the confidence and band, rather than tuning the phrasing until
it passes.

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency. No expression evaluator: **the placeholder resolves to a value
  and nothing else.** If you find yourself parsing arithmetic, stop — that is out of scope by design.
- Do NOT change a flow that uses no placeholder.
- Do NOT substitute an empty string on a failure path. Skip, and name the node.
- Do NOT modify `specs/`.
- **Run graphs, do not only read code** (user instruction: *"graph must be ran from laya!"*).
- Do not delete anything; `mv` aside and say so.
