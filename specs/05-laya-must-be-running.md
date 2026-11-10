# Spec: 05 — Laya must be running (mandatory preflight)
#: depends=- status=done block=-

## Overview
**User directive (verbatim):** *"it will be mandatory the app to check for laya be
running! and it must inform/stop if not!"*

Every tool in `laya-rs` is worthless — and actively dangerous — if Laya is not
running, because the failure mode of a decision layer that cannot decide is a
**silent wrong default**. This spec makes a reachable Laya a **precondition**:

- the server **refuses to start** when Laya is unreachable;
- it **informs** with the exact reason, the endpoint tried, and the remediation;
- it **re-checks during operation**, so a Laya that dies mid-session is caught;
- when it is down, every tool **fails closed** — `gate` answers `ask`, never
  `allow`; other tools return a protocol error, never an empty success.

**Status: IMPLEMENTED.** `laya-core::readiness` + `laya doctor`; the head probe was added 2026-09-30 after `doctor` was found certifying READY while the head was dead.

**Project**: laya-rs
**Phase**: P1 (blocks everything)

## Requirements

### Functional
- F1. A **pure** readiness decision lives in `laya-core` (no I/O): given a probe
  outcome, decide `Ready` / `NotReady` / `Degraded`, with a human-readable reason.
- F2. The probe itself lives in `laya-backbone` (the only crate that does I/O):
  `GET /health` and `GET /v1/models`, with a timeout.
- F3. **Startup is gated**: the MCP server probes before serving. If Laya is down it
  writes a diagnostic to **stderr** and **exits non-zero**. It does not serve in a
  degraded state pretending to work.
- F4. The diagnostic **informs**: which endpoint was tried, the exact error, and
  what to do about it. An agent reading stderr must be able to act on it without
  reading this source.
- F5. **Re-check during operation**, with a short cache (default 5 s) so a
  per-tool-call check does not add a round trip per call — but a Laya that dies is
  noticed within the cache window rather than never.
- F6. **Fail closed when down**: `gate` → `ask` (**never** `allow`); `decide` and
  the `ask_*` tools → a typed **protocol error** (never an empty or defaulted
  success).
- F7. A `laya doctor` CLI command reports readiness and exits non-zero when not
  ready, so a hook, a cron or a human can check without parsing prose.
- F8. The check is **model-agnostic and cheap**: health of the endpoint, not a real
  decision. It must not consume a decision or corrupt the ledger.

### Non-Functional
- NF1. The readiness *decision* is testable with **zero I/O** — every branch,
  including the down case, is a unit test.
- NF2. A timeout is **not** a hang: the probe has a bounded timeout and its
  expiry is reported as `NotReady`, never as an exception that takes the server
  down silently.
- NF3. The check never logs a secret (an endpoint URL may be logged; a key never).

## Architecture

```
startup ──► probe(/health, /v1/models, timeout)
              │
              ├── ok        ──► serve (MCP stdio)
              └── not ready ──► stderr diagnostic + exit(non-zero)   [F3/F4]
                                  "laya-rs: Laya is not reachable at <url>.
                                   tried: GET <url>/health -> <error>
                                   fix: start the container `llamacpp-laya` and
                                   confirm `curl <url>/health` returns ok.
                                   refusing to start: a decision layer that cannot
                                   decide must not run."

per tool call ──► cached readiness (TTL 5s)
                    │
                    ├── ready    ──► proceed
                    └── not ready ──► gate: Ask   |   others: protocol error
```

**Why fail closed and not fail open:** the whole point of the project is that
decisions arrive as typed values the graph matches on. A `gate` that returns
`allow` because it could not ask is precisely the "single-path gate that gets
routed around" failure the review documented (the reference design's own Level 6), except
self-inflicted. **An unavailable gate must never mean permission.**

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `a_healthy_probe_is_ready` | probe ok | `Ready` |
| `a_connection_error_is_not_ready_with_a_reason` | connection refused | `NotReady`, reason names the error |
| `a_timeout_is_not_ready_not_an_error` | elapsed > timeout | `NotReady`, reason says "timed out" |
| `an_http_5xx_is_not_ready` | status 503 | `NotReady` |
| `an_http_404_on_health_is_not_ready` | status 404 | `NotReady` (wrong endpoint, not a healthy server) |
| `a_malformed_body_is_not_ready` | 200 with junk | `NotReady` — a 200 alone is not proof |
| `ready_when_both_endpoints_answer` | health ok, models ok | `Ready` |
| `degraded_when_health_ok_but_models_empty` | health ok, models `[]` | `NotReady`, reason names the missing model |
| `the_reason_names_the_endpoint_tried` | any failure | reason contains the URL |
| `the_diagnostic_never_contains_a_key` | a probe with an api key set | the formatted diagnostic contains no key value |
| `cache_serves_a_second_check_within_the_ttl` | two checks, 1 s apart | one probe call, not two |
| `cache_re_probes_after_the_ttl` | two checks, TTL elapsed | two probe calls |
| `a_cached_ready_does_not_mask_a_later_failure` | ready, then the probe fails after the TTL | the second check reports not-ready |
| `gate_is_ask_when_not_ready` | `NotReady` | `GateDecision::Ask` — **never** `Allow` |
| `every_other_tool_is_a_protocol_error_when_not_ready` | `NotReady` | an error for `decide`/`ask_file`/`ask_files`/`ask` |
| `doctor_exits_nonzero_when_not_ready` | a down endpoint | non-zero exit |

## Exit Criteria
- [x] `cargo test -p laya-core --lib readiness` — all pass
- [x] `cargo test -p laya-backbone` — all pass
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — zero
- [x] `cargo run -q -p laya-cli -- doctor --url http://127.0.0.1:1; test $? -ne 0`
- [x] `cargo run -q -p laya-cli -- doctor --url http://10.10.10.217:8083` — exit 0 while the container is up

## Guardrails
- Do NOT start serving when Laya is unreachable. Refusing to start is the correct
  behaviour; a degraded server that answers with defaults is the failure this spec
  exists to prevent.
- Do NOT return `allow` from `gate` when the source is down. Closed, always.
- Do NOT let a timeout hang the server; it is a `NotReady`, reported.
- Do NOT let the readiness check consume a decision or write a ledger row — it must
  not cost a real decision, and it must not pollute the ledger.
- Do NOT log a key, ever. The endpoint URL is fine; the credential is not.
- Do NOT cache `Ready` longer than the TTL. A long cache turns "it was up when we
  started" into a silent fail-open.
