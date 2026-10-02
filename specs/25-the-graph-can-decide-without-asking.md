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
| `the_control_a_decide_still_works` | an existing decide flow | unchanged behaviour (NF3) |

The last four rows are the ones that catch the real mistakes: a `check` whose failure stops the run is
indistinguishable from a broken node, and a `check` that calls the model is not a `check`.

## Exit Criteria

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_check_takes_the_pass_port_on_exit_zero 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_failing_check_does_not_fail_the_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_check_whose_command_cannot_be_executed_fails_the_node 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib no_model_is_called_by_a_check 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [ ] **every committed flow still validates AND runs**, judged by the report (`git status` clean of `^ D`)
- [ ] `cd /projects/laya-rs && cargo test --workspace` : 0 failed
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0 diagnostics
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` : clean
- [ ] `cd /projects/laya-rs && python3 scripts/check-doc-citations.py` : every citation resolves
- [ ] `cd /projects/laya-rs && python3 scripts/regen-index.py` : 0 broken links

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
