# AGENTS.md — laya-rs

**Project**: `/projects/laya-rs` — a Rust System-1 decision layer (Laya over MCP
stdio), feeding deterministic DAG `route` nodes.

Read this before doing anything here. It is the operating contract for agents.

## What this project is, in one paragraph

Laya is a small calibrated decision model (421M ModernBERT backbone + a 26.5M
parameter head). Unlike an LLM it generates no text: you send a state plus typed
questions (`noul` / `choice` / `score`) and get typed answers with probabilities in
one parallel pass. `laya-rs` exposes it as **one Rust binary speaking MCP over
stdio**, so Hermes, pi, Claude Code, skills and hooks all consume the same tool
surface with no network listener; every decision lands in a redb ledger; and the
payload is **route-shaped** so a DAG branch is `route on = "$/.payload.label"` and
the **graph owns every edge** — the model can only select among ports that already
exist.

## Read these first, in this order

1. `_docs/01-overview/` — what it is.
2. **`_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md`** — **THE CORRECTIONS.**
   The adversarial review found five blockers in the original design. **The
   amendments WIN wherever they conflict.** Do not act on `01-design.md` alone.
3. `_docs/06-research/01-design.md` — the original design (superseded in part).
4. `_docs/06-research/02-analysis-glm-5.3.md` — the full review.
5. `_docs/04-development/TEN-LEVELS-MAPPING.md` — the source material mapping.

## Hard rules

### 1. Destructive commands are banned (absolute)
As everywhere in this workspace: **never `rm -rf` anything that already existed
before this run.** Move aside to a dated backup instead. The only exception is a
scratch dir you created this run with `mktemp -d`. A guard around a destructive
command must **fail safe** — an empty/unset variable must make it REFUSE.

### 2. The graph owns every edge — the model never does
Laya supplies a **value**. It never emits a node id, an edge, or control flow. Any
change that lets the model choose *whether* an edge exists (rather than *which*
pre-declared port*) is a violation of the project's entire purpose.

### 3. Thresholds live in the caller
Every number a decision depends on is a `Policy` field (`laya-core::policy`), in
one reviewable place. Never in a prompt. Never in the model. A threshold change is
a diff, not a prompt edit.

### 4. `band` is computed by us, never by a graph transform
a graph `route` node's expr language is *"only object member access … no arrays, no
functions"*. A `transform` node **cannot** compute a band, and `route`'s `eq` is
exact equality with no ranges. That is why the payload arrives ready to match.
Writing a band transform will look like it works and silently never match.

### 5. A model gate is ONE SIGNAL
The video's own Level 6 gate was routed around (the agent wrote `.env` via a bash
heredoc); its author calls the fix *"an instruction, not a control."* Every gate
names its deterministic companion, and a denied branch is **terminal**.

### 6. `route` is not an MCP tool
Exposing it lets an agent select edges outside any graph — the injection failure
mode the design exists to prevent.

### 7. Fail closed
If the decision source is unavailable, `gate` answers `ask`. Never `allow`.

### 8. No secrets, ever
Never in code, docs, the ledger, tool results, or an error message. The ledger
stores `state_sha256`, never the state. `ask_file` reads only within its configured
scope.

### 9. Tests are offline; the model is not needed for correctness
`laya-abi` / `laya-core` / `xtask` tests run with **zero model calls**. Anything
needing Laya to judge *meaning* is a **live** test and is kept separate (the
video's `just test` vs `just test-live` split). If a unit test needs the model, the
design is wrong.

### 10. Rust bar: ZERO, not "below baseline"
`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` are **denied**; `unsafe_code`
is **forbidden**. `cargo xtask gate` = fmt + clippy (`-D warnings`) + test, all
zero. There is one documented upstream allowance (`multiple_crate_versions`, rmcp
pulls syn 2 and 3) with its reason in the workspace manifest.

### 11. Laya usage is ALWAYS and AUTOMATIC — saving provider tokens is MANDATORY

(User mandate 2026-10-06, verbatim: *"laya must be used always and automatically"*,
*"saving tokens is mandatory!"* — correcting a report that counted only manual calls.)

- **The automatic path must stay wired**: the `pre_tool_call` gate hook (fail-closed,
  every `terminal`/`write_file`/`patch`/`execute_code` of every session) is the
  mandatory-usage mechanism — an agent never bypasses it; `LAYA_GATE_DISABLE` and
  `HERMES_SAFE_MODE` are human escape hatches only. Wiring check: `laya.log` grows per
  state-changing call.
- **The retrieval path must be preferred**: a typed `laya ask`/`ask_files` question that
  can answer over the read roots (`/projects:/root`) REPLACES reading the artifact into
  context (measured: 417,039 B of files → 9,678 B of answers, 0 provider tokens per
  call). `ask` never blocks and never writes — using it is free.
- **Usage and savings are measured, not assumed**: ledger outcomes get recorded
  (`laya ledger outcome <id> --correct|--wrong`) after real decisions; a session that
  read large artifacts without a single `ask` is a defect, auditable via
  `laya.log` + the ledger.
- **The gate's negative branches must be exercised**: DENY never fired in production
  (5072 decisions) and outcomes sat at 0 — an always-pass gate is decoration. The
  standing test bed is the **`autoresearch-loop` skill**
  (`/root/.hermes/skills/software-development/autoresearch-loop/`, selftest-proven
  2026-10-06): its KEEP seam is a real commit path this project's gate may refuse —
  wired (driver records laya's verdict per experiment row; enforcement flips on
  calibration proof). Status 2026-10-06: spec 28 deployed (decision_id on every
  payload, outcome joins verified live); spec 29 = gate_deny is next.
- **The saving lane is enforced, not optional (SPEC-B live)**: read_file of a large
  askable artifact is block-once-then-allow with a paste-ready `ask_file` call
  (`/root/.hermes/hooks/laya-ask-nudge.sh`, delegated from the gate hook; selftest
  22/22). Silent 100 KB reads instead of a laya ask are the behaviour this rule
  forbids; explicit override = `LAYA_NUDGE_DISABLE=1`.

## TDD — non-negotiable

**RED before GREEN.** Write the failing test, watch it fail, then implement.

- No production code without a failing test first.
- One behavior per test; clear name describing behavior.
- **Controls are mandatory.** Every rule that skips, flags or suppresses something
  needs a near-miss case that must take the normal path — e.g. saturation is
  tested against `[1.000,0,0]` (flagged) *and* `[0.998,0.001,0.001]` (not flagged),
  or the flag becomes noise nobody reads.
- Drive the **live artefact**, never a source constant. A check whose input is
  re-typed by hand proves the typist, not the system.
- **When a test disagrees with the code, suspect the test first.** (This happened
  here: a noul-band assertion was arithmetically wrong; the code was right.)
- Specs carry the TDD contract in `specs/`. Add the test row before the code.

## Workflow

```
spec (specs/NN-*.md, with the TDD table + runnable exit criteria)
  → RED    : write the failing test, watch it fail
  → GREEN  : minimal code to pass
  → REFACTOR
  → gate   : cargo xtask gate   (fmt + clippy -D warnings + test, all zero)
  → docs   : update _docs/INDEX.md + _docs/TOC.md in the SAME pass
```

**The index is the part that rots.** Adding a doc without indexing it is half the
work. `scripts/regen-index.py` regenerates INDEX/TOC from the directory listing *and*
verifies every relative link resolves — run it, do not hand-edit the index.

## Documentation (SSOT)

- **There is exactly ONE project documentation folder: `_docs/`.** The earlier
  `docs/` (three pointer stubs left by a move) was archived to `docs.old-<ts>/` —
  two folders for one project's docs is the ambiguity SSOT exists to remove, and
  pointer stubs are a second location even when they are only redirects.
- `/projects/docs/` is a **different tree** — the workspace-level docs (the open
  register, workspace rules). Do not merge it in, and do not repoint it.
- Each fact lives in **exactly one place**; link, don't duplicate.
- Research write-ups are numbered: `_docs/06-research/NNN-topic.md`.
- `docs.old-<timestamp>/` is an ARCHIVE, not a second tree: the three pointer
  stubs that used to live in `docs/`. Archived rather than deleted (this
  workspace never hard-deletes). Nothing should link into it.
- **Never erase a superseded claim** — annotate it and point at the current truth.
  The disagreement is the finding.
- **This project is STANDALONE: nothing outside is linked, copied, or named.**
  *This corrects an earlier entry here that read "contracts are linked, never copied" — the
  direction is now stricter (2026-09-30): no association with anything outside this
  repository at all. A link couples us to something we cannot verify from here; a copy
  rots. So we do neither: a property we rely on from outside is stated as a fact about an
  interface, and its consequence is enforced by a test in our own code, so nothing external
  can move our behaviour silently.*
- Never relay a status claim without re-checking the primary artifact. "Status:"
  fields, "(verified)" headings and per-row `Source:` columns prove *where* a
  number was read, never *when*.

## Never do

- Edit `hermes-agent` source (upgrades revert it); fixes go in stdlib skills.
- Touch files outside this project (or the specific project under work).
- Commit — the orchestrator commits, workers leave work in the tree.
- Trust a worker's self-report; verify the artifact.

## Current state (2026-09-29)

- `laya-abi` + `laya-core` + `xtask`: **implemented, 42 tests green, clippy 0, fmt
  clean** (specs 01, 02).
- `laya-backbone` / `laya-head` / `laya-store` / `laya-mcp` / `laya-cli`: scaffolds
  only. Specs 03 and 04 are written and NOT started.
- The design was adversarially reviewed; **read the amendments before implementing
  anything in P1 or P4.**
- Known-unmeasured: the Rust head's cold-load time (decides the hook lane), and
  llama.cpp determinism across thread counts/builds.

## SCOPE — THIS PROJECT ONLY

**No association with any other project.** `/projects` holds many; this one is ours alone.

- Never modify, commit in, or "fix while I am here" another project.
- Never take a code dependency on another project here.
- Never treat another project's state, specs or HEAD as ours.
- Never cite another project's file paths or line numbers in our docs: state what we rely on
  as a **fact about the interface**, so the document stands on its own.

When something elsewhere shows a contract we need, we **restate it in our own code** — see
`crates/laya-core/src/route_payload.rs`, which names the two fields a graph matches on and pins
them by test. That is why a change outside this repository cannot move our behaviour silently.

**A DAG runner belongs HERE**, in this project — not in whatever project we borrowed an idea from.

When the reference shows a contract we need, we **copy the shape into our own code** and cite it
— we do not reach across and change it. Our `route_payload.rs` is exactly that: the contract
restated in our own crate, so a change over there cannot move our behaviour silently.

**A DAG runner belongs here, in `laya-rs`** — not in the project we borrowed the idea from.
