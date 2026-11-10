# Spec: 04 — The MCP stdio server and the ledger (P1)
#: depends=- status=done block=-

## Overview
One Rust binary (`laya-rs`) speaking **MCP over stdio**, so Hermes, pi, Claude Code,
skills and hooks all consume the same tool surface with **no network listener**.
Every decision is recorded in an embedded redb ledger via a single-writer design.

**Status: COMPLETE (2026-09-30).** Both halves are implemented and proven end to end:
the MCP stdio server (F1-F3, F5-F7, F9-F10) and the redb ledger (F4, F8, NF3). The TDD contract
below now names the tests that EXIST — the first version of this table listed 13 test names that
existed under no name at all, which is how a spec reads as verified while nothing checks it.

**What it cost to get here, recorded because it is the useful part:** the head was measured, not
assumed, and three of the spec's own claims were wrong. See the two "MEASURED" sections at the
end. This spec incorporates the review's corrections: P1 needs a **pinned Python sidecar** for
the head (blocker B3), the ledger uses a single-writer scheme (blocker B4), and `route` is
**not** a tool (finding M6).

**Project**: /projects/laya-rs
**Phase**: P1

## Requirements

### Functional
- F1. Transport is **MCP over stdio**, JSON-RPC 2.0 on stdin/stdout. Copy the
  pattern — `stdio()` → `serve_server(server,
  (r, w))` → `running.waiting()` — **including the spec-22 bounded-lifetime fix**,
  so the server exits on EOF instead of parking forever on `pending()`.
- F2. The tool surface is: `decide`, `ask_file`, `ask_files`, `gate`,
  `should_compact`, `ask`, `ledger`. **`route` is NOT a tool** (M6): exposing it
  would let an agent select edges outside any graph, which is the injection failure
  mode the whole design exists to prevent.
- F3. `decide` accepts a state plus typed questions and returns the route-shaped
  payload of spec 02, including `margin`.
- F4. **P1's head runtime is named and pinned** (blocker B3): a **persistent Python
  sidecar** pinned to a specific Laya version, not a per-call interpreter spawn.
  The pinned version is recorded in every ledger row.
- F5. Errors use the **protocol's error surface** — a failing `gate` must not
  return a successful result whose text begins `"error: …"` (review M6: this is
  fail-open by accident).
- F6. `gate` **fails closed**: if the decision source is unavailable, the answer is
  `ask`, never `allow`.
- F6a. `gate` **cannot allow a near tie** — the same fail-closed rule as an unreachable
  head, applied to a reachable but undecided one. Added 2026-09-30.
- F6b. **`should_compact` (L7).** The four judgements in ONE call; the token count and the
  thresholds stay in code. `mid_edit` is a **veto** (a compaction mid-edit discards the working
  state the edit depends on), and the question that produces it was **measured**: the first
  phrasing agreed with a known-intended answer 1/6 times and was replaced by one scoring 6/6.
  A veto on a coin-flip question is worse than no veto. Added 2026-09-30.
- F6c. **The gate is TWO signals, and `allow` requires both. (Added 2026-09-30.)** Measured:
  this head cannot classify command danger. `"Is this action safe?"` as a noul scored **6/12**
  on commands with known answers (`ls -la /tmp` and `rm -rf <dir>` both came back 0.85-0.89);
  six consequence phrasings *all* had a negative separation, so no threshold could work; a
  `choice` over declared effects reached **13/18** and still missed `: > file` and `dd if=`.
  A `safe` noul at 0.89 with `irreversible=false` had produced a real `allow` on an `rm -rf`.
  Therefore:
  * the model's verdict alone must never be the control, **and neither must a string list**
    (the source research records an agent routing around a string block with a heredoc);
  * `allow` requires the destructive-pattern signal to be **clear AND** the model to reach the
    high band. A pattern match can never be allowed through, whatever the model says;
  * the response reports `model_decision`, `destructive_signals` (matched text + reason) and
    `irreversible`, so a refusal is auditable rather than opaque.
- F6d. **`Policy::gate` and the `irreversible` flag - a decorative parameter, corrected.**
  Both `Band::Mid` arms were identical (`if irreversible { Ask } else { Ask }`), so the flag
  changed nothing, and nothing ever inferred it - it defaulted to `false` and stayed there.
  The measured `allow` on `rm -rf` came through that hole. The safety decision now comes from
  the two-signal rule in F6c, and the flag is reported rather than trusted.
- F6e. **`ask` (L10).** ONE tool where the CALLER writes the questions, over a state composed
  from a summary + files + a command. The command is judged by the SAME `judge_command` the
  `gate` tool uses - one gate, never two that could disagree - and is **not executed at all**
  unless the verdict is `allow`. The response carries answers and reports
  `content_in_context: false`; the state composition (order: output, then files, then summary;
  256 KiB cap as a refusal, not a truncation) lives in `laya-core::agentic` where it is pinned
  offline. `validate_questions` refuses a `choice`/`score` with **no `criteria`** - an earlier
  version accepted `{"type":"choice","options":[...]}`, which laya does not read and which
  raised `AttributeError` inside laya: a validator passing a request that could never work.
- F7. Per-tool **least-privilege** scopes. **The scope lives in `laya-core`, not
  `laya-mcp`** (corrected 2026-09-30): the pure crate, so the rule deciding a refusal is
  unit-testable without a server, and so a second copy cannot drift. Configured by
  `LAYA_READ_ROOTS` (colon-separated); **unset means the process cwd, never `/`**, and the
  resolved roots are printed at startup. `ask_file`/`ask_files` have a defined
  read scope (a configured root set), and refuse out-of-scope paths — not
  a blanket workspace-wide grant.
- F8. Every decision is appended to a **per-process segment file**; the store
  itself is opened **read-only** by the server. `laya-cli`'s `laya ledger compact`
  is the only writer (blocker B4). **Implemented:** the server's recorder takes a *directory*
  (`laya_store::append_segment`), so it is structurally incapable of opening the store — the
  collision is impossible, not merely avoided. `LAYA_LEDGER_DIR` unset means no ledgering, and
  that is deliberate: bookkeeping that silently started writing somewhere unexpected would be
  worse than none. A ledger failure is reported in the response under `ledger` and does **not**
  fail the decision — but it is never swallowed, because a ledger that has stopped recording is
  worse than one that never did (every later claim about it keeps reading as measured).
- F8a. `laya ledger` subcommands: `compact` | `stats` | `tail [N]` | `outcome`.
  `stats` prints **NO OUTCOMES RECORDED** when there are none, so the calibration gap is visible
  rather than implied.
- F9. `ask_file`/`ask_files` mark file content as **DATA** in the state, never as
  instructions (the reference design's own warning: *"state is not treated as hostile"*).
- F10. The seam is a precisely-named node factory (finding m3): registered in **both**
  a server's `registry()` **and**
  `lint_registry()`, with `ports()`/`required_caps()`/`effects()` — a net-calling
  node is **not** `PURE`.

### Non-Functional
- NF1. **No network listener** — stdio only. This is what makes it usable inside a
  container with no inbound access.
- NF2. Zero clippy warnings, zero fmt diffs, no `unwrap`/`expect`/`panic`.
- NF3. A per-integration-point **latency budget**, measured on the real path
  (review M2). The claim "cheap enough for EVERY tool call" is **withdrawn**: the
  HTTP backbone path measured 608 ms, and cold load measured 23 s for the Python
  stack, so a hook lane needs a persistent server, not a per-call spawn.
  **Implemented:** `latency_ms` is recorded per decision from a clock started at the top of the
  handler. It read **0 ms** in the first version because it measured the process's *uptime*
  instead of the call — a number that looks measured and is not. Only the end-to-end proof
  caught it, because the field was present, well-formed and plausible. Measured for real:
  **436 ms** for a live two-question `decide`.
- NF4. redb pin: **4.2**, chosen rather than inherited. Recorded because the
  original version claim was half wrong (review F10).

## Architecture

```
Hermes ─┐
pi ─────┤  laya-rs  (MCP stdio, JSON-RPC 2.0 on stdin/stdout)
Claude ─┤    decide | ask_file | ask_files | gate
skills ─┤    should_compact | ask | ledger          (NO `route` — finding M6)
hooks ──┘                    │
             ┌───────────────┴──────────────────┐
             │                                  │
   segment files (append-only JSONL)      python sidecar (PINNED version)
   ledger-<pid>.jsonl                     = the head, until P4 replaces it
             │                                  │
             ▼                                  ▼
   `laya ledger compact`               llama.cpp :8083 /embedding
   = THE ONLY WRITER
```

## TDD Contract

The tests that exist, by their real names. (`cargo test -p <crate>` to run them.)

| Test Name | Crate | Asserts |
|---|---|---|
| `a_scope_refuses_a_path_outside_its_root` | `laya-core` | the read scope, not a blanket grant |
| `a_scope_refuses_a_traversal_that_escapes_the_root` | `laya-core` | `..` cannot escape |
| `ask_file_refuses_an_out_of_scope_path_even_when_the_head_is_up` | `laya-mcp` | refusal is not a readiness artefact |
| `file_content_is_marked_as_data` | `laya-core` | `to_state()` labels content UNTRUSTED, never instructions |
| `a_failure_is_never_a_successful_result_with_an_error_prefix` | `laya-mcp` | the protocol error surface (F5) |
| `an_unknown_tool_is_an_invalid_params_error` | `laya-mcp` | unknown tools are refused |
| `gate_with_a_dead_sidecar_answers_ask_and_never_allow` | `laya-mcp` | fail closed (F6) |
| `decide_with_a_dead_sidecar_is_an_error_not_a_fabricated_answer` | `laya-mcp` | no invented answers |
| `the_still_unimplemented_tools_say_so` | `laya-mcp` | an unimplemented tool cannot read as success |
| `payload_exposes_the_two_route_keys` | `laya-core` | `label` + `band`, the fields a graph matches |
| `label_is_always_one_of_the_declared_options` | `laya-core` | the edge set is fixed |
| `two_writers_on_one_store_is_a_typed_error_not_a_panic` | `laya-store` | **B4**: `AlreadyOpen`, not a crash |
| `compact_merges_across_many_processes_segments` | `laya-store` | N consumers, one writer |
| `compact_is_idempotent_because_records_are_recognised_by_identity` | `laya-store` | a retry inserts nothing |
| `compact_moves_segments_aside_and_never_deletes_them` | `laya-store` | the audit trail survives |
| `compact_continues_the_sequence_rather_than_restarting_it` | `laya-store` | a second merge cannot overwrite |
| `a_malformed_line_is_an_error_naming_the_file_and_line_not_a_skip` | `laya-store` | no silent row loss |
| `an_outcome_is_a_separate_record_so_the_store_stays_append_only` | `laya-store` | outcomes do not edit decisions |
| `an_outcome_for_an_unknown_decision_is_refused` | `laya-store` | nothing unjoinable is stored |
| `stats_reports_zero_outcomes_so_the_calibration_gap_is_visible` | `laya-store` | the gap is stated |
| `latency_ms_is_required_because_the_budget_claim_depends_on_it` | `laya-store` | a null latency cannot parse |
| `a_percentile_of_nothing_is_none_not_zero` | `laya-store` | unknown is not zero |

**End-to-end, through the real binaries** (`scripts/archive/prove-ledger.py`, not `cargo test`): a real
`decide` writes a segment and **does not create the store**; `compact` merges it and moves it
aside; a second compact inserts **zero** rows; `stats` reports the real latency; the outcome
path records a judgement and refuses what cannot be joined; two concurrent writers both exit
cleanly.

## Exit Criteria
- [x] `cd /projects/laya-rs && cargo test -p laya-mcp -p laya-store` — all pass (58 tests; note that before this, `laya-store` contributed **0** and the criterion passed vacuously)
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` — zero
- [x] `cd /projects/laya-rs && cargo fmt --all --check` — zero diffs
- [x] the seven tools are listed — **the previous one-liner here was WRONG and could not
  pass**: it sent `tools/list` as the first message, which the protocol forbids (the server
  correctly answers `-32602`, then *"expect initialized request, but received: ListTools"*).
  The handshake must come first:
  `printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"x","version":"1"}}}\n{"jsonrpc":"2.0","method":"notifications/initialized"}\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n' | timeout 20 cargo run -q -p laya-mcp | tail -1`
  → `ask, ask_file, ask_files, decide, gate, ledger, should_compact` (no `route`)
- [x] the two-process redb test passes (blocker B4 proven, not assumed) —
  `two_writers_on_one_store_is_a_typed_error_not_a_panic`, plus `scripts/archive/prove-ledger.py`
  for the no-panic property under a real race

## Guardrails
- Do NOT expose `route` as a tool (M6) — it contradicts the design's own thesis.
- Do NOT return a failed `gate` as a success (fail-open by accident).
- Do NOT grant a blanket filesystem scope to `ask_file`/`ask_files`.
- Do NOT let a server process write the shared store (B4); segments + one writer.
- Do NOT spawn a Python interpreter per call (B3) — the sidecar is persistent and
  version-pinned.
- Do NOT claim per-tool-call latency affordability without measuring the real path
  (M2).
- Do NOT treat Laya's state as trusted: a file's content is DATA, and a model gate
  is **one signal among several**.

### Measured limit: the head reads a PREFIX of the state (2026-09-30)

The tool schema used to promise `ask_files` would be "asked in parallel". Two measured
facts make that promise false, and both are now stated in the contract rather than implied:

- **The sidecar is strictly serial.** It accepts one connection and serves it in a blocking
  loop, so parallel asks queue. `ask_files` therefore asks **sequentially** and reports
  `parallel: false`. (The BACKBONE is 5.8x parallel — the single-process head is not.)
- **The head judges a fixed prefix of the state, not the whole of it.** A signal placed at
  the START of the content scored exactly `0.788` at 3,975 / 15,943 / 63,951 and 199,951
  bytes — **invariant across a 50x size range**, which is only possible if the tail is not
  read. A signal at the END survived at 984 bytes and was completely gone at 3,976,
  scoring `0.087` — identical to the no-signal control (`0.0873`).

  The exact boundary could **not** be pinned: the filler's own semantics move the answer, so
  the result is a documented bound, not a threshold. `FULL_ATTENTION_BYTES = 1024` is
  therefore set conservatively *below* the smallest observed failure.

  Consequence, and it is the important one: `ask_file`'s old message said "refusing rather
  than truncating". That is true of US and **false of the model**. Both file tools now
  return `bytes` and attach a `warning` above the bound, because a verdict that silently
  covers the first page of a 200 KiB file is worse than a refusal — nothing in the answer
  would say so.
