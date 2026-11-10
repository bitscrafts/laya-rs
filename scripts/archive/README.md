# scripts/archive/

One-shot scripts from the build-out of this project: red/green steps that patched the source by
string anchor, measurements that fed a design decision, proofs of finished work, and dispatch
scripts for finished specs. Their effects are already in the crates and the specs. They are kept
for history and **are not maintained**: several hardcode paths from the machine that built them,
and none is run by CI or referenced by the gate, the docs or another script.

| Script | What it was for (its own first comment line) |
|---|---|
| `ar-metric-gate-latency.sh` | ar-metric-gate-latency.sh — autoresearch metric for spec 30: p50 laya-gate latency in ms. |
| `ask-file-wiring-green.py` | GREEN: implement the ask_file handler, and repoint the OLD scope tests. |
| `ask-file-wiring-green2.py` | GREEN, corrected anchor: insert `ask_file_via_sidecar` before `decide_via_sidecar` |
| `ask-file-wiring-green3.py` | Complete the ask_file wiring, tolerating an already-absent block. |
| `ask-file-wiring-red.py` | RED for the ask_file HANDLER, and remove the duplicated security check. |
| `ask-green.py` | GREEN for `ask` (L10): insert the composition + validation, above the test module.""" |
| `ask-red.py` | RED for `ask` (L10): append the contract tests. No implementation yet, so this must fail |
| `check-real-crates.sh` | Verify the REAL crates hold this project's bar: zero clippy warnings, fmt clean, tests pass. |
| `close-review.py` | Close the review: record the fixes, CORRECT the one wrong finding, update HANDOFF.""" |
| `compact-green.py` | GREEN: implement `compact`, prepended above the test module.""" |
| `deploy-binaries.sh` | Deploy the freshly built binaries. Keeps the old ones as .old-<stamp>; never cp over a live binary. |
| `fix-scope-resolution.py` | GREEN, part 2: resolve a path for the scope check WITHOUT requiring the file to exist. |
| `fix-scope-test-contradiction.py` | Fix a contradiction between TWO TESTS I WROTE, by stating the semantics. |
| `gate-signal-green.py` | GREEN: the second, independent signal for command safety — plus honest validation. |
| `install-host-service.sh` | Install `laya-sidecar` as a SUPERVISED service on the host `lnx`. |
| `measure-and-prove.py` | Measure Laya concurrency properly, then PROVE the redb cross-process behaviour. |
| `measure-concurrency.sh` | Measure the TWO contention points that decide P1's architecture. |
| `measure-content-limit.py` | MEASURE the real content limit of the head — do NOT assume 256 KiB is seen. |
| `measure-effect-choice.py` | THE DECISIVE TEST: this head CLASSIFIES, it does not reason about consequences. |
| `measure-effect-choice2.py` | THE DECISIVE TEST, with the CORRECT choice shape (`criteria`, per laya's own docstring). |
| `measure-gate-question.py` | Score the GATE's question, and find the real `choice` shape. |
| `measure-mid-edit-variants.py` | Find a `mid_edit` phrasing that actually works — or conclude the veto must go. |
| `measure-mid-edit.py` | Is the `mid_edit` question over-triggering? Measure before changing the design. |
| `measure-window.py` | Locate the exact content window, and test whether it is tokens or bytes. |
| `near-tie-green.py` | GREEN: implement the margin, the near-tie flag, and the band demotion. |
| `near-tie-red.py` | RED: the near-tie margin (spec 04 F3) and the inert noul threshold. |
| `poc-mcp-full.py` | THE POC TEST, corrected: full chain AND a genuinely fail-closed case. |
| `poc-sidecar.py` | POC test: drive the sidecar over its real socket and get a REAL decision. |
| `probe-question-shapes.py` | Find the correct choice/score question shape for the sidecar, and why is_irreversible |
| `prove-ask-file.py` | PROVE ask_file end to end through the REAL MCP surface — the live artefact. |
| `prove-ask-files.py` | PROVE ask_files end to end, through the REAL MCP surface. |
| `prove-ask.py` | PROVE ask (L10) end to end, through the REAL MCP surface. |
| `prove-hermes-path.sh` | PROVE the current binary + sidecar work end to end, on the DEFAULT socket, |
| `prove-ledger.py` | PROVE spec 04's ledger half end to end, through the REAL binaries. |
| `prove-margin.py` | PROVE margin + near_tie + the noul threshold, through the REAL MCP surface. |
| `prove-should-compact.py` | PROVE should_compact end to end, through the REAL MCP surface. |
| `refactor-ask-file.py` | REFACTOR: clear the clippy findings in the new module. Every one is a real fix. |
| `run-analysis.sh` | Dispatch pi + glm-5.3 (FULL, not flash) for the laya-rs adversarial design review. |
| `run-glm-analysis.sh` | run-glm-analysis.sh -- dispatch a READ-ONLY analysis job to a pi worker. |
| `run-opus-graph-analysis.sh` | run-opus-graph-analysis.sh -- dispatch the generic-graphs analysis to claude + opus. |
| `run-pi-worker.sh` | run-pi-worker.sh -- COPY of the maintained pi runner, with the three marked spots filled |
| `run-spec-pi.sh` | run-spec-pi.sh -- dispatch ONE spec-implementation job to a pi worker (pi + glm-5.3-flash:cloud). |
| `run-spec28-implement.sh` | run-spec28-implement.sh — dispatch the workhorse lane (pi-workhorse harness) for laya-rs spec 28. |
| `run-spec29-implement.sh` | run-spec29-implement.sh — dispatch the workhorse lane for laya-rs spec 29 (a policy can hard-deny). |
| `smoke-deployed.sh` | Smoke-test the DEPLOYED binaries: does the live path now have every fix? |
| `test-mcp-e2e.py` | End-to-end test of the laya-rs MCP server over REAL stdio, both directions. |
| `wire-margin-1.py` | Wire margin + the configurable noul threshold into the MCP surface. |
