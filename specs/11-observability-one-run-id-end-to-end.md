# laya-rs — spec-11: observability — one run id end to end, every judgement recorded, a timeline per run

- **Project**: `/projects/laya-rs`
- **Priority**: P1. At 3 a.m. the question is "which decision sent run X down branch Y, and what did the gate let through". Today neither can be answered from the artefacts.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`REPORT.md` §2)
- **Depends-On**: 09 (`RunReport` shape), 08 (the gate response fields that R2 records).

---

## Verified Premises

- `crates/laya-store/src/lib.rs:93-144`: `DecisionRow` has `run_id: Option<String>` and `node_id: Option<String>`, documented as *"the join key to the consumer's own run"*.
- `crates/laya-mcp/src/lib.rs:714-715`: `record_decision` writes `run_id: None, node_id: None` **unconditionally**. `RawArgs` (`lib.rs:359-379`) has no field that could carry them. **A graph run's decisions can never be joined to its `RunReport`.**
- `crates/laya-graph/src/node.rs:47-54`: `McpCaller::call(&self, tool: &str, arguments: &Value)` receives no run context. `route_exec.rs:149-171` knows `opts.run_id` and the node id, and cannot pass either.
- `crates/laya-mcp/src/lib.rs:1328`: `record_decision` has **one** call site, `decide_via_sidecar`. `gate`, `ask` (which **executes commands**, `lib.rs:863`), `ask_file`, `ask_files` and `should_compact` judge and are **never recorded**. The most safety-relevant decisions in the system leave no trace.
- `crates/laya-mcp/src/lib.rs:742-745`: `bucket_clamped` is filled from `payload["saturated"]`. These are two different facts: B2's temperature clamp, and a saturated probability. Every row's `bucket_clamped` is wrong whenever they differ.
- `crates/laya-mcp/src/lib.rs:740-741`: `head_sha256: None, backbone_sha256: None`. The schema has provenance fields, and nothing fills them.
- `crates/laya-mcp/src/lib.rs:705`: `let dir = state.ledger_dir.as_ref()?;`. With `LAYA_LEDGER_DIR` unset, recording is **silently** skipped. Nothing at startup says the ledger is off.
- `crates/laya-graph/src/node.rs:63,109`: `duration_ms` starts **after** the permits are acquired (`sched.rs:238-246`). Semaphore wait is invisible, so a run starved by `limit` looks the same as a run of slow nodes.
- `crates/laya-store/src/lib.rs:807-823`: `Stats` has `latency_p50/p95/max` across **all** tools, with no per-tool split and no gate-decision distribution.
- No crate uses `tracing` or `log`. `tracing 0.1.44` is already in `Cargo.lock` (pulled in by `rmcp`). **`tracing-subscriber` is not in the offline registry** (`/usr/local/cargo/registry/cache` checked), so a subscriber would need a network fetch.
- `crates/laya-mcp/src/main.rs` has 9 `eprintln!`. `crates/laya-mcp/src/lib.rs:666` has 1 (the gate's fail-closed fallback). They are free text, with no request id, tool, latency or run id.

---

## Overview

The system already records the right *kind* of thing: run records with `run_id`, and a ledger
schema with a `run_id` column. The wires between them are missing. This spec connects them:

1. **One id end to end.** The run id and node id travel from the scheduler, through `McpCaller`,
   to `laya-mcp`, into the ledger row. `RunReport ⋈ ledger` on `(run_id, node_id)` becomes a join
   and stops being a guess.
2. **Every judgement is a row.** `gate` and `ask` most of all. A gate verdict is recorded with its
   final decision, the model decision, the signal count, and whether a command executed.
3. **A timeline per node.** `queued_ms` and `started_at_ms` separate waiting from working, so
   fan-out utilisation and starvation can be computed from the report alone.
4. **One structured event line per tool call** on `laya-mcp`'s stderr, JSON, carrying the same ids.
   This is hand-rolled, about 40 lines, because the subscriber crate is not available offline. The
   event names are chosen so a later move to `tracing` is a mechanical swap.

---

## Requirements

### Functional

- **R1: run context reaches the ledger.** `McpCaller::call` takes a `&CallContext { run_id: String, node_id: String }`.
  `LiveMcpCaller` sends them as tool arguments `run_id` / `node_id`. `RawArgs` accepts both
  (optional), and `record_decision` writes them. A call with no context (a human using `decide`
  directly) still records `None`.
- **R2: every judging tool records.** `gate`, `ask`, `ask_file`, `ask_files` and `should_compact`
  write a `DecisionRow` (one row per question answered). `DecisionRow` gains:
  `gate_decision: Option<String>` (the final `allow|ask|deny`), `model_decision: Option<String>`,
  `destructive_signals: Option<u32>`, `executed: Option<bool>` (for `ask`), and
  `command_sha256: Option<String>`. The command itself is **never** stored (AGENTS rule 8: hash, not content).
- **R3: `bucket_clamped` stops lying.** It becomes `Option<bool>`. It is `None` until a source that
  actually knows the clamp state supplies it. It is never copied from `saturated`.
- **R4: the ledger state is announced.** At startup `laya-mcp` writes one event:
  `{"event":"ledger","enabled":bool,"dir":…}`. `laya doctor` reports `ledger: off (LAYA_LEDGER_DIR unset)`
  as a warning line. It is not a failure.
- **R5: a node timeline.** `NodeRecord` gains `queued_ms` (from spawn to permit) and `started_at_ms`
  (offset from run start). `RunReport` gains `wall_ms`.
- **R6: a structured event per tool call.** `laya-mcp` emits one JSON line to stderr per tool call:
  `{"ts","event":"tool_call","seq","tool","run_id","node_id","latency_ms","outcome":"ok|error","decision"?}`.
  `seq` is a per-process counter that serves as the request id. The existing free-text `eprintln!`
  diagnostics stay, so the human-readable path is unchanged.
- **R7: `stats` per tool.** `laya ledger stats` prints, per tool: count, p50/p95/p99 latency, and for
  `gate`/`ask` the `allow/ask/deny` counts and the `executed` count.

### Non-Functional

- **N1: `laya-graph` still does not depend on `laya-mcp` or `laya-store`.** `CallContext` lives in `laya-graph`.
- **N2: rows remain readable.** Every new field uses `#[serde(default)]`, and old segments compact unchanged (test).
- **N3: no secrets.** The command, state and file contents are never written. Only hashes.
- **N4:** no new crate. If `tracing` is adopted later, it replaces R6's emitter and keeps its field names.

---

## Architecture

```
sched/route_exec ──CallContext{run_id,node_id}──► McpCaller::call
LiveMcpCaller ──args.run_id/node_id──► laya-mcp handle ──► record_decision ──► segment ──► compact ──► redb
                                              └──► stderr: {"event":"tool_call","seq",…,"run_id"}
NodeRecord{queued_ms, started_at_ms, duration_ms} ─► RunReport{wall_ms}
```

**Key decision: no `tracing` yet.** Spans across a stdio process boundary need context propagation,
and R1 provides exactly that by hand. A subscriber crate is not available offline. Logs are not
where this system's evidence lives; the ledger is, and that is where the join must work. Revisit
`tracing` when there is a collector to send spans to.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | `a_decide_records_the_callers_run_id_and_node_id` (laya-mcp) | `decide` with `run_id:"r1", node_id:"n1"`, a tmp ledger dir, a stub sidecar answer | the segment's row has `run_id:"r1", node_id:"n1"` |
| R1 | `the_control_a_decide_without_context_records_none` | same, no context | `run_id: None` |
| R1 | `the_ledger_row_joins_the_run_report` (laya-graph acceptance, **live**) | R-ACC positive fixture, `LAYA_LEDGER_DIR` = tmp | exactly one row with `run_id == report.run_id` and `node_id == "classify"`, and its `label` equals the route record's `value` |
| R2 | `every_judging_tool_writes_a_ledger_row` | each of gate/ask/ask_file/ask_files/should_compact against a stub answer | ≥ 1 row with that `tool` |
| R2 | `the_control_the_ledger_tool_writes_no_decision_row` | `ledger stats` | no new row |
| R2 | `a_gate_row_carries_the_decision_and_never_the_command` | `gate` on `"ls -la /tmp"` | `gate_decision` set; `command_sha256` set; **the string `ls -la` appears nowhere in the segment** |
| R3 | `bucket_clamped_is_not_a_copy_of_saturated` | payload `saturated: true` | `bucket_clamped == None` |
| R5 | `a_record_separates_queue_wait_from_execution` | `limit:1`, two `sleep 0.3` nodes | second node `queued_ms ≥ 250`, `duration_ms ≥ 250`; first node `queued_ms < 100` |
| R6 | `each_tool_call_emits_one_structured_event` | one `gate` call through `handle` with an injected writer | exactly one JSON line, `event == "tool_call"`, `tool == "gate"` |
| N2 | `an_old_segment_without_the_new_fields_still_compacts` | a segment line from today's schema | compacts; new fields `None` |

**Stub sidecar.** R1/R2/R3 need a predictable answer without the live model (AGENTS rule 9). This
needs the `Judge` seam described in `REPORT.md` §3: `ServerState` holds `Box<dyn Judge>`, and tests
inject a fixed-answer implementation. That trait is in scope here because without it these tests
cannot be offline.

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`:**

- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_decide_records_the_callers_run_id_and_node_id 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib every_judging_tool_writes_a_ledger_row 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_gate_row_carries_the_decision_and_never_the_command 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2 + N3
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp --lib bucket_clamped_is_not_a_copy_of_saturated 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --lib a_record_separates_queue_wait_from_execution 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R5
- [ ] `cd /projects/laya-rs && cargo test -p laya-graph --test acceptance the_ledger_row_joins_the_run_report -- --include-ignored 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1 end to end (**live**; see spec 13 R1 for why it is ignored by default)

**Carried re-checks:**

- [ ] `cd /projects/laya-rs && test -f crates/laya-graph/Cargo.toml && ! grep -qE '^laya-(mcp|store)' crates/laya-graph/Cargo.toml`: N1. Control: `grep -qE '^laya-store' crates/laya-mcp/Cargo.toml` succeeds
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings`

---

## Guardrails

- **G1: never store the state, a command, or file content.** Hashes only (AGENTS rule 8). R2's test greps the segment for the plaintext.
- **G2: do NOT open redb from `laya-mcp`.** Rows still go to segments; `compact` stays the only writer (B4).
- **G3: do NOT make a ledger write failure fail the tool call.** Today's `"ledger": note` behaviour (`lib.rs:689-695`) stays.
- **G4: do NOT expose `route` as a tool** while adding arguments to `decide`.

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-graph/src/node.rs` | `CallContext`; `McpCaller::call` signature | R1 |
| `crates/laya-graph/src/{sched,route_exec}.rs` | pass context; `queued_ms`, `started_at_ms`, `wall_ms` | R1, R5 |
| `crates/laya-graph/src/run.rs` | new record fields | R5 |
| `crates/laya-cli/src/mcp_client.rs` | send `run_id`/`node_id` | R1 |
| `crates/laya-mcp/src/lib.rs` | `RawArgs` fields; record every tool; `Judge` seam; event emitter | R1, R2, R6 |
| `crates/laya-mcp/src/main.rs` | ledger-state startup event | R4 |
| `crates/laya-store/src/lib.rs` | `DecisionRow` new fields; `bucket_clamped: Option<bool>`; per-tool `Stats` | R2, R3, R7 |
| `crates/laya-cli/src/main.rs` | `stats` per tool; `doctor` ledger line | R4, R7 |
| `crates/laya-graph/tests/acceptance.rs` | the join test | R1 |
| `_docs/05-operations/DECISION-LEDGER.md` | schema additions | R2 |
