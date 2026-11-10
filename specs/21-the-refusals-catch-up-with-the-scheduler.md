# Spec 21 — lift the refusals that spec 17 made stale, and finish the decide's failure policy
#: depends=- status=- block=-

## Overview

Spec 20 refused `timeout` and `allow_failure` **on a `decide`** with the message *"dead on a `decide`
until spec 17"*. Spec 17 has landed. **The timeout half of that refusal is now stale, and the other
half is half-done** — both proved by running, not by reading:

| the field on a `decide` | spec 20 | measured now |
|---|---|---|
| `timeout` | refused "until spec 17" | **WORKS.** `timeout: 0` → `success=false dur=1ms err="timed out after 0ms"`, `stopped_early=true`, `failed_node=d` |
| `allow_failure` | refused "until spec 17" | **PARTIAL.** The decide's own failure is tolerated (`failed_node` is `d/route`, not `d`, and an independent sibling ran) — but the synthetic `d/route` node then fails and **stops the run anyway** |

A refusal that outlives its reason is the same defect class as a field that does nothing: it reads as
a deliberate rule and is an accident of sequence. And `allow_failure` on a decide is now **worse than
refused** — it is accepted, looks like it is honoured for one beat, and then the run stops.

**Project**: `/projects/laya-rs`
**Phase**: 21 — the refusals catch up with the scheduler
**Depends on**: spec 17 (`ac9e94f`) — done.
**Source**: measured here on 2026-10-02; `specs/20-the-format-is-a-contract.md` row 8.

## Requirements

### R1 — lift the `timeout` refusal on a `decide`
Delete it from `validate.rs:193-206` and pin the behaviour with a test: **a decide with
`timeout: 0` must FAIL with a timed-out error**, not complete. (This is the probe that proved the
field works: `err="timed out after 0ms"`.) Keep the check that the field is *reachable* — a test that
would fail if the refusal came back.

### R2 — make `allow_failure` on a `decide` actually let the run continue, or refuse it again
Measured today, with the refusal removed:

```
a decide the server refuses, allow_failure: true
  d        success=false                        <- the decide's own failure IS tolerated
  d/route  success=false                        <- but its synthetic route node fails...
  stopped_early = True   failed_node = d/route  <- ...and the run stops here
  sibling  success=true                         <- this ran only because it was already dispatched
```

Two acceptable resolutions; **pick one and state why**:
- **(a)** a `Route` node whose upstream failed **propagates the upstream's `allow_failure`** — a route
  is bookkeeping for a decide, so it should not fail harder than the decide it routes; or
- **(b)** `allow_failure` on a decide is **refused again**, with a message saying the route node would
  still stop the run, so the field would mislead.

(a) is preferred if it can be done without giving routes their own failure semantics; (b) is honest and
cheap. **Do not leave it half-working**, which is the state today.

### R3 — audit the OTHER interim refusals for staleness
`retry` and `escalate_on_near_tie` are also refused. Neither has a successor spec yet, so neither is
stale — **verify that by checking whether the engine would now honour them**, and record the answer in
the spec. If any field the engine now honours is still refused, lift it in this spec. The message for
each remaining refusal must name **what it waits for**, not just "not implemented".

### R4 — the field-effect table reflects reality again
`crates/laya-graph/src/field_contract.rs` has a row per field saying whether it is `Observable`,
`Refused` or `Documentary`. Lifting a refusal turns a `Refused` row into an `Observable` one, and an
`Observable` row is **checked by running two fixtures that differ only in that field**. So this spec
is not done until the table agrees with the code — that is the mechanism working, not friction.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_decide_timeout_bounds_the_decide` | a decide with `timeout: 0` | it FAILS as timed out; `failed_node` names the decide |
| `the_control_a_decide_without_a_timeout_still_completes` | the same decide, no `timeout` | completes and routes (near-miss: the timeout must not be applied when absent) |
| `a_decide_with_allow_failure_does_not_stop_the_run` | a decide whose call is refused, `allow_failure: true`, plus an independent sibling | `stopped_early == false`, the sibling ran — **or**, if R2(b) is chosen, `validate` refuses the combination and names the reason |
| `the_control_a_decide_without_allow_failure_does_stop_the_run` | the same flow without `allow_failure` | `stopped_early == true` (the rule must not tolerate every failure) |
| `every_field_refusal_still_has_a_reason` | the field-effect table | no row is `Refused` for a field the engine now honours |

Two controls are mandatory and must be reported with output. The second row's control is the one that
distinguishes "the timeout works" from "every decide now fails".

## Exit Criteria

- [x] `cargo test -p laya-graph --lib a_decide_timeout_bounds_the_decide 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cargo test -p laya-graph --lib the_control_a_decide_without_a_timeout_still_completes 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cargo test -p laya-graph --lib a_decide_with_allow_failure_does_not_stop_the_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cargo test -p laya-graph --lib the_control_a_decide_without_allow_failure_does_stop_the_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cargo test -p laya-graph --lib every_row_holds 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] **run it through the CLI** (a graph claim must be run, not read): the decide-timeout flow exits non-zero with a timed-out error, and the same flow without `timeout` exits 0. Use `./target/debug/laya` — **the `laya` on `PATH` is a stale build and would pass vacuously.**
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings` : 0
- [x] `cargo fmt --all --check` : clean

Every criterion uses `-p <the owning crate>` and `ok\. 1 passed`. A criterion that accepts
`ok. 0 passed` is satisfied by a test name that matches nothing.

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- **Do NOT weaken a test to make the suite pass.** The refusal tests added by spec 20 assert the
  refusal text; lifting a refusal means **re-pinning** those to assert the new behaviour, with a
  comment naming this spec. Never delete them silently.
- Do NOT lift `retry` or `escalate_on_near_tie` unless R3 **measures** that the engine now honours
  them; if it does not, leave the refusal and say so.
- Do NOT change the wire contract or the payload shape.
- Do NOT modify `specs/`.
- **Run graphs, do not only read code** (operator instruction, verbatim: *"graph must be ran from
  laya!"*).
- Do not delete anything; `mv` aside and say so.

---

## Verification of this spec (run here, 2026-10-02, after the worker finished)

### R1 — the `timeout` refusal is lifted AND the field bites, through the CLI

```
$ ./target/debug/laya graph validate dt.flow.json          (a decide with "timeout": 0)
laya graph validate: /tmp/dt21.flow.json is valid           <- no longer refused

$ ./target/debug/laya graph run dt.flow.json
  d              success=False dur=1ms err="timed out after 0ms"
  stopped_early = True   failed_node = d

$ ./target/debug/laya graph run dt-none.flow.json          (the same flow, no timeout)
  ran: ['d', 'd/route', 'y']   stopped_early = False        <- the CONTROL: it routes normally
```

Before spec 17 this field was dead; before this spec it was refused as dead. **Now it is live and the
refusal is gone.** The control is what distinguishes "the timeout works" from "every decide fails".

### R2 — `allow_failure` on a decide: resolution (a), and the run now continues

```
$ ./target/debug/laya graph run af.flow.json   (a decide the server refuses, allow_failure: true)
  ran: ['d', 'd/route', 'sibling']
  stopped_early = False   failed_node = None
```

**Before: `stopped_early = True, failed_node = d/route`** — accepted, apparently honoured, then the
synthetic route node stopped the run anyway. The worker chose **(a)**: the route inherits its decide's
policy (`lower.rs`), so a route gains no failure semantics of its own, and a failed allow-failure route
selects no port so its targets are **skipped rather than run**.

### R3 — the audit, measured rather than assumed

| field | honoured? | action |
|---|---|---|
| `retry` | **NO** — `lower.rs` drops it, `sched.rs` has no retry logic | refusal **kept**; message now says *"until the scheduler retries a failed node … no spec has landed it"* |
| `escalate_on_near_tie` / `near_tie_port` | **NO** — neither lowering nor routing reads them | refusal **kept**; message already named what it waits for |

### R4 — the table, and a new structural guard

The `timeout`/`allow_failure` rows were already `Observable` (shell fixtures), and the decide-specific
refusal was never a row, so **no row changed** — the worker said so rather than inventing a change.
New: **`every_field_refusal_still_has_a_reason` requires every `Refused` row's message to contain
"until"** — i.e. a refusal must say what it waits for. That is a durable guard against exactly the
defect this spec exists to fix, and it caught a real gap: the `inputs` refusal lacked one.

### The worker found a subtlety worth keeping

With an **instant** scripted caller, `timeout: 0` never fires — the call finishes first, so the test
would have been **vacuous**. The decide tests use a 200 ms `SlowCaller`, like a real call. **A timeout
test against a caller that cannot be slow tests nothing.** A mutation of the new tolerate-branch made
`a_decide_with_allow_failure_does_not_stop_the_run` FAIL, then was restored.

### Gate

5/5 targeted tests · **600 passed, 0 failed** · clippy 0 · fmt clean · 65 docs resolve.
One workspace run showed a **known flake of the `sched.rs` timing family** (recorded in
`05-operations/TEST-HARNESS-FLAKE.md`); measured again here at **0 failures in 10 runs**.
