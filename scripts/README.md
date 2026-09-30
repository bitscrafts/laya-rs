# scripts/

Everything here is **load-bearing and versioned**. The distinction that matters:
these are re-runnable, and their results are the numbers the documentation quotes.

| Script | What it does |
|---|---|
| [`check-real-crates.sh`](check-real-crates.sh) | asserts the test count spans the whole workspace, not three crates |
| [`install-host-service.sh`](install-host-service.sh) | first attempt at the host unit (kept: documents what was wrong) |
| [`install-host-service2.sh`](install-host-service2.sh) | the WORKING host unit installer, referenced by SUPERVISION.md |
| [`measure-and-prove.py`](measure-and-prove.py) | the redb two-process collision proof (DatabaseAlreadyOpen) |
| [`measure-concurrency.sh`](measure-concurrency.sh) | the /embedding concurrency measurement (35 ms median, 5.8x parallel) |
| [`poc-mcp-full.py`](poc-mcp-full.py) | the end-to-end POC: MCP client -> server -> sidecar -> head |
| [`poc-sidecar.py`](poc-sidecar.py) | the sidecar POC and its fail-closed case |
| [`prove-hermes-path.sh`](prove-hermes-path.sh) | drives the exact Hermes command line to prove which build answers |
| [`regen-index.py`](regen-index.py) | regenerates _docs/INDEX.md + TOC.md and link-checks the tree |
| [`run-analysis.sh`](run-analysis.sh) | dispatches the pi+glm-5.3 design review, referenced by the analysis doc |
| [`test-mcp-e2e.py`](test-mcp-e2e.py) | the real-stdio MCP e2e test (up and down cases) |
| [`verify-all.sh`](verify-all.sh) | the verification gate: fmt + clippy -D warnings + test, whole workspace |

## The gate

```bash
bash scripts/verify-all.sh      # fmt + clippy -D warnings + test, whole workspace
```

> **Why this file is here and not in `_tmp/`.** `verify-all.sh` used to live in `_tmp/`,
> which is gitignored scratch. The project's gate was therefore invisible: nothing
> referenced it, it was not versioned, and a fresh clone had no way to verify anything.
> A gate in a scratch directory is not a gate. Moved here 2026-09-30.

## Not here

One-shot patch scripts (the `fix-*.py` / `wire-*.py` family that edited the source
once) are archived under `_docs/99-archive/one-shot-scripts-*/`. They are kept for
provenance, and deliberately NOT in `scripts/`, because re-running an already-applied
patch fails or double-applies — a script that looks runnable and is not is a trap.

## The Laya runtime

| Script | What it does |
|---|---|
| [`setup-runtime.sh`](setup-runtime.sh) | creates the pinned `.venv-laya` (laya 0.3.5) and the `laya-sidecar` launcher |
| [`laya-sidecar.py`](laya-sidecar.py) | the decision head as a persistent unix-socket service |

