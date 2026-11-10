# AGENTS.md — laya-rs

A Rust **System-1 decision layer**: a small calibrated model answers typed questions over
MCP stdio, and the answer feeds deterministic DAG `route` nodes. This file is the operating
contract for anyone, human or agent, changing this repository. `CLAUDE.md` adds the day-to-day
workflow; where the two disagree, this file wins.

## What this project is

Laya is a small calibrated decision model (a 421M ModernBERT backbone plus a 26.5M-parameter
head). It generates no text: you send a state and typed questions (`noul`, `choice`, `score`) and
get typed answers with probabilities in one pass. `laya-rs` exposes it as **one Rust binary
speaking MCP over stdio**, so any agent host, skill or hook consumes the same tool surface with
no network listener. Every decision lands in a redb ledger, and the payload is **route-shaped**:
a DAG branch is `route on = "$.payload.label"`, so the **graph owns every edge** and the model
can only select among ports that already exist.

## Read these first, in this order

1. [`_docs/01-overview/README.md`](_docs/01-overview/README.md) — what it is.
2. [`_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md`](_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md) —
   **the corrections.** An adversarial review found five blockers in the original design; the
   amendments win wherever they conflict with the design.
3. [`_docs/06-research/01-design.md`](_docs/06-research/01-design.md) — the original design (superseded in part).
4. [`_docs/06-research/02-analysis-glm-5.3.md`](_docs/06-research/02-analysis-glm-5.3.md) — the full review.
5. [`_docs/04-development/TEN-LEVELS-MAPPING.md`](_docs/04-development/TEN-LEVELS-MAPPING.md) — the source material, mapped to our tools.
6. [`HANDOFF.md`](HANDOFF.md) — what is true now and what is next.

## Hard rules

### 1. Nothing pre-existing is deleted
Never `rm -rf` anything that existed before the run. Move it aside to a dated backup. The only
exception is a scratch directory you created with `mktemp -d`. A guard around a destructive
command must **fail safe**: an empty or unset variable must make it refuse.

### 2. The graph owns every edge; the model never does
Laya supplies a **value**. It never emits a node id, an edge or control flow. Any change that
lets the model choose *whether* an edge exists, rather than *which* pre-declared port, defeats
the purpose of the project.

### 3. Thresholds live in the caller
Every number a decision depends on is a `Policy` field (`crates/laya-core/src/policy.rs`), in one
reviewable place. Never in a prompt, never in the model. A threshold change is a diff.

### 4. `band` is computed by us, never by a graph transform
A graph `route` node's expression language has only object member access, no arrays and no
functions, and `eq` is exact equality with no ranges. A `transform` node cannot compute a band,
so the payload arrives ready to match. A band transform would look like it works and silently
never match.

### 5. A model gate is one signal
A gate that only a model enforces can be routed around (an agent can write a file through a shell
heredoc instead of the guarded tool). Every gate names its deterministic companion, and a denied
branch is **terminal**.

### 6. `route` is not an MCP tool
Exposing it would let an agent select edges outside any graph, which is the injection failure
mode the design exists to prevent.

### 7. Fail closed
If the decision source is unavailable, `gate` answers `ask`, never `allow`. (A caller that must
stay available when the source is down chooses that itself, with a timeout; see
`laya decide --timeout-ms`.)

### 8. No secrets, ever
Never in code, docs, the ledger, tool results or an error message. The ledger stores
`state_sha256`, never the state. `ask_file` reads only within its configured scope.

### 9. Tests are offline; the model is not needed for correctness
`laya-abi`, `laya-core` and `xtask` tests run with **zero model calls**. Anything that needs Laya
to judge *meaning* is a **live** test, kept separate and marked `#[ignore]`. If a unit test needs
the model, the design is wrong.

### 10. The Rust bar is zero
`unwrap_used`, `expect_used`, `panic` and `indexing_slicing` are **denied**; `unsafe_code` is
**forbidden**. `cargo xtask gate` is fmt, clippy with `-D warnings`, and test, all at zero. The
toolchain is pinned in `rust-toolchain.toml`, so the bar is the same on every machine. One
documented upstream allowance exists (`multiple_crate_versions`, because the MCP library pulls two
major versions of `syn`), with its reason in the workspace manifest.

### 11. Usage is automatic, and is measured
When this layer is wired into an agent host, the pre-tool-call gate hook is the mechanism that
makes its use automatic and fail-closed; the hook itself is host configuration and lives outside
this repository. Two properties hold wherever it is wired:

- A typed `ask`/`ask_files` question that can answer over the configured read roots replaces
  reading the artifact into the agent's context. `ask` never blocks and never writes.
- Usage and benefit are measured from the ledger (`laya ledger stats`, and
  `laya ledger outcome <id> --correct|--wrong` after real decisions), not assumed. A gate that has
  never denied anything is decoration; its negative branch must be exercised.

## TDD — non-negotiable

**Red before green.** Write the failing test, watch it fail, then implement.

- No production code without a failing test first. One behaviour per test, with a name that
  describes the behaviour.
- **Controls are mandatory.** Every rule that skips, flags or suppresses something needs a
  near-miss case that must still take the normal path. Saturation, for example, is tested against
  `[1.000, 0, 0]` (flagged) *and* `[0.998, 0.001, 0.001]` (not flagged), or the flag becomes noise.
- Drive the **live artefact**, never a source constant. A check whose input is re-typed by hand
  proves the typist, not the system.
- **When a test disagrees with the code, suspect the test first.**
- Specs carry the TDD contract in [`specs/`](specs/README.md). Add the test row before the code.

## Workflow

```
spec (specs/NN-*.md: TDD table + runnable exit criteria)
  → RED       write the failing test, watch it fail
  → GREEN     minimal code to pass
  → REFACTOR
  → gate      cargo xtask gate      (fmt + clippy -D warnings + test)
  → docs      python3 scripts/regen-index.py   (indexes and link-checks _docs/)
```

**The index is the part that rots.** `scripts/regen-index.py` regenerates `_docs/INDEX.md` and
`_docs/TOC.md` from the directory listing and verifies that every relative link resolves. Run it;
do not hand-edit the index.

## Documentation

- There is one project documentation folder: `_docs/`. Each fact lives in exactly one place; link,
  do not duplicate.
- Research write-ups are numbered, `_docs/06-research/NN-topic.md`.
- **Never erase a superseded claim.** Annotate it and point at the current truth; the
  disagreement is the finding.
- Never relay a status claim without re-checking the primary artefact. A "Status:" field, a
  "(verified)" heading or a `Source:` column proves where a number was read, never when.
- This project is **standalone**: it links to nothing outside the repository. A property we rely
  on from outside is stated as a fact about an interface, and its consequence is enforced by a test
  in our own code, so nothing external can move our behaviour silently. The payload contract is
  restated in `crates/laya-core/src/route_payload.rs`, which names the two fields a graph matches
  on and pins them by test.

## Never do

- Modify another project, or take a code dependency on one.
- Cite another project's file paths or line numbers; state the interface instead.
- Commit as an agent unless asked: workers leave their work in the tree for review.
- Trust a worker's self-report. Verify the artefact.

## Current state

See [`HANDOFF.md`](HANDOFF.md) for the live state and the next steps. In short: every crate in the
workspace is implemented (`laya-head` is the complete pure-Rust decision head, checked against the
reference implementation); `laya decide` runs in-process with no Python; and the MCP server and
gate hook still reach the head through a Python sidecar, which is the main open work.
