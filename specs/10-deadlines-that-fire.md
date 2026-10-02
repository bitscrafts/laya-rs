# laya-rs — spec-10: deadlines that fire — node `timeout`, and a client that kills what it abandons

- **Project**: `/projects/laya-rs`
- **Priority**: P1. A declared field that does nothing is the `irreversible` defect again (HANDOFF, "decorative"). An unbounded child process is how a 3 a.m. run hangs with nothing to read.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`REPORT.md` §1.3)
- **Depends-On**: 09 (shares `RunReport`/`NodeRecord` edits; land 09 first to avoid a merge on the same struct).

---

## Verified Premises

- `crates/laya-graph/src/flow.rs:135-137`: `/// Timeout, in seconds.` `pub timeout: Option<u64>`. `_docs/03-reference/GRAPH-SPEC.md:84` documents it as `seconds | inherited from the flow`.
- `grep -rn timeout crates/laya-graph/src/` gives only `flow.rs:137`, `flow.rs:193` (a test constructor) and `render.rs:33` (listed as a field the *renderer drops*). **No executor reads it.**
- **Measured live** (`notes/timeout-flow.json`): a flow whose only node is `{"kind":"shell","run":"sleep 3; echo done","timeout":1}` gives `success:true, duration_ms:3002` and `laya graph run: completed`. The declared 1-second timeout is ignored, `validate` accepts it, and nothing warns.
- `crates/laya-graph/src/node.rs:62-64`: `execute_shell` is `Command::new("sh").arg("-c").arg(run).output()`. It blocks inside `spawn_blocking` (`sched.rs:253-255`), with no deadline and no kill. A hung command pins a blocking thread and the whole run forever.
- `crates/laya-cli/src/mcp_client.rs:38`: `CALL_TIMEOUT = Duration::from_mins(2)` bounds only `rx.recv_timeout` (`:114`). On the error path, `:239-241` does `drop(stdin); let _ = child.wait();` and **never `child.kill()`**. A `laya-mcp` that is hung rather than slow, and ignores stdin EOF, makes `wait()` block indefinitely, so the 2-minute "timeout" does not bound the call. *Reasoned from code; a truly hung child was not reproduced in this review.*
- `crates/laya-backbone/src/sidecar.rs:43`: `PREDICT_TIMEOUT = Duration::from_mins(2)` is hardcoded and not configurable. `laya-mcp` runs on a **current-thread** runtime (`crates/laya-mcp/src/main.rs:72-74`) and calls the sidecar synchronously, so a slow head stalls every tool for up to 2 minutes.
- The workspace pins `tokio = { version = "1.53", default-features = false }` (`Cargo.toml:51`). The `process` and `time` features are part of the same crate, so using them adds **no second runtime** (rejection recorded at part 3 G20).

---

## Overview

Every deadline in this system is either decorative (the node `timeout`) or bounds the wrong thing
(the client's `recv_timeout`, after which it waits forever anyway). This spec makes each deadline
**fire** and **kill** what it abandons, and records the expiry as data, so a timed-out node is
visible as such and not as a generic failure.

---

## Requirements

### Functional

- **R1: a node `timeout` is enforced.** `GraphNode` carries `timeout: Option<Duration>`, lowered
  from `Node.timeout` (seconds). Shell and mcp nodes run under it.
- **R2: expiry kills the child, and its process group.** Shell nodes run via `tokio::process::Command`
  with `kill_on_drop(true)` in a new process group. On expiry the group is signalled, so
  `sh -c "sleep 600 & wait"` leaves no orphan. The record says `success: false`, `timed_out: true`,
  and `duration_ms` ≈ the deadline.
- **R3: a flow-level default.** `Flow.timeout` (seconds, optional) is inherited by nodes that do not
  set one, as GRAPH-SPEC.md:84 already documents. With neither set, there is **no** deadline, which
  keeps today's behaviour and keeps the change opt-in.
- **R4: the MCP client kills on every error path.** In `call_real_mcp`, any `Err` (including a
  `recv_timeout` expiry) calls `child.kill()` before `child.wait()`. Wall-clock is then bounded by
  `CALL_TIMEOUT` plus a small constant.
- **R5: `NodeRecord` gains `timed_out: bool`** (`#[serde(default)]`).

### Non-Functional

- **N1:** tokio features `process` and `time` are added to `laya-graph`. No new crate, no second runtime.
- **N2:** a node *without* a timeout keeps exactly today's behaviour. Proven by the R1 control.
- **N3:** zero clippy, fmt clean; no `unwrap`/`expect`/`panic`.

---

## Architecture

```
lower: Node.timeout ?? Flow.timeout ─► GraphNode.timeout
run_one ─► Shell: tokio::process (own pgid, kill_on_drop) ─► tokio::time::timeout(d, child.wait_with_output())
                    └─ Elapsed ─► kill(-pgid) ─► NodeRecord{success:false, timed_out:true}
        ─► Mcp:   spawn_blocking(execute_mcp) — McpCaller is sync; the deadline is enforced by the caller impl (R4)
```

**Key decision: move `shell` from `std::process` inside `spawn_blocking` to `tokio::process`.** A
blocking `output()` cannot be timed out from outside without leaking the thread. `tokio::process`
can be awaited under `timeout` and killed on drop. The process-free criterion of part 1 is scoped to
part 1's six files, so this does not violate it.

**Signalling the group without `unsafe`.** `tokio::process::Command::process_group(0)` puts the
child in its own group. The workspace forbids `unsafe_code`, so `libc::kill` is not available.
Kill the group by spawning `kill -KILL -- -<pgid>` with `std::process::Command` (verified
available: `/bin/kill`). `libc` is in `Cargo.lock` only as a transitive dependency; it is not a
direct one, and adding it would not help, because calling it needs `unsafe`.

**Rejected: a watchdog thread per node.** It is the same thing built by hand, with more ways to get
it wrong.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | `a_node_past_its_timeout_is_killed_and_reported` | shell `sleep 30`, `timeout: 1` | run returns in < 5 s; record `success:false, timed_out:true` |
| R1 | `the_control_a_node_within_its_timeout_succeeds` | shell `sleep 0.1`, `timeout: 5` | `success:true, timed_out:false` |
| R1 | `the_control_a_node_without_a_timeout_is_unbounded_as_before` | shell `sleep 1.5`, no timeout | `success:true` |
| R2 | `a_timeout_leaves_no_orphan` | `sh -c 'sleep 600 & echo $! > <tmp>/pid; wait'`, `timeout: 1` | after the run, `kill -0 $(cat <tmp>/pid)` **fails**: the grandchild is gone |
| R3 | `a_flow_timeout_is_inherited` | `Flow.timeout: 1`, node with none, `sleep 30` | `timed_out:true` |
| R4 | `a_hung_server_is_killed_not_awaited_forever` | `LiveMcpCaller` with `bin` = a script that reads nothing and `sleep 600`s, with `CALL_TIMEOUT` overridable to 1 s in test | returns `Err` in < 5 s |

**R2 is the acceptance test.** Killing only `sh` leaves `sleep 600` running, the timeout "passes",
and the orphan is the defect.

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`:**

- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_node_past_its_timeout_is_killed_and_reported 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib the_control_a_node_without_a_timeout_is_unbounded_as_before 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1 control
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_timeout_leaves_no_orphan 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [x] `cd /projects/laya-rs && cargo test -p laya-cli --lib a_hung_server_is_killed_not_awaited_forever 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4
- [ ] `cd /projects/laya-rs && cargo build -q -p laya-cli && d=$(mktemp -d) && printf '%s' '{"flow":"t","nodes":[{"id":"s","kind":"shell","run":"sleep 3","timeout":1}]}' > "$d/f.json" && ! timeout 10 ./target/debug/laya graph run "$d/f.json" >/dev/null 2>&1`: **live, end to end**. The CLI must report the run as failed. Measured pre-implementation: exit 0, `completed`, `duration_ms: 3002`

**Carried re-checks:**

- [x] `cd /projects/laya-rs && cargo test -p laya-graph`: no regression
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings`

---

## Guardrails

- **G1: do NOT add a default timeout.** Today's behaviour (none) stays for flows that declare none (N2). A silent new default would kill legitimate long nodes.
- **G2: do NOT implement cancellation of *other* running nodes.** A timeout is local to its node; fail-fast (spec 09) decides the rest.
- **G3: no second async runtime, no `async-std`/`smol`** (part 3 G20).

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-graph/src/flow.rs` | `Flow.timeout` | R3 |
| `crates/laya-graph/src/graph.rs` | `GraphNode.timeout` | R1 |
| `crates/laya-graph/src/lower.rs` | carry and inherit timeout | R1, R3 |
| `crates/laya-graph/src/node.rs` | async shell executor with pgid + kill | R1, R2 |
| `crates/laya-graph/src/sched.rs` | run under `timeout` | R1 |
| `crates/laya-graph/src/run.rs` | `NodeRecord.timed_out` | R5 |
| `crates/laya-graph/src/render.rs` | stop listing `timeout` as dropped | R1 |
| `crates/laya-graph/Cargo.toml` | tokio `process`, `time` | N1 |
| `crates/laya-cli/src/mcp_client.rs` | kill on every error path; test-overridable timeout | R4 |
