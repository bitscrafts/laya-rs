# laya-rs — spec-09: the scheduler keeps the promises spec 07 made — fail-fast, cycles, payload shape

- **Project**: `/projects/laya-rs`
- **Priority**: P1. These are correctness defects in landed code, reproduced through the public API. None of them is reachable through `laya graph run` on a *validated* flow except R1, and R1 is.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`REPORT.md` §1.2); reproduction `notes/repro-graph.txt` (scratch crate `repro-graph/`, outside the repo)
- **Depends-On**: none.

---

## Verified Premises

- `specs/07-graph-executor-part3.md:81-83` (R11): *"A failing node stops the run; **siblings not yet started are not started**."*
- `crates/laya-graph/src/sched.rs:207-226`: `spawn_ready` spawns **every** ready node into the `JoinSet` immediately. Concurrency is bounded later, inside `run_one` (`sched.rs:238`), by `fanout.acquire_owned().await`.
- `crates/laya-graph/src/sched.rs:180-189`: on failure, `stop_scheduling = true` stops *future* `spawn_ready` calls only. Nothing tells tasks that were already spawned, and are still **waiting on the semaphore**, that they must not start. `run_one` (`sched.rs:231-271`) checks no flag after acquiring its permit.
- **Reproduced** (`notes/repro-graph.txt`), `limit: 1`, five independent shell nodes, `a` = `sleep 0.2; exit 1`:
  `R11 siblings: stopped_early=true failed=Some("a") records_after_failure=["a", "b", "c", "d", "e"]`.
  All four siblings **started after the failure**. With `writes: true` shell nodes, that means four repository edits ran after the run had already failed.
- `crates/laya-graph/src/sched.rs:603-630`: the only R11 test uses a **chain** `n1 → n2 → n3`. There, `n3` cannot start because its indegree never reaches zero, so the test passes with or without fail-fast. It is the "a negative is satisfied by doing nothing" shape CLAUDE.md warns about.
- `crates/laya-graph/src/graph.rs:106-110`: `Graph`'s fields are `pub`, and `laya_graph::run` is `pub` (`lib.rs:56`). `sched.rs:139-148` validates edge endpoints, but nothing in `run` detects a cycle.
- **Reproduced**: a hand-built `Graph` with `p ⇄ q` plus an independent `z` returns `Ok`, `stopped_early=false`, `records=["z"]`. That is a partial run reported as complete, which is the failure mode R11's own text names (*"A partial result that looks complete is the failure mode this project keeps finding"*).
- `crates/laya-graph/src/route_exec.rs:266-276`: `extract_on_value` accepts only `Value::as_str`. A numeric, boolean or null `payload.<field>` becomes `RunError::NodeTaskFailed`, which **aborts `run_flow` with `Err`**. A failing `decide` *call*, by contrast, is data (`stopped_early: true`, R12). The same class of upstream fault is therefore reported two different ways.
- Mutation testing (`mutants/mutants.out/missed.txt`): `sched.rs:52:29 replace == with != in topo_order` is **MISSED**, so no test distinguishes a correct topological order from a broken one on a graph with a node of in-degree ≥ 2. Five more mutants in `sched.rs` (`:67`, `:84`, `:118`) were caught only by cargo-mutants' external **timeout**. No scheduler test bounds its own runtime.

---

## Overview

Spec 07 promised that a failure stops siblings that have not yet started. The scheduler spawns
everything that is ready and lets a semaphore pace it, so "spawned" and "started" have come apart
and the promise covers only the first. The fix is small: one shared halt flag, checked after the
permit is acquired. A sibling that never started gets a record that says so, so the report
accounts for every node.

Two more refusals belong to the same family. `run` must refuse a graph it cannot finish, rather
than return the part it could run as `Ok`. And a route value of the wrong JSON type must be
reported as data, the same way a failing upstream call is.

---

## Requirements

### Functional

- **R1: a halt flag gates the start of work.** `run` owns an `Arc<AtomicBool>` halt flag. `run_one`
  checks it **after** acquiring its permits and **before** executing. When it is set, the node does
  not execute.
- **R2: un-started nodes are accounted for.** `RunReport` gains `not_started: Vec<String>`
  (sorted): every node that was spawned but halted by R1, **plus** every node never spawned
  because fail-fast stopped scheduling. `records.len() + not_started.len() == graph.nodes.len()`
  holds for every run with `stopped_early: true`.
- **R3: `run` refuses a cyclic graph.** If `topo_order(graph).len() < graph.nodes.len()`, `run`
  returns `Err(RunError::Cycle { nodes })` naming the nodes left over. It executes nothing.
- **R4: a non-string route value is data.** `extract_on_value` failing for any reason records the
  route node as failed (`success: false`, `error` naming the field and the JSON type found), and
  the run reports `stopped_early: true`. It is no longer `Err`.
- **R5: scheduler tests bound their own runtime.** Every `#[tokio::test]` in `sched.rs` and
  `route_exec.rs` wraps the run in `tokio::time::timeout(Duration::from_secs(10), …)`. A hang then
  becomes a failure the test reports itself.

### Non-Functional

- **N1:** no new dependency. `tokio` gains the `time` feature in `laya-graph` only, and it is
  already a workspace feature used by `laya-mcp` and `laya-cli`.
- **N2:** `RunReport` remains serde-compatible for readers. `not_started` uses `#[serde(default)]`.
- **N3:** zero clippy, fmt clean.

---

## Architecture

```
spawn_ready ─► JoinSet ─► run_one: acquire fanout ─► (writes? acquire write_lock)
                                      └─► if halt.load() { return NotStarted(id) }  ◄── R1
join_next: failed && !allow_failure ─► halt.store(true); stop_scheduling = true
run end:   not_started = halted ∪ (nodes − records − halted)                           ◄── R2
run start: topo_order(graph).len() < nodes.len() ─► Err(Cycle)                          ◄── R3
```

**Key decision: a flag checked at the permit, not `JoinSet::abort_all`.** Aborting a task while it
waits on a semaphore is safe, but aborting one inside `spawn_blocking` does not stop the blocking
work. That would give a run in which "aborted" means two different things. The flag gives one
meaning: *checked before work begins*. Cancelling work already running stays deferred, as master
spec R11 already records.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | `a_failure_stops_queued_siblings_from_starting` | `limit: 1`; independent `a` (`sleep 0.2; exit 1`), `b`,`c`,`d` (`touch <tmp>/<id>`) | `stopped_early`; **no `<tmp>/b|c|d` file exists** (an effect check); `not_started == ["b","c","d"]` |
| R1 | `the_control_allow_failure_does_not_halt_siblings` | same, `a.allow_failure = true` | all three files exist; `not_started` empty |
| R2 | `every_node_is_either_recorded_or_not_started` | 6-node diamond, a mid node fails | `records.len() + not_started.len() == 6` |
| R3 | `a_cyclic_graph_is_refused_not_partially_run` | `p ⇄ q`, plus `z` | `Err(RunError::Cycle)` naming `p`,`q`; `z` did **not** run (effect check) |
| R3 | `the_control_a_diamond_is_not_a_cycle` | `a→b, a→c, b→d, c→d` | `Ok`; `d` ran once, after `b` and `c` (this also kills the `sched.rs:52` mutant) |
| R4 | `a_non_string_route_value_is_data_not_an_error` | caller returns `{"payload":{"label":3}}` | `Ok(report)`, `stopped_early`, the route record's error names `label` and `number` |
| R4 | `the_control_a_string_route_value_still_routes` | `{"payload":{"label":"a"}}` | the `a` branch runs |

**R1's effect check is the acceptance test.** Under the current code, `records` would *list* `b`,
`c` and `d` with `success: true`, so an assertion on `records` written loosely could pass by
accident. Only the absence of their files proves they did not run.

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`** (the tests do not exist, so `ok. 0 passed` gives grep nothing to match):

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_failure_stops_queued_siblings_from_starting 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_allow_failure_does_not_halt_siblings 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1's control
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib every_node_is_either_recorded_or_not_started 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_cyclic_graph_is_refused_not_partially_run 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_non_string_route_value_is_data_not_an_error 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4
- [ ] `cd /projects/laya-rs && TMPDIR=/projects/_tmp/mutants-tmp cargo mutants --cargo-test-arg=--lib --gitignore true -f crates/laya-graph/src/sched.rs -o "$(mktemp -d /projects/_tmp/mutants-out-XXXX)"`: mutation score. cargo-mutants exits 0 only when every viable mutant is caught; it exits 2 for a missed mutant, 3 for a timeout and 4 for a failing baseline, so the check fails safe. Measured pre-implementation: 1 missed (`sched.rs:52`) and 5 timeouts.
  **`TMPDIR` and `--gitignore true` are load-bearing, not decoration.** cargo-mutants copies the tree into `TMPDIR` and, by default, **copies gitignored files too** — including this repo's `.venv-laya`, which is 5.4 GB. On 2026-10-01 that filled the container's root filesystem (`No space left on device`, `Failed to copy … libcusparseLt.so.0`) and the run died. Root `/` holds ~22 GB free; `/projects` holds ~188 GB. Measured on this box: with `TMPDIR` on `/projects` and `--gitignore true` the same command completes. Without them it can fill root and then fail *at random*, which is the worst failure mode for a gate.

**Carried re-checks:**

- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_failure_stops_the_run_and_reports_stopped_early`: the existing R11 chain test still passes
- [ ] `cd /projects/laya-rs && cargo clippy -p laya-graph --all-targets -- -D warnings`
- [ ] `cd /projects/laya-rs && cargo fmt --all --check`

---

## Guardrails

- **G1: do NOT implement cancellation of running work.** It stays deferred (master spec R11 and part-3 G18). R1 halts only what has not started.
- **G2: do NOT change dispatch order.** R8 determinism (id order) is unchanged.
- **G3: do NOT make `Graph`'s fields private** to "solve" R3. That is a breaking API change and hides the defect rather than refusing the input.
- **G4:** no git worktrees, no second runtime (recorded rejections; see `_docs/06-research/03-four-architecture-questions.md` §3.5 and part 3 G20).

### Error handling expectations
- `RunError::Cycle` carries the node ids, sorted. "graph is invalid" on its own is not acceptable.
- R4's error text names the field **and** the JSON type found (`number`, `bool`, `null`, `object`, `array`, `missing`).

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-graph/src/sched.rs` | halt flag; `not_started`; cycle refusal; test timeouts; diamond test | R1–R3, R5 |
| `crates/laya-graph/src/run.rs` | `RunReport.not_started` | R2 |
| `crates/laya-graph/src/error.rs` | `RunError::Cycle { nodes }` | R3 |
| `crates/laya-graph/src/route_exec.rs` | R4 as data; test timeouts | R4, R5 |
| `crates/laya-graph/Cargo.toml` | tokio `time` feature | R5 |
| `crates/laya-cli/src/graph.rs` | print `not_started` in the summary line | R2 |
