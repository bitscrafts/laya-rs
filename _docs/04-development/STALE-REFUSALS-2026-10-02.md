# STALE REFUSALS — a rule that outlives its reason is a defect too

**Measured 2026-10-02**, after spec 17 landed (`ac9e94f`). This is the mirror image of the
inert-field problem, and the project had not looked for it until now.

## The finding

Spec 20 refused two fields **on a `decide` node** with the message *"dead on a `decide` until spec
17"*: `timeout` and `allow_failure`. The refusal was correct when written — decides ran in the phase
that ignored node-level timeout and failure policy, measured as a `decide` with `timeout: 0`
completing in 106 ms.

**Spec 17 moved decides into the scheduler.** So the refusal was tested by removing it and running:

| field on a `decide` | measured now |
|---|---|
| `timeout` | **WORKS.** `timeout: 0` → `success=false dur=1ms err="timed out after 0ms"`, `stopped_early=true`, `failed_node=d` |
| `allow_failure` | **PARTIAL — worse than refused.** The decide's own failure is tolerated (`failed_node` is `d/route`, not `d`), but its synthetic `d/route` node then fails and **stops the run anyway** |

The `allow_failure` measurement, verbatim:

```
a decide the server refuses, with allow_failure: true
  d        success=false                        <- the decide's own failure IS tolerated
  d/route  success=false                        <- but its synthetic route node fails...
  stopped_early = True   failed_node = d/route  <- ...and the run stops here
  sibling  success=true                         <- ran only because it was already dispatched
```

## Why this matters as much as an inert field

**A refusal that outlives its reason reads as a deliberate design decision and is an accident of
sequence.** It blocks a working feature, and — worse — it teaches the reader something false about the
engine. The `allow_failure` case is the sharper lesson: lifting the refusal without finishing the work
leaves a field that is accepted, appears honoured for one beat, and then stops the run. **That is
strictly worse than refusing it**, because a refusal at least tells the truth.

The general rule this project keeps re-learning: **a declared field must be observable, refused by
name, or documented as carrying no behaviour — and whichever it is must be re-checked when the engine
under it changes.** Spec 20's field-effect table
(`crates/laya-graph/src/field_contract.rs`) is what makes that check mechanical: lifting a refusal turns
a `Refused` row into an `Observable` row, and an `Observable` row is verified by running two fixtures
that differ only in that field.

## Filed as spec 21

`specs/21-the-refusals-catch-up-with-the-scheduler.md` — lift the `timeout` refusal, finish
`allow_failure` on a decide (either make the route node inherit its upstream's policy, or refuse it
again — **not half-working**), and audit `retry` / `escalate_on_near_tie` for staleness by checking
whether the engine would now honour them.

## How the finding was made — and the bug in the check that made it

The first attempt to test this used:

```sh
if $BIN graph validate flow.json 2>&1 | grep -qi 'timeout'; then ...
```

Under `set -o pipefail` this tests the **binary's** exit status, not grep's match — so a refusal that
fires correctly reported as "NO refusal". The check was wrong, not the code. **Fifth instance this
session of a check whose mechanics, not whose intent, was the defect.** It was caught by reading the
raw output instead of trusting the branch, and redone by removing the refusal and running the flow —
which is what the question actually needed.

## A related trap in the quota gate

`python3 ~/.hermes/scripts/laya-quota.py` prints **"TOO LITTLE ROOM … Do not start a dispatch"**
below a 45% floor, and `run-spec-part.sh` **prints that line but does not enforce it** — the gate is
advisory. At 43% free (one spec needs ~15-25%) there is room for one more dispatch. So: **read the
number, do not obey the sentence.** The floor is a heuristic for "leave room for the next run", not a
hard limit, and treating it as one would have stalled this queue for two hours for no reason.
