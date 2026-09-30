# Spec: 04 — The MCP stdio server and the ledger (P1)

## Overview
One Rust binary (`laya-rs`) speaking **MCP over stdio**, so Hermes, pi, Claude Code,
skills and hooks all consume the same tool surface with **no network listener**.
Every decision is recorded in an embedded redb ledger via a single-writer design.

**Status: NOT STARTED.** This spec incorporates the review's corrections: P1 needs
a **pinned Python sidecar** for the head (blocker B3), the ledger uses a
single-writer scheme (blocker B4), and `route` is **not** a tool (finding M6).

**Project**: /projects/laya-rs
**Phase**: P1

## Requirements

### Functional
- F1. Transport is **MCP over stdio**, JSON-RPC 2.0 on stdin/stdout. Copy the
  pattern from `pii-mcp/src/main.rs:144-158` — `stdio()` → `serve_server(server,
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
- F7. Per-tool **least-privilege** scopes. **The scope lives in `laya-core`, not
  `laya-mcp`** (corrected 2026-09-30): the pure crate, so the rule deciding a refusal is
  unit-testable without a server, and so a second copy cannot drift. Configured by
  `LAYA_READ_ROOTS` (colon-separated); **unset means the process cwd, never `/`**, and the
  resolved roots are printed at startup. `ask_file`/`ask_files` have a defined
  read scope (a configured root set), and refuse out-of-scope paths — not
  pii-mcp's blanket workspace-wide grant.
- F8. Every decision is appended to a **per-process segment file**; the store
  itself is opened **read-only** by the server. `laya-cli`'s `laya ledger compact`
  is the only writer (blocker B4).
- F9. `ask_file`/`ask_files` mark file content as **DATA** in the state, never as
  instructions (the video's own warning: *"state is not treated as hostile"*).
- F10. The `laya` node seam in pii is named precisely (finding m3): a new
  `NodeFactory` registered in **both** pii-server's `registry()` **and**
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
- NF4. redb pin: **4.2** (same major as `pi-improved`). Recorded because the
  original claim about amem/pidag was half wrong (review F10).

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

| Test Name | Input | Expected Output |
|---|---|---|
| `server_exits_on_eof` | close stdin | process exits 0, does not park |
| `initialize_and_tools_list` | an MCP handshake | the seven tools, no `route` |
| `route_is_not_exposed` | `tools/list` | no tool named `route` |
| `gate_fails_closed_when_the_source_is_down` | sidecar stopped | `ask`, never `allow` |
| `a_tool_error_is_a_protocol_error` | an induced failure | a JSON-RPC error, not a success with `"error:"` text |
| `ask_file_refuses_outside_its_scope` | `/etc/passwd` | refused, with the scope named |
| `two_servers_can_run_at_once` | two processes | both append to their own segment; no lock error |
| `the_store_is_opened_read_only_by_the_server` | a server process | no write handle on `ledger.redb` |
| `compact_merges_segments` | N segments | one store, row count == sum |
| `a_redb_write_conflict_is_a_typed_error` | two writers, one store | a typed error, **not** a panic (B4) |
| `payload_includes_margin_and_provenance` | a decision | `margin`, `laya_version`, hashes present |
| `file_content_is_marked_as_data` | a hostile file | content labelled DATA in the state |
| `latency_is_recorded_per_call` | any call | `latency_ms` present in the ledger row |

## Exit Criteria
- [ ] `cd /projects/laya-rs && cargo test -p laya-mcp -p laya-store` — all pass
- [ ] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` — zero
- [ ] `cd /projects/laya-rs && cargo fmt --all --check` — zero diffs
- [ ] `printf '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | timeout 10 cargo run -q -p laya-mcp | grep -q decide`
- [ ] the two-process redb test passes (blocker B4 proven, not assumed)

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
