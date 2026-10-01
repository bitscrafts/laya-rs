# CLAUDE.md — laya-rs

Project instructions for Claude Code. **`AGENTS.md` is the authoritative project file** — read it
first; this one adds the workflow for working *in this repo* and the conventions that are easy to
get wrong. Where the two disagree, `AGENTS.md` wins.

## What this project is

A **System-1 decision layer**: instead of asking an LLM to generate free-form output, a typed
question with declared options is answered by a small deterministic head, and the answer is a
routing **label** a graph edge can match. Laya supplies values; the LLM does open work; the DAG
owns every edge.

- **`laya-rs` is STANDALONE.** Never modify, depend on, or cite another project's files or line
  numbers. A property relied on from outside is stated as a *fact about an interface* and its
  consequence is enforced by a test in this repo.
- The decision head is a **26.5M-parameter ModernBERT reproduction in pure Rust** (`laya-head`),
  differentially proven against the Python reference (worst logit diff 4.05e-6).

## Read this before you touch anything

| what | where |
|---|---|
| project rules, crate boundaries, the safety findings | `AGENTS.md` |
| what is true right now, and what is next | `HANDOFF.md` |
| the design, and the corrections to it | `_docs/06-research/01-design.md`, then `_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md` |
| **the failures already paid for, and the rule each one produced** | `_docs/04-development/CODE-REVIEW-2026-09-30.md`, and the "verification defects" section of `HANDOFF.md` |
| the implementation contracts | `specs/` (01–07, all complete) |
| every doc, indexed | `_docs/INDEX.md` |

*(A workspace-level `DO-NOT-REPEAT.md` exists at `/projects/docs/`, but this project is
STANDALONE — it never cites or depends on files outside this repository, so that index is
deliberately not referenced here. This repo's own equivalent is the two rows above.)*

## The one command that tells the truth

```bash
bash scripts/verify-all.sh        # VERDICT: ALL CHECKS PASS | FAILURES PRESENT
```

It runs the gate, the test count, clippy, fmt, doc links, **every citation in every doc**, the
commit-hygiene scan, the specs' own exit criteria, the payload contract live, and the live corpus
parity — then derives a single verdict. **Exit 0 from this script means the checks passed**; an
exit code from any single command does not.

```bash
cargo xtask gate                  # fmt + clippy -D warnings + test
cargo test --workspace            # 414 tests as of 2026-10-01
cargo clippy --workspace --all-targets -- -D warnings
```

## Working here — the conventions that bite

**Rust bar is ZERO, not "below baseline".** `clippy` at `-D warnings` and `cargo fmt --all --check`
must both be clean. The workspace denies `unwrap_used`, `expect_used`, `panic`, `missing_docs` and
`unsafe_code`; test modules add `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`
as the existing files do.

**Crate boundaries are load-bearing.**
- `laya-core` is **I/O-free** — no HTTP, no MCP, no file I/O. Adding I/O there is a defect.
- `laya-backbone` is the **only** crate that talks to the network.
- `laya-mcp` must **never** depend on `laya-graph`: the judge does not depend on the executor.
- `laya-graph` depends on **neither `laya-store` nor `redb`**; run records are data in `RunReport`.
- `route` is deliberately **not** an MCP tool. Exposing it would let an agent select edges outside
  any graph — the injection failure mode the design exists to prevent.

**Every check must be able to FAIL.** This project has paid for the alternative repeatedly:
a wrapper that discarded exit codes, criteria that re-entered the runner, `grep -qv .` which
cannot pass, a check whose name and assertion disagreed. When you add a check, add its **control**:
make it fail on purpose and confirm it reports. A check nobody has watched fail is unverified.

**A negative is satisfied by doing nothing.** Requirements phrased "at most N", "never overlaps",
"does not exist" need a near-miss case that must still take the normal path — otherwise a serial
implementation, or an empty pattern, passes.

**The spec is the contract; criteria live in it.** Run `python3 scripts/run-exit-criteria.py`, which
executes each spec's own criteria from its own text. A criterion that is never run is
indistinguishable from one that passed.

## Commits

**Never name an AI assistant or tool — in the message, the author name, or the email.** Commits
must read as a human engineer's. This is enforced: `scripts/check-commit-msg.sh` is installed as
the `commit-msg` hook and is a gate step, so a message naming a tool is refused at write time.
Describe the *change*, not who typed it.

```sh
git config user.name    # check — do not assume
```

Workers do **not** commit: leave work in the tree and let the orchestrator review the diff.

## Dispatching work to another agent

**Always from a script, never an inline command** — `scripts/run-spec-part.sh <N>` prints the
model, spec, brief and payload size before spending anything, sets its own env, records the session
id and the pre-dispatch HEAD. Verify with `scripts/verify-spec-part.sh <N>`, which drives the real
artefact and prints a PASS/FAIL row per criterion. **A worker's report is a self-report**; its exit
code is not evidence, the tree is.

## This repo has a knowledge graph

`graphify-out/` holds a queryable graph of this codebase (gitignored, regenerable). When asked a
question about the codebase's structure — "how does X work", "what calls Y", "trace Z" — **query the
graph first**:

```bash
graphify query "how does the route node select an edge?"
graphify explain "route_exec"
graphify path "Flow" "RunReport"
```

If `graphify-out/graph.json` is missing, rebuild with `graphify .` from the repo root.
