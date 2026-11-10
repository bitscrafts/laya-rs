# Spec: 29 — A policy can hard-deny (gate_deny beats ask)
#: depends=- status=done block=-

**Project**: laya-rs
**Phase**: P4 enforcement — closes G2 (deny production-unreachable: 5083 decisions, 0 denies)
**Status**: NOT IMPLEMENTED (structural proof dated 2026-10-06 — see premises)
**Derived from**: `_docs/06-research/05-laya-seam-review-2026-10-06.md` §G2, SPEC-C

## Verified Premises

| # | Premise | Evidence |
|---|---|---|
| V1 | `gate_verdict` forces `Ask` for anything not on `gate_allow`; Deny is reachable ONLY as (allowlisted ∧ label=No ∧ noul High ≥0.8) | laya-core `agentic.rs:556-580`, `:528-539`; deny unit test `:1160-1164` exists and passes |
| V2 | The model measures 0.90–0.99 "safe" even on destructive commands; a confident No on `ls`/`git status` is semantically contradictory | `policy.rs:201-211` measured note; live row: `destructive_signals:2, allow, p=0.9743` |
| V3 | An empty allowlist is a valid policy (precedent for empty-vec fields) | `policy.rs:743-752` |
| V4 | `rm -rf` on pre-existing paths is categorically banned in this workspace yet passes the gate today (signal recorded, not enforced) | AGENTS.md rule 1; the host-side hook records the signal and deliberately does not enforce it (documented there) |

**Appearance-only requirement:** a `gate_deny` checked AFTER the allowlist question can
never fire on the commands it exists for (non-allowlisted input is already `Ask`-forced) —
it looks implemented (unit test with an allowlisted target passes) and denies nothing in
production. The order test below is the control.

## Requirements

### Functional
- F1. `Policy` gains `gate_deny: Vec<String>`: serde-default empty, TOML round-trips,
  validated like the allowlist (no empty-pattern entries).
- F2. `gate_verdict` evaluates `gate_deny` **before** the allowlist question. A match forces
  `Deny` regardless of band, label, or allowlist membership. Deny beats ask, allow beats nothing here.
- F3. Pattern semantics (decided here, not left open): **case-sensitive literal substring**
  match on the command text — the same convention the hook's `destructive_signals` prose
  uses. No globs, no regex.
- F4. The verdict carries WHY: the deny result includes the matched pattern (additive field
  or the existing refusal-reason channel — worker picks the shape that fits `GateVerdict`
  `agentic.rs:510-524`; the name of the matched pattern must be observable in the payload).
- F5. Default policy (`policy.rs` Default impl): `gate_deny = vec![]` — **deny nothing**.
  Deployed enablement is a separate config change the ORCHESTRATOR makes after merge
  (candidate list: `rm -rf`, `of=/dev/`, `git reset --hard`, `git push --force`);
  the code change and the deployed config are deliberately decoupled ships.

### Non-Functional
- Offline tests only (AGENTS.md rule 9); Rust bar zero; forbid(unsafe) untouched.

## TDD Contract (RED first)

| Test Name | Input | Expected Output |
|---|---|---|
| `deny_beats_ask` | policy `gate_deny=["rm -rf"]`, command `rm -rf /tmp/x` (NOT allowlisted) | Deny (not Ask — the order control) |
| `deny_beats_allow` | same, but pattern also allowlisted-adjacent: command `git status && rm -rf /tmp/x` | Deny (deny wins over the allowlisted prefix too) |
| `default_policy_denies_nothing` | default Policy, `rm -rf /tmp/x` | NOT deny (today's Ask/behavior unchanged — the naive always-deny fails this) |
| `routine_survives` | `gate_deny=["rm -rf"]`, `mv a b` | Ask (the routine class must survive — signals are not deny) |
| `matched_pattern_named` | deny fires | payload/reason carries the matched text `rm -rf` |
| `empty_vec_valid_and_toml_roundtrip` | `gate_deny=[]` and a populated one | round-trips through TOML both ways (policy.rs:760-765 pattern) |

## Exit Criteria

- [ ] `cargo xtask gate`
- [ ] `cargo test -p laya-core -- deny_beats_ask default_policy_denies_nothing routine_survives matched_pattern_named 2>&1 | grep -q '4 passed'`  (deny_beats_allow's `&&` defeats its own allowlist half — its property lives in gate_deny.rs `…_stronger`)
- [ ] `grep -q 'gate_deny' crates/laya-core/src/policy.rs`

## Guardrails

- Do NOT widen the allowlist in this spec (`gate_allow` is untouched — adding the seam's
  actions there would let the model ALLOW commits; that direction is wrong).
- Do NOT flip any default beyond the empty deny list: no default-policy behavior change ships
  from this repo (the deployed deny list is the orchestrator's separate, reviewed step).
- Do NOT add regex/globs — F3 fixes the matcher's semantics.
- Workers never commit.