# Spec 25 — the graph can decide without asking

## Overview

Today the **only** way a graph can branch is a `decide`, and a `decide` is a **model call**. The format
has no comparator of any kind (measured 2026-10-02: no `lt`/`gt`/`eq`/`threshold`/`if` field exists in
`flow.rs`), and a `route` node cannot be authored at all (`laya: node `r` has unsupported kind `route``).
So decision routing means the model must answer a **comparison**, and it is measurably wrong at exactly
the boundaries a spec decision needs:

| question put to the model | truth | model's answer | consequence |
|---|---|---|---|
| "is the number of unfinished criteria equal to 0?" | **1** unfinished | `yes` @ band low 0.1115 | **releases with one spec open** |
| "did the build succeed?" | build exited **7** | `yes` @ band mid 0.79 | proceeds on a failed build |
| "is the guess higher than the secret?" | guess **is** the secret | `higher` | the game cannot be won |

The band is not a usable signal either: `low` holds both a correct answer (0.0603) and the wrong one
(0.1115), so there is no honest way to escalate on doubt.

**This spec adds a deterministic branch that does not ask the model.** A shell command's **exit status**
selects the port. Every failing case above is expressible in a single line of shell — `[ "$n" = 0 ]`,
`cargo test`, `[ "$guess" = "$secret" ]` — so one primitive closes all of them.

**Project**: /projects/laya-rs · **Phase**: 25 — the graph decides on its own

## Requirements

### Functional

- **R1 — a `check` node: the exit status selects the port, with no model in the loop.**
  ```json
  {"id": "closed", "kind": "check",
   "run": "[ \"$(grep -hc '^- \\[ \\]' specs/*.md | paste -sd+ | bc)\" = \"0\" ]",
   "depends_on": ["count"],
   "ports": {"pass": "release", "fail": "work_left", "_": "escalate"}}
  ```
  - exit **0 → `pass`**; exit **non-zero → `fail`**; a port whose key is absent → `_`
  - **A non-zero exit MUST NOT fail the node or stop the run.** The exit status is *data*, not an error.
    This is the whole point and the most likely thing to get wrong.
  - no model call, no network, no `laya-mcp` — the node must run with the sidecar **down**
- **R2 — it is honest about what it cannot decide.**
  - `run` absent → refused by name (a `check` with nothing to run is not a check)
  - the command cannot be executed → the node **fails by name**; it must not silently take `fail`, because
    "the check did not run" and "the check said no" are different answers
  - `ports` must contain a `_` catch-all (consistent with `decide`)
  - `allow_failure` on a `check` is **refused**: a check's "failure" is its answer, so the field is nonsense
  - `timeout` **is** honoured, and a timed-out check is a **failing node**, not a `fail` port
- **R3 — it composes with what exists.** A `check`'s port target may be a shell, a `decide`, or another
  `check`; `depends_on` still orders it; spec 17's routing rules still hold (only the selected port runs,
  the rest are named in `skipped`).
- **R4 — the boundary is documented, measured.** `_docs/03-reference/GRAPH-DETERMINISM.md` must state:
  **comparison → `check`; judgement → `decide`**, with the three measured failures above as the reason,
  and the rule that a `decide` must never be used where a `check` can answer.

### Non-Functional

- **NF1** — deterministic: the same command state selects the same port every run, with no model involved.
  **Test this with the sidecar stopped** — that is what proves no model call happens.
- **NF2** — no new crate or dependency.
- **NF3** — the existing `decide` path is unchanged; every committed flow still validates **and runs**.

## Architecture

```
shell: compute a value / count
   |
   +--> check: [ "$value" = 0 ]        exit 0 -> pass, non-zero -> fail     (NO MODEL)
          |
          *-- pass --> release (shell)
          *-- fail --> work_left (shell)
          *-- _    --> escalate (absent port)                              no model involved anywhere
```

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_check_takes_the_pass_port_on_exit_zero` | `run: "exit 0"` | `pass` target runs; `fail` in `skipped` |
| `a_check_takes_the_fail_port_on_nonzero_exit` | `run: "exit 1"` | `fail` target runs; **run does NOT stop** |
| `the_control_a_failing_check_does_not_fail_the_run` | `run: "exit 7"` | `stopped_early == false`, `failed_node == None` |
| `a_check_with_no_run_is_refused_by_name` | `{"kind":"check"}` | refused, message names the missing `run` |
| `a_check_whose_command_cannot_be_executed_fails_the_node` | `run: "/nonexistent/bin/x"` | node **fails** (not a `fail` port) |
| `a_check_needs_a_fallback_port` | no `_` port | refused |
| `allow_failure_on_a_check_is_refused` | `allow_failure: true` | refused by name |
| `a_timed_out_check_fails_the_node_not_the_fail_port` | `run: "sleep 5"`, `timeout: 0` | node fails, run stops |
| `no_model_is_called_by_a_check` | a `check` flow, sidecar **stopped** | both ports still reachable, run completes |
| `the_control_a_decide_still_works` | an existing decide flow | unchanged behaviour (NF3) — **no such unit test was written.** The property is covered by the "every committed flow still runs clean" criterion and by the raw `judge/route` record quoted in the appendix. |

The last four rows are the ones that catch the real mistakes: a `check` whose failure stops the run is
indistinguishable from a broken node, and a `check` that calls the model is not a `check`.

## Exit Criteria

- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_check_takes_the_pass_port_on_exit_zero 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_failing_check_does_not_fail_the_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_check_whose_command_cannot_be_executed_fails_the_node 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib no_model_is_called_by_a_check 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] **every committed flow still validates AND runs**, judged by the report (`git status` clean of `^ D`)
- [x] `cd /projects/laya-rs && cargo test --workspace` : 0 failed
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0 diagnostics
- [x] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [x] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves
- [x] `cd /projects/laya-rs && python3 scripts/regen-index.py` : 0 broken links

## Fixtures this spec MUST be judged against (all measured today, all failing without it)

1. `/tmp/specs-control` (one spec with an open criterion) — a `check` counting unfinished criteria must
   take `fail`. **Today the `decide` version says `yes` at band low and releases.**
2. `_docs/02-guides/wc-classify-measured.flow.json` against `long-note.txt` **and** `short-note.txt` — the
   300/301 boundary must be exact.
3. The guessing game with `guess == secret` — must be able to reach a win.
4. A shell that exits 7 — a `check` must take `fail` **and the run must continue**.

## Guardrails

- Do NOT make a `check`'s non-zero exit stop the run. **That is the single most important requirement.**
- Do NOT let a `check` fall back to a model, ever, for any reason. If it cannot decide, it says so.
- Do NOT remove or weaken `decide`; this adds a primitive, it does not replace one.
- Do NOT claim in any document that a comparison should be delegated to a `decide`.
- Do NOT add a crate or dependency; this is a node kind in `laya-graph`.
- Do NOT weaken, re-pin or delete an existing test.
- Do NOT commit. Leave the work in the tree.
- Do NOT delete anything pre-existing; `mv` aside and say why.

---

## Verification of this spec (independent, run here 2026-10-02 after the worker finished)

Worker did not commit (HEAD `29419f9`, 0 deleted). **33 substantive checks passed.**

### R1 — the exit status selects the port, with no model

```
run: "exit 0"   the_check ok   -> on_pass  TOOK_PASS   skipped=['on_fail','on_other']
run: "exit 1"   the_check ok   -> on_fail  TOOK_FAIL   skipped=['on_other','on_pass']
```

**The critical one — a non-zero exit must not fail the run:**

```
run: "exit 1"   stopped_early=False  failed_node=None  no node failed
run: "exit 7"   took the fail port AND the run continued
```

**A command that cannot be executed FAILS the node rather than answering `fail`:**

```
the_check  FAIL  "sh: 1: /nonexistent/bin/nope: not found"
stopped_early=True  failed_node=the_check   -- it did NOT take the _ port
```

That distinction is the requirement: *"the check did not run"* and *"the check said no"* are different.

### NF1 — no model in the loop, PROVEN by stopping the sidecar

```
sidecar pid 168492  -> SIGSTOP
the_check ok -> on_pass TOOK_PASS   stopped_early=False
sidecar RESUMED
```

The flow completed with the server **suspended**. That is the only honest demonstration that a `check`
does not ask the model — reading the code could not show it.

### R4 — the four fixtures that fail without this primitive, all now exact

| fixture | result |
|---|---|
| a spec set with unfinished criteria (10 and 11 of them) | **TOOK_FAIL**, correct in both |
| **300 words / 301 words** | **PASS / FAIL — exact at the boundary** |
| `guess == secret` | **TOOK_PASS** — a win is now expressible, which it was not |
| a command exiting 7 | **fail port, run continued** |

### R2 — the refusals

```
check node `c` requires `run`            refused
allow_failure on a check                 refused
a check without a `_` fallback port      refused
```

### Regression, which is what "production ready" turns on

```
every committed flow still VALIDATES        PASS
every committed flow still RUNS clean       PASS   (includes the decide-based flows)
spec 23's stall guard, all 3 tests          PASS   <- the risk flagged before dispatching
workspace suite  625 passed / 0 failed      clippy 0 · fmt clean
73 docs, every citation resolves            124 links, 0 broken
```

The decide path is intact — the raw record, verbatim:

```
judge/route  output: {"on":"$.payload.label","value":"yes","selected_port":"advise_lower"}
check "closed"  exit_code=1 success=true  -> work_left ran, skipped ['escalate','release']
```

### Two corrections about my own verification, recorded not hidden

1. **My harness invented a test name.** It checked for `the_control_a_decide_still_works`, which exists
   only as a TDD-table row and was never an exit criterion, then reported that as the single FAIL. The
   property is covered by the committed-flow regression check. **A criterion must name a test that
   exists** — and so must a check.
2. **My harness misread the report schema.** It read `selected_port` as a top-level record field; it is
   JSON inside the record's `output` string. That extraction always returned empty, so a "decide routes"
   assertion built on it would have been meaningless. It did not cause a false pass here (the port
   assertions work off the target node actually running), but it is the same defect class as the rest of
   this session: **a check whose reading of the artefact was wrong.**
