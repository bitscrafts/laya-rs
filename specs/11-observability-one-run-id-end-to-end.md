# laya-rs — spec-11: observability — one run id end to end, every judgement recorded, a timeline per run
#: depends=- status=pending block=-

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
- **Measured 2026-10-02, in the live Hermes deployment — the ledger has NEVER recorded anything.**
  `LAYA_LEDGER_DIR` was **unset**, and absent from the MCP server entry in the host config, so
  `record_decision`'s `?` returned early on every call. All from the deployed system: `laya ledger
  stats` reported `decisions: 0, outcomes: 0, latency p50/p95/max: unknown`; the host's session store
  (133 207 messages, ~65 000 tool calls) contains **3** calls to any `mcp_laya_*` tool and no row
  attributed to the server; and no `ledger-*.jsonl` existed outside throwaway test directories. With
  `LAYA_LEDGER_DIR` set on the same call, `decide` writes a segment immediately --
  `{"tool":"decide","question_id":"paid","label":"yes","band":"mid","confidence":0.7872,
  "latency_ms":478}` -- so the mechanism works and only the configuration was missing.
  **This is the strongest possible argument for R4.** A subsystem that is silently off is
  indistinguishable from one that is broken, and this one was off for the entire life of the
  deployment without a single signal. The criteria for R4 should therefore assert the *warning*
  appears when the directory is unset, not merely that a healthy server prints `enabled: true`.

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
- **R2: every judging tool records — including its REFUSALS.** `gate`, `ask`, `ask_file`, `ask_files`
  and `should_compact` write a `DecisionRow` (one row per question answered). A refusal counts: `ask`'s
  refusal path returns `Err` (`crates/laya-mcp/src/lib.rs:939-947`) and `gate`'s fail-closed arm
  (`:652-661`) currently produce no row, so **what the gate denied, what `ask` refused, and what the
  fail-closed path answered are invisible** — backwards for an audit ledger, since those are the rows
  an incident review wants most. The row is written before the error is returned. `DecisionRow` gains:
  `gate_decision: Option<String>` (the final `allow|ask|deny`), `model_decision: Option<String>`,
  `destructive_signals: Option<u32>`, `executed: Option<bool>` (for `ask`), and
  `command_sha256: Option<String>`. The command itself is **never** stored (AGENTS rule 8: hash, not content).
- **R3: `bucket_clamped` stops lying, and `confidence` stops inventing.** `bucket_clamped` becomes
  `Option<bool>`: `None` until a source that actually knows the clamp state supplies it, never copied
  from `saturated`. **The same class covers `confidence`**, which is `unwrap_or(0.0)`
  (`crates/laya-mcp/src/lib.rs:711-714`) so a missing confidence records as `0.0` — indistinguishable
  from "maximally unsure", and it poisons any calibration metric built on the column. It becomes
  `Option<f64>` too.
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
  `gate`/`ask` the `allow/ask/deny` counts and the `executed` count. It opens the store **read-only**
  — today it takes the exclusive write lock (`crates/laya-cli/src/main.rs:273,308`), so an operator
  asking for numbers during a `compact` gets a lock error instead of numbers.
- **R8: one comparable ordering key across every writer.** `ts_nanos` is process-relative
  (`crates/laya-mcp/src/lib.rs:763-773`) while the CLI's outcome rows use epoch nanos
  (`crates/laya-cli/src/main.rs:346-353`) — a ~10^16 scale gap, measured on the live store (7 rows,
  `ts_nanos` 65..171). Every outcome therefore sorts after every decision regardless of time, and
  decisions from different processes interleave arbitrarily, which breaks R1's join story and R7's
  "newest". Rows gain `wall_nanos: u64` (epoch nanoseconds, `#[serde(default)]`), and `compact` sorts
  on it. `ts` (second resolution) is too coarse to break ties.
- **R9: the ledger's documented claims match the schema, or are annotated.** Two confirmed drifts:
  the docs call `ts` RFC 3339 while `now_rfc3339` emits `epoch+{secs}`
  (`crates/laya-mcp/src/lib.rs:776-785` vs `DECISION-LEDGER.md:85`); and `DECISION-LEDGER.md:93`
  lists `p` / `score` fields that `DecisionRow` does not have, even though `DecisionPayload` carries
  both (`crates/laya-core/src/route_payload.rs:56,59`) and `record_decision` simply does not copy
  them. Copy the fields, and annotate the `ts` claim rather than erasing it.

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
| R4 | `the_ledger_state_is_announced_at_startup` | a server started with `LAYA_LEDGER_DIR` set | one startup event `{"event":"ledger","enabled":true,"dir":…}` |
| R4 | `the_control_an_unset_ledger_directory_is_announced_as_off` | a server with no `LAYA_LEDGER_DIR` | the same event says `enabled:false`, and it is a **warning**, not silence |
| R7 | `stats_reports_per_tool_latency_and_gate_decisions` | rows for `decide` and `gate` in a tmp ledger | `stats` prints a per-tool block; `gate` shows `allow/ask/deny` counts and an `executed` count |
| R7 | `the_control_stats_opens_the_store_read_only` | a `compact` holding the writer | `stats` still answers, rather than failing on the write lock |
| N2 | `an_old_segment_without_the_new_fields_still_compacts` | a segment line from today's schema | compacts; new fields `None` |

**Stub sidecar.** R1/R2/R3 need a predictable answer without the live model (AGENTS rule 9). This
needs the `Judge` seam described in `REPORT.md` §3: `ServerState` holds `Box<dyn Judge>`, and tests
inject a fixed-answer implementation. That trait is in scope here because without it these tests
cannot be offline.

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`:**

- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_decide_records_the_callers_run_id_and_node_id 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib every_judging_tool_writes_a_ledger_row 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_gate_row_carries_the_decision_and_never_the_command 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2 + N3
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib bucket_clamped_is_not_a_copy_of_saturated 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [x]  `cd /projects/laya-rs && cargo test -p laya-graph --lib a_record_separates_queue_wait_from_execution 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R5
- [x]  `cd /projects/laya-rs && cargo test -p laya-graph --test acceptance the_ledger_row_joins_the_run_report -- --include-ignored 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1 end to end (**live**; see spec 13 R1 for why it is ignored by default)
- [x] `cargo test -p laya-mcp --lib the_ledger_state_is_announced_at_startup 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib the_control_an_unset_ledger_directory_is_announced_as_off 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4's control — the off state must be **announced**, which is the whole point of the requirement, and the deployment failure this spec answers
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib each_tool_call_emits_one_structured_event 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R6
- [x]  `cd /projects/laya-rs && cargo test -p laya-mcp --lib events_carry_the_run_and_node_id 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R6 — an event without the ids answers nothing, so the ids are the requirement, not the JSON shape
- [x] `cargo test -p laya-store --lib stats_reports_per_tool_latency_and_gate_decisions 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R7
- [x] `cargo test -p laya-store --lib stats_opens_the_store_read_only 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R7 — a read must not take the exclusive write lock
- [x] `cargo test -p laya-store --lib merged_rows_sort_by_wall_time_not_process_relative_nanos 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R8 — two rows from different processes, written out of order, must sort correctly after compaction
- [x] `cargo test -p laya-store --lib an_absent_confidence_is_none_not_zero 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3's second half
- [x] `cargo test -p laya-mcp --lib a_refused_gate_is_recorded 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R2 — the refusal row, which is the one an incident review needs
- [x] `cargo test -p laya-cli --lib stats_does_not_take_the_write_lock 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R7's read-only requirement
- [x] `cargo test -p laya-store --lib the_documented_fields_exist_on_the_row 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R9 — every field the ledger doc tabulates is a field of `DecisionRow`; this fails today on `p` and `score`
- [x]  `cd /projects/laya-rs && python3 scripts/live/check-observability.py`: **live**. It drives the deployed server, asserts a ledger segment is written for a `gate` call, that the row carries `run_id`/`node_id` when they are supplied, and that the command text never appears in the segment. Exits non-zero if the ledger is off — because "off" and "broken" must not look the same

**Carried re-checks:**

- [x]  `cd /projects/laya-rs && test -f crates/laya-graph/Cargo.toml && ! grep -qE '^laya-(mcp|store)' crates/laya-graph/Cargo.toml`: N1. Control: `grep -qE '^laya-store' crates/laya-mcp/Cargo.toml` succeeds
- [x]  `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings`

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

---

## Independent review of THIS spec (GLM-5.3, 2026-10-02) — verified additions

A second reviewer was given this spec and asked whether it is complete, requirement by requirement.
It found four things the first review did not. Each claim below was **checked by the orchestrator
against the tree** before being written here, and is marked with what checking found.

**1. `laya ledger stats` takes the exclusive write lock to print.** `[measured — confirmed]`
`crates/laya-cli/src/main.rs:273` and `:308` both call `laya_store::Ledger::open_writer(store)` to
read. Under the single-writer model (B4) that means an operator running `stats` while a `compact`
runs gets a lock error rather than numbers. A read path should open read-only. **One line each, and
it is not in R7.** Worth folding into R7.

**2. The ledger doc promises two fields the struct does not have.** `[measured — confirmed]`
`_docs/05-operations/DECISION-LEDGER.md:93` lists `` `p` / `score` `` — "for a `noul` / `score`" —
and `DecisionRow` (`crates/laya-store/src/lib.rs:93-144`) has **neither**; `question_type` and
`label` are what actually carry that information. So the doc describes a schema that was never
built, and a reader auditing the ledger will look for columns that do not exist. Either R2 adds
them or the doc is annotated — but the drift must not survive this spec, since a ledger with no
`p` makes a `noul`'s raw probability unrecoverable from the row.

**3. R4's `doctor` line is weaker than it reads.** `[measured — confirmed]`
`laya doctor` runs as its own process, so it can read *its own* environment, not the running
server's. It therefore cannot answer "is the server's ledger on?" — the question R4 exists to make
visible. What it *can* check is the shared, known directory: whether `LAYA_LEDGER_DIR` resolves to
a path that exists and holds segments. **R4's criterion should assert the weaker, true claim**
(directory state) rather than implying doctor can see the server. The startup event (also R4) is
what actually reports the server's own view, so the two are complementary and the spec should say so.

**4. "What did the gate allow in the last hour?" needs a window, and R7 has none.** `[measured —
confirmed by inspection]` `tail` returns newest-first over the whole store and `stats` is
all-time and per-tool only. There is no `--since`. So after this spec lands, the seven questions
include one that is still not answerable. Either R7 gains a window or the claim is narrowed in R2's
wording. (The gate rows themselves become possible once R2 records them; that part is right.)

**One more, from the same reviewer, which the first review raised and spec 11 does not close:**
`head_sha256`/`backbone_sha256` are always `None`, and **no requirement in R1–R7 fills them.** So
"which head produced this decision" is unanswerable after this spec too. Either add a requirement
or state explicitly that it is out of scope and why — leaving it silently absent is how it stays
absent.

**What the reviewer confirmed as right:** the `Judge` seam is the correct place to make these
judgements testable offline; `DecisionSink` is worth introducing precisely because it turns "the
ledger is off" from a silent `?` into an explicit state that can be announced — which is the
deployment failure this whole spec responds to; and R1/R2 are the two highest-value requirements,
in that order.

**Evidence and provenance.** The reviewer was killed by a stream timeout while writing its report,
so no report file exists; its reasoning was recovered from the run log to
`/projects/_tmp/glm-observability/REPORT.RECOVERED.md` and is **raw material, not a verified
review**. The four claims above were independently confirmed against the tree, which is why they
are recorded here; anything else in that log is unreviewed and should be treated as a lead.

---

## Review findings folded in (GLM-5.3, 2026-10-02) — the ones that are requirements, not notes

Each was **verified by the orchestrator against the tree** before being recorded here. Six are
defects this spec must close; one is a defect it must not be allowed to make worse.

### A. The ledger's ordering key is not comparable across processes `[measured — confirmed]`

`now_nanos` is **process-relative**: `START.get_or_init(Instant::now).elapsed()`
(`crates/laya-mcp/src/lib.rs:763-773`). `compact` sorts merged segments by `ts_nanos` while claiming
*"sequence numbers reflect real order even when segments came from several processes"*
(`crates/laya-store/src/lib.rs:868-871`). Two processes each measure from their own start, so the
merged order is arbitrary. The CLI's outcome rows use a **different basis entirely** —
`SystemTime::now().duration_since(UNIX_EPOCH).as_nanos()` (`crates/laya-cli/src/main.rs:346-353`).

Measured on the live store: **7 rows, `ts_nanos` spanning 65..171** (process-relative), against an
epoch-based outcome value around `1.79e18`. The scale gap is ~10^16, so **every outcome sorts after
every decision regardless of real time**, and decisions from different processes interleave
arbitrarily.

**Consequences this spec cannot ignore:** R1's join story depends on ordering being meaningful, and
R7's `tail` claims "newest" — with this key, "newest" is not newest in time. **New requirement
needed:** one comparable key across all writers. `ts` is second-resolution
(`now_rfc3339` emits `epoch+{secs}`), which is too coarse to break ties. Either epoch-nanos
everywhere, or keep the monotonic counter per process and add a globally-comparable
`wall_nanos: u64` that `compact` sorts on.

### B. `ts` is documented as RFC 3339 and is not `[measured — confirmed]`

`now_rfc3339` emits `"epoch+{secs}"` (`crates/laya-mcp/src/lib.rs:776-785`) — neither an RFC 3339
string nor derived from one. Both `crates/laya-store/src/lib.rs:94-95` and
`DECISION-LEDGER.md:85` claim RFC 3339. A production row reads `"ts": "epoch+179…3906"`. Either the
function or the two docs is wrong; the docs are the cheaper correction, and the house rule is to
annotate rather than erase.

### C. `confidence` keeps the sibling defect R3 fixes `[static — confirmed]`

R3 makes `bucket_clamped` an `Option` so a missing value stops masquerading as a value. `confidence`
still does `unwrap_or(0.0)` (`crates/laya-mcp/src/lib.rs:711-714`): a missing confidence records as
**0.0**, which is indistinguishable from "the model was maximally unsure" and poisons any calibration
metric built on the column. **R3 should cover both fields** — they are one defect class.

### D. R2 must record REFUSALS, not only successes `[static — confirmed]`

`ask`'s refusal path returns `Err` (`crates/laya-mcp/src/lib.rs:939-947`) and `gate`'s fail-closed arm
(`:652-661`) produces a payload without recording. If rows are written only on success, then **what
the gate denied, what `ask` refused, and what the fail-closed path answered are all invisible** —
which is exactly backwards for an audit ledger. The refusals are the rows an incident review wants
most. R2 must say so explicitly, with a criterion.

### E. R4's doctor line cannot see the server's environment `[static — confirmed]`

`laya doctor` runs as its own process and reads its own `LAYA_LEDGER_DIR`
(`crates/laya-cli/src/main.rs:195-211`), which says nothing about the running MCP server whose env
comes from the host's MCP entry. A doctor run in the operator's shell **certifies the shell** — a
check that passes while the deployed server is misconfigured, which is the precise shape of the
deployment failure this spec answers. Fix: have doctor report the **ledger directory's** observable
state (segments present? recent mtime?) — a deployment fact visible from outside the server — and
keep the startup event as the server's own view. The two are complementary and the spec must say so.

### F. R6 should be dropped or bounded `[static — see §2-R6 of the report]`

The value is duplicated by the ledger row; stderr's fate is host-controlled and unverifiable from
this repo; and there is a measured pipe-fill risk. **Recommendation: drop R6, or bound it** to a
single startup line. A structured event that nobody can guarantee is delivered adds surface without
adding evidence — and the ledger is where this system's evidence lives.

### G. `compact`'s insert cost grows with exactly the success R2 creates `[measured]`

`insert_if_new` rescans identities and writes one transaction per row: measured **0.487 → 4.277
ms/row as the store grows 250→1500 rows** (extending the prior review's 2.51→4.70 ms/row), at
`crates/laya-store/src/lib.rs:544-563`. R2 raises the row production rate, so this spec makes the
maintenance cost worse by succeeding. **Fold the one-transaction fix in** (hoist the `identities()`
rescan out of the loop), or land it in a store spec before volume rises.

### H. Re-anchor the criteria `[static — confirmed]`

The spec's criteria say "watched to FAIL on `b0ee704`"; HEAD moved to `f43bb23` when spec 08 landed.
Every premise was re-verified at the current tree and all still hold, at shifted lines.
