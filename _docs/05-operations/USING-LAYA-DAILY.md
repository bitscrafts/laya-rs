# USING LAYA GRAPH DAILY — the spec gate, and what it saves

**Measured 2026-10-02**, on this project, with the installed `laya`. Every number below came from a run,
not from reading code.

## The problem this solves

Deciding "is this spec finished?" is a **comparison**, and until spec 25 the graph could only ask the
model — which was measurably wrong at exactly that boundary (asked whether unfinished criteria equalled
zero with **one** open, it answered `yes` at band low and would have released with work outstanding).

Now it is a `check`: a command's exit status picks the branch, **with no model in the loop**.

## The daily commands

```sh
# one spec, one compact line, exit status is the answer
cd /projects/laya-rs && scripts/spec-gate.sh specs/25-the-graph-can-decide-without-asking.md
    -> 25-the-graph-can-decide-without-asking.md: DONE 9/9 criteria passed      exit 0
    -> 26-the-examples-are-run-by-the-suite.md: NOT DONE 6/7 passed, 1 failed   exit 1

# the whole spec set, one compact line
cd /projects/laya-rs && scripts/spec-all-gate.sh
    -> spec set: ALL DONE (33 specs)                                            exit 0
    -> spec set: 2 NOT DONE of 33; open: 16 18                                  exit 1

# and the same decision, made by a GRAPH (routing, skipped nodes, a report)
cd /projects/laya-rs && laya graph run _docs/02-guides/spec-gate.flow.json
```

`spec-gate.flow.json` is the graph form: one `check` node runs `scripts/spec-all-gate.sh` and routes to
`release` / `work_pending` / `escalate`.

## What it costs, and what it saves

**The point of the compact line is the token cost.** A spec sweep that prints every criterion produces a
wall of text; an exit status plus one line is a handful of tokens and carries the same decision.

| what you ask | what comes back | roughly |
|---|---|---|
| `spec-gate.sh <one spec>` | one line, ~60–100 bytes | **~25 tokens** |
| `spec-all-gate.sh` | one line, ~40–60 bytes | **~15 tokens** |
| `laya graph run spec-gate.flow.json` | JSON report, 4 records | **~200 tokens** |
| the same check as a full `verify-all` transcript | every criterion echoed | **thousands of tokens** |

**The saving is not the graph — it is the compact verdict.** A model that needs "is it done?" should get
one line and an exit status, not a transcript it must read to work out the same thing. The graph adds
structure (which branch, what was skipped, a report); the shell script is what makes it cheap.

## The honest limits, measured — and the FAST mode they produced

**A full sweep is SLOW, and the reason is not the graph.** Measured: a whole-set sweep ran past **twelve
minutes and had not finished**, because **four specs' criteria invoke the mutation gate at roughly twelve
minutes each** (specs 09, 13, 17, 23). My first FAST filter matched only the wrapper script name and
**missed the four specs that call `cargo mutants` directly** — so FAST mode was not fast at all, which is
how the gap was found.

`FAST=1` (the default for `spec-all-gate.sh`) now skips every form of that criterion and **says so in its
line**, so the answer never hides what it did not run:

| command | measured |
|---|---|
| `FAST=1 scripts/spec-gate.sh <spec>` | **3–4 seconds** (73s for the one spec with a workspace run) |
| the same spec without FAST, when it has a mutation criterion | **~12 minutes** |

**A quality check is not needed to answer a completeness question.** Run the full form before a release;
run FAST while you work.

**The graph does not replace the worker.** It answers *whether* a spec is done; it does not write code.
Implementing still belongs to the dispatch scripts (`run-spec-part.sh`), which loop in bash — a graph
cannot loop.

**Where a decision is a judgement, the graph still asks the model** — and that is the right use of
`decide`. The rule: **comparison → `check`, judgement → `decide`.**

| use | node kind |
|---|---|
| "do all criteria pass?", "> 300 words?", "is it committed?" | **`check`** — deterministic |
| "is this change risky?", "does this read as a refusal?" | **`decide`** — the model is good at this |

## What a graph still cannot do

- **Iterate** — no loops; unroll, or drive the loop in bash.
- **Compose** — `subflow` needs a library the CLI does not load; inline the child's nodes.
- **Pass data shell-to-shell** — use a file; `{{x}}` substitutes in a `decide`'s `prompt` only.

## The two things this work found by being used

Running the gate for real immediately surfaced three defects, which is the argument for using it:

1. **A stale criterion in spec 13.** It pinned the *ignored* count in an acceptance test
   (`…; 0 failed; 2 ignored`). Three more tests had since been marked ignored, so the count was now 5 and a
   criterion that tested a true property failed on an incidental number. Now it pins the property.
2. **A stale criterion format.** My first fix put the correcting note *inside* the backtick pair that
   delimits a criterion, so the parsed command became `… | grep -qE '…' _(CORRECTED …` — a shell syntax
   error. **An annotation belongs outside the delimiters**; the parser captures between the first and
   second backtick on the line.
3. **The example runner caught a new flow.** The moment `spec-gate.flow.json` was added, spec 26's runner
   failed with *"a committed flow has no expected outcome"* — exactly its job. An operator flow that reads
   live state cannot be asserted hermetically, so it is now an **explicit exemption** with its own test
   that fails if the exempted file disappears. The list cannot silently grow.

## The one thing nobody has done yet

**No laya graph has driven real spec work end to end.** Every piece is verified — `check` routing, the
exit-status decision, running a real spec's criteria (measured: spec 25's first three criteria run as
graph nodes and route to `all_done`) — but that combination has not yet been the thing that actually
gated a commit. Treat this as *ready to trial*, not *battle-tested*, until it has.

## See also

- `_docs/03-reference/GRAPH-SPEC.md` — the flow format
- `_docs/03-reference/GRAPH-DETERMINISM.md` — `check` vs `decide`, with the measured failures
- `_docs/02-guides/GRAPH-EXAMPLE-WORDCOUNT.md`, `GRAPH-EXAMPLE-GUESSING.md` — worked examples
- `scripts/verify-all.sh` — the same job done in Python today; the graph is the candidate replacement
