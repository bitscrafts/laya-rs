# CLAUDE.md — laya-rs

Project instructions for Claude Code. **[`AGENTS.md`](AGENTS.md) is the authoritative project
file**: read it first. This one adds the workflow for working in this repo and the conventions
that are easy to get wrong. Where the two disagree, `AGENTS.md` wins.

## What this project is

A **System-1 decision layer**: instead of asking an LLM for free-form output, a typed question
with declared options is answered by a small deterministic head, and the answer is a routing
**label** that a graph edge can match. Laya supplies values; the LLM does open work; the DAG owns
every edge.

- **`laya-rs` is standalone.** Never modify, depend on, or cite another project's files. A
  property relied on from outside is stated as a *fact about an interface*, and its consequence is
  enforced by a test in this repo.
- The decision head is a **26.5M-parameter reproduction in pure Rust** (`crates/laya-head`),
  differentially checked against the Python reference (worst logit difference 4.05e-6).

## Where things are

| what | where |
|---|---|
| project rules, crate boundaries | [`AGENTS.md`](AGENTS.md) |
| what is true now, and what is next | [`HANDOFF.md`](HANDOFF.md) |
| the design, and the corrections to it | [`_docs/06-research/01-design.md`](_docs/06-research/01-design.md), then [`_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md`](_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md) |
| failures already paid for, and the rule each produced | [`_docs/04-development/CODE-REVIEW-2026-09-30.md`](_docs/04-development/CODE-REVIEW-2026-09-30.md) |
| implementation contracts | [`specs/`](specs/README.md) |
| every doc, indexed | [`_docs/INDEX.md`](_docs/INDEX.md) |
| how to run it | [`_docs/02-guides/RUNNING-LAYA.md`](_docs/02-guides/RUNNING-LAYA.md) |

## The commands

```bash
cargo xtask gate                          # fmt + clippy -D warnings + test
cargo test --workspace                    # offline; the live tests are #[ignore]d (`-- --ignored` runs them)
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/verify-all.sh                # the whole picture, with one verdict line
```

`scripts/verify-all.sh` runs the gate, the test count, clippy, fmt, the doc links, **every citation
in every doc**, the commit-message scan, the specs' own exit criteria, and the live checks. It ends
with `VERDICT: ALL CHECKS PASS | FAILURES PRESENT`. The live checks need the backbone server; with
it down they report UNMEASURED, never a silent pass. Exit 0 from this script means the checks
passed; an exit code from a single command does not.

## Working here: the conventions that bite

**The Rust bar is zero.** Clippy at `-D warnings` and `cargo fmt --all --check` must both be
clean. The workspace denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
`missing_docs` and `unsafe_code`; test modules add
`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` as the existing files do.

**Crate boundaries are load-bearing.**
- `laya-core` is **I/O-free**: no HTTP, no MCP, no file I/O. Adding I/O there is a defect.
- `laya-backbone` is the **only** crate that talks to the network.
- `laya-mcp` must **never** depend on `laya-graph`: the judge does not depend on the executor.
- `laya-graph` depends on **neither `laya-store` nor `redb`**; run records are data in `RunReport`.
- `route` is deliberately **not** an MCP tool, so an agent cannot select edges outside any graph.

**Every check must be able to fail.** This project has paid for the alternative repeatedly: a
wrapper that discarded exit codes, criteria that re-entered the runner, `grep -qv .` which cannot
pass, a check whose name and assertion disagreed. When you add a check, add its **control**: make it
fail on purpose and confirm that it reports. A check nobody has watched fail is unverified.

**A negative is satisfied by doing nothing.** Requirements phrased "at most N", "never overlaps"
or "does not exist" need a near-miss case that must still take the normal path, otherwise a serial
implementation, or an empty pattern, passes.

**The spec is the contract; the criteria live in it.** `python3 scripts/run-exit-criteria.py`
executes each spec's own criteria from its own text. A criterion that is never run is
indistinguishable from one that passed. Each spec's second line is a card,
`#: depends=… status=… block=…`, so its state can be read with `grep`.

**Scripts.** `scripts/` holds what the gate and the specs run. `scripts/live/` holds checks that
need the running stack. `scripts/archive/` holds one-shot scripts from the build-out, kept for
history and not maintained; see its README.

## Commits

**Never name an AI assistant or tool, in the message, the author name or the email.** Commits must
read as a human engineer's. `scripts/check-commit-msg.sh` enforces this; it is also a step in
`verify-all.sh`. Install it as a local hook once per clone:

```sh
ln -s ../../scripts/check-commit-msg.sh .git/hooks/commit-msg
git config user.name    # check; do not assume
```

Describe the *change*, not who typed it. Workers do **not** commit: leave the work in the tree and
let the reviewer inspect the diff.

## Dispatching work to another agent

**Always from a script, never an inline command.** `scripts/run-spec-part.sh <N>` prints the model,
spec, brief and payload size before spending anything, sets its own environment, and records the
session id and the pre-dispatch HEAD. Verify with `scripts/verify-spec-part.sh <N>`, which drives
the real artefact and prints a PASS/FAIL row per criterion. **A worker's report is a self-report**:
its exit code is not evidence; the tree is.

## Optional: a knowledge graph of the code

If you have `graphify` installed, `graphify .` builds a queryable graph of this codebase into
`graphify-out/` (git-ignored, regenerable). For structural questions ("what calls X", "how does Y
work"), query it before grepping:

```bash
graphify query "how does the route node select an edge?"
graphify explain "route_exec"
graphify path "Flow" "RunReport"
```
