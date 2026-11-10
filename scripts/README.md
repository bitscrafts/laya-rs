# scripts/

What the gate, CI and the specs run. Everything here is versioned and re-runnable; its results are
the numbers the documentation quotes.

## Verification

| Script | What it does |
|---|---|
| [`verify-all.sh`](verify-all.sh) | the whole picture: fmt, clippy `-D warnings`, test, doc links, every citation in every doc, the commit-message scan, the specs' own exit criteria, then the live checks. Ends with one `VERDICT:` line |
| [`run-exit-criteria.py`](run-exit-criteria.py) | runs each spec's own exit criteria from its own text |
| [`spec-gate.sh`](spec-gate.sh) | runs one spec's criteria and reports in one line |
| [`spec-all-gate.sh`](spec-all-gate.sh) | is every spec finished? One line, one exit status (~13 minutes for the full sweep) |
| [`mutants-gate.sh`](mutants-gate.sh) | the mutation gate: zero missed mutants on the safety-critical modules |
| [`check-doc-citations.py`](check-doc-citations.py) | every citation in the docs must resolve to something real |
| [`check-commit-msg.sh`](check-commit-msg.sh) | refuses a commit message that names an AI assistant or tool |

## Documentation

| Script | What it does |
|---|---|
| [`regen-index.py`](regen-index.py) | regenerates `_docs/INDEX.md` and `_docs/TOC.md` from the files git would publish, and link-checks the tree |

## Fixtures

| Script | What it does |
|---|---|
| [`make-head-fixture.py`](make-head-fixture.py), [`make-decide-fixture.py`](make-decide-fixture.py), [`make-tokenizer-fixture.py`](make-tokenizer-fixture.py) | regenerate the differential fixtures the offline tests compare against. They need the reference implementation and the checkpoint, so they are run by hand, not by CI |

## Running the stack

| Script | What it does |
|---|---|
| [`laya-sidecar.py`](laya-sidecar.py) | the persistent decision-head process the MCP server and the gate hook still use |
| [`setup-runtime.sh`](setup-runtime.sh) | creates the sidecar's pinned Python environment |
| [`install-host-service2.sh`](install-host-service2.sh) | installs the sidecar as a supervised systemd user service |

## Dispatching work

| Script | What it does |
|---|---|
| [`run-spec-part.sh`](run-spec-part.sh) | dispatches one spec part to an agent, from a script and never inline |
| [`verify-spec-part.sh`](verify-spec-part.sh) | verifies a dispatched part independently, one PASS/FAIL row per criterion |

## `live/`

Checks that need the running stack (a backbone server and the head), reported as UNMEASURED rather
than passed when it is down: [`gate-live-corpus.py`](live/gate-live-corpus.py),
[`live-corpus-parity.py`](live/live-corpus-parity.py), [`check-observability.py`](live/check-observability.py),
[`check-ten-levels.py`](live/check-ten-levels.py), [`check-wc-boundary.py`](live/check-wc-boundary.py).

## `archive/`

One-shot scripts from the build-out, kept for history and not maintained. See
[`archive/README.md`](archive/README.md).
