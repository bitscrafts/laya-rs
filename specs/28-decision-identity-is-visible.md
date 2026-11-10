# Spec: 28 — The decision identity is visible (decision_id in the gate payload)
#: depends=- status=done block=-

**Project**: laya-rs
**Phase**: P4 observability — closes G1 (outcomes=0, no calibration feedback) at the source
**Status**: NOT IMPLEMENTED (exit criteria proven to FAIL 2026-10-06T14:23Z)
**Derived from**: `_docs/06-research/05-laya-seam-review-2026-10-06.md` (pi+glm-5.3 analysis, committed `b4ced29`), SPEC-A laya-rs part

## Verified Premises

| # | Premise | Evidence (read this run by the analysis lane; re-check while implementing) |
|---|---|---|
| V1 | `record_decision_for` returns only an error note (or `None`) — the row's identity never reaches the caller | laya-mcp `lib.rs:1327`, `:1416-1419` |
| V2 | The ledger join key is `Record::identity()` = `d␟ts_nanos␟wall_nanos␟tool␟question_id␟label` | laya-store `lib.rs:236-260`, `:781` (record_outcome), CLI `main.rs:384-425` (`laya ledger outcome`) |
| V3 | The gate payload printed by `gate_payload` carries no decision reference today | live probe: `laya-gate "ls" --run-id probe-t \| jq -r '.decision_id'` → `ABSENT` (2026-10-06T14:23Z) |
| V4 | The hook passes `--run-id "hook-$$"` (ephemeral PID) so ledger rows cannot be joined to sessions | the host-side hook that calls `laya-gate` (outside this repo's scope — an orchestrator fix, not the worker's) |
| V5 | Calibration exists and refuses <30 samples | laya-store `calibrate.rs:44-58` |

**Appearance-only requirement (written down so it cannot be "implemented" vacuously):**
a payload field `decision_id` that is a *fresh UUID* instead of the ledger row's `Record::identity()`
looks right, passes a presence check, and joins NOTHING — the outcome write would silently orphan.
The test must compare the payload's decision_id against the identity computed from the ledger row
itself, not merely assert non-empty.

## Requirements

### Functional
- F1. `record_decision_for` (laya-mcp `crates/laya-mcp/src/lib.rs`) returns the recorded decision's
  `Record::identity()` string on success; the existing error-note-on-failure behavior is preserved.
- F2. The payload printed by `gate_payload` gains an additive key `"decision_id"`: the identity string
  when a ledger row was written, `null` when none was (no-ledger mode / ledger write failure).
- F3. `laya-gate` (the hook's binary) requires NO change: it prints the whole payload, so the hook
  inherits the field. A test asserts the binary's output carries it (guards against a wrapper stripping it).
- F4. Orphan outcomes are refused: `laya ledger outcome <id> …` targeting an identity that matches no
  decision row in the store exits non-zero with a named error. If V2's write path already validates
  this, the spec's test pins it; if not, `record_outcome` (laya-store `lib.rs:781`) gains validation.
- F5. Wire shape is otherwise UNCHANGED: existing consumers (the hook's `jq -r '.decision //'`, the MCP
  `gate` tool responses, the ledger schema) keep working — the key is additive, no field is renamed.

### Non-Functional
- Everything stays offline-testable (no model calls in unit tests — AGENTS.md rule 9).
- `#![forbid(unsafe_code)]` and the Rust bar (rule 10: zero clippy/fmt) hold.
- The 450-line file gate is respected; if a touched file would cross it, report back instead of splitting.

## TDD Contract (RED first — each test written and seen failing before its code)

| Test Name | Input | Expected Output |
|---|---|---|
| `payload_carries_decision_identity` | `gate_payload` with ledger active | `decision_id` = non-empty string |
| `decision_id_equals_record_identity` | same call; read the ledger row back, compute `Record::identity()` | payload's `decision_id` == the computed identity (byte-equal) — the anti-vacuous control |
| `no_ledger_no_identity` | gate_payload with `--no-ledger` / LEDGER disabled | `decision_id` == null or absent |
| `laya_gate_binary_passthrough` | the `laya-gate` binary on an allowlisted command | stdout JSON carries non-empty `decision_id` |
| `orphan_outcome_refused` | `record_outcome` with an identity matching no row | Err (named error), not silent accept |
| `known_good_payload_unchanged` | freeze a small golden JSON of the payload pre-change, minus the new key | new output == golden + `decision_id` only (shape control) |

## Exit Criteria

- [ ] `LAYA_LEDGER_DIR=/var/lib/laya-ledger laya-gate "ls" --run-id spec28 | jq -e '.decision_id != null'`
- [ ] `LAYA_LEDGER_DIR=/var/lib/laya-ledger laya ledger outcome "$(LAYA_LEDGER_DIR=/var/lib/laya-ledger laya-gate 'ls' --run-id spec28b | jq -r .decision_id)" --correct --source spec28`
- [ ] `LAYA_LEDGER_DIR=/var/lib/laya-ledger laya ledger stats | grep -q '^outcomes:  *[1-9]'`
- [ ] `git diff --quiet crates/laya-mcp/src/bin/laya-gate.rs` (the binary needs NO edit — verified by the passthrough test; this criterion fails if it was touched)
- [ ] `cargo xtask gate` (fmt + clippy -D warnings + test, all zero)

## Guardrails

- Do NOT rename or re-shape any existing payload/ledger field (F5 is a hard requirement).
- Do NOT invent a UUID/other id — the identity is `Record::identity()`, the same bytes
  `laya ledger outcome` joins on.
- Do NOT touch the host-side hook that calls `laya-gate`, or any agent-host file — outside this
  repo; the orchestrator wires it after deploy.
- Do NOT change Policy/thresholds — that is spec 29's scope only.
- Workers never commit; work stays in the tree for orchestrator review.