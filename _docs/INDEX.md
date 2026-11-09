# laya-rs — documentation index

Project-local documentation, organised with the SSOT methodology. Each fact lives
in exactly one place; this index points at it, never restates it.

**Project root**: `/projects/laya-rs`

> **Read in this order on a cold start:** `01-overview` → `04-development/DESIGN-AMENDMENTS-2026-09-29.md` (the corrections) → `06-research/01-design.md`
> (the original). The amendments WIN wherever they conflict with the design.

## Sections

| section | holds |
|---|---|
| `01-overview/` | what this project is and why it exists |
| `02-guides/` | how to run, gate and use it |
| `03-reference/` | technical reference: the contract, the wire shapes |
| `04-development/` | design decisions, the review amendments, implementation notes |
| `05-operations/` | the ledger, budgets, running the server |
| `06-research/` | numbered research write-ups (design v1, the review, prior-art trails) |

## Documents

### 01-overview
- [What laya-rs is](01-overview/README.md) — The idea in four sentences, the three safety properties, and what is implemented today.

### 02-guides
- [Graph example — the guessing game, and the boundary it is built to state](02-guides/GRAPH-EXAMPLE-GUESSING.md) — The second worked example: a shell produces a round's secret and guess, the decide's prompt carries the {{setup}} placeholder so the head judges the actual round, and the graph routes to the advice. One round per run -- the no-loop boundary documented and measured (the cycle refusal is run in the document), with a control pair and a break-the-port drill.
- [Graph example — the `wc` word count, and what it exposed](02-guides/GRAPH-EXAMPLE-WORDCOUNT.md) — A worked graph using `wc`, its control, and the fix: the head answered MEDIUM across a 77x range with confidence 0.09-0.26 (band=low) every time, and only the flow that routed on `band` escalated instead of acting on a label the head had disowned.
- [Installing Laya in Hermes](02-guides/INSTALL-IN-HERMES.md) — How to register the laya MCP server so Hermes (and any other MCP client) can call the decision layer as tools.
- [Running Laya + llama.cpp](02-guides/RUNNING-LAYA.md) — How to stand up the whole decision layer on lnx, from nothing to a verified answer.
- [USING LAYA DAILY — the operator's guide](02-guides/USING-LAYA-DAILY.md) — This document answers one question: what do I run, every day, and what should I expect?
- [Guides](02-guides/README.md) — How to build, gate and call laya-rs.

### 03-reference
- [The examples need laya-mcp on PATH — measured, and it is NOT intermittent](03-reference/EXAMPLES-NEED-LAYA-MCP-ON-PATH.md) — Every worked example runs a decision node, and the command spawns the server through the caller's PATH, so the examples fail from a shell without the install directory. Measured three ways. First mistaken for a flake: a nondeterministic cause and a differing environment look identical from a single result.
- [GRAPH-DETERMINISM — comparison is a `check`, judgement is a `decide`](03-reference/GRAPH-DETERMINISM.md) — Rule: comparison → check; judgement → decide. A decide must never be used where a check can answer.
- [Graph features — composability, branches, loops: what is implemented](03-reference/GRAPH-FEATURES.md) — Measured by running, not reading. Branches: yes, working. Composability: implemented in the library with tests, UNREACHABLE from the CLI (no library flag, and the error message blames the wrong field). Loops: no, refused by design with the cycle path named.
- [The flow format contract — what changes, why, and what it costs](03-reference/GRAPH-FORMAT-CONTRACT.md) — The twelve changes that make every declared field either work, be refused by name, or be documented as carrying no behaviour. With the measured 'before' for each: a typo of any key is silently inert, four silent misroutes pass a validate, and the reference doc advertises $.inputs, a feature that does not exist.
- [The graph seam — laya-rs drives a DAG's `route` node](03-reference/GRAPH-SEAM.md) — Status: the contract is IMPLEMENTED and tested; the end-to-end run is not built yet (see "What is missing" at the end — stated, not implied).
- [Suggested graph specification, and how a flow becomes a graph](03-reference/GRAPH-SPEC.md) — A SUGGESTED (not decided) format for flows — nodes, kinds, composition — and the four-stage translation to a graph, with the invariants, the renderer's round-trip test, and a list of the weakest points to attack first.
- [Reference](03-reference/README.md) — Technical reference: the contract and the wire shapes.

### 04-development
- [laya-rs — architectural review (2026-10-01)](04-development/ANALYSIS-2026-10-02.md) — Reviewed tree: b0ee704 on master, clean at the start of the review. Read-only throughout: nothing in /projects/laya-rs was written. A separate session modified
- [Code review — laya-rs, 2026-09-30](04-development/CODE-REVIEW-2026-09-30.md) — Method: deterministic audit (codebase-audit 2.0.0) + hand-verification of every PROVEN finding + targeted reads of the fail-closed paths. No claim below is from the
- [Design amendments — what the adversarial review changed](04-development/DESIGN-AMENDMENTS-2026-09-29.md) — MANDATORY corrections to the design, with the five blockers and their resolutions.
- [Engine defects — three confirmed findings in the graph executor](04-development/ENGINE-DEFECTS-2026-10-02.md) — depends_on does not order a decide before its shell (measured: the shell ran LAST); escalate_on_near_tie is validated but inert; and a decide's question type is hardcoded to choice, so a flow can never ask the noul the head answers best.
- [Analysis — why a generic graph cannot run today](04-development/GENERIC-GRAPHS-ANALYSIS.md) — The engine has no data flow at all: every decide input is a literal compiled into the flow file, and all four mechanisms that could carry run-time data are absent. The two-phase execution model, what a flow can actually express, and a ranked set of fixes.
- [The head's question-shape limits — measured](04-development/HEAD-QUESTION-SHAPE-LIMITS.md) — A 3-option choice does not track a number while a noul threshold does (15/15, incl. the boundary); and a bare noul on a subjective question returns a confident CONSTANT (no at 1.0 for both positive and negative text). Why the correct handling is bounding one signal.
- [Opus review — running generic graphs through laya](04-development/OPUS-REVIEW-GENERIC-GRAPHS.md) — An independent review that re-ran every claim with laya graph run. Found that routing is not a function of the graph (node ARRAY ORDER decides execution), that the near-tie target runs unconditionally, and that 'no data flow at all' was wrong: data CAN reach the head, it cannot reach an edge. Five contradictions, three re-verified here.
- [OPUS ROUND 2 — the build order, and what I had wrong](04-development/OPUS-ROUND2-BUILD-ORDER.md) — claude + opus, 2026-10-02, resumed session d58b577f-fc19-48d5-b690-f6d7524ea4f8 (15 turns, $4.27, 620 s). Full report: /projects/_tmp/opus-graph-analysis/ROUND2.md (346 lines), with every
- [P1 status — and the measurements that shaped it](04-development/P1-STATUS.md) — What was measured before designing (Laya concurrency, the redb collision), and what P1 delivered.
- [The scheduler's single guard, and the mutants that hang it](04-development/SCHEDULER-SINGLE-GUARD.md) — dependency_tables is the shared source of the graph's shape for topo_order, refuse_cycle and the Tracker, and nothing checks it is about the graph at all. refuse_cycle is the only cycle guard and is built on the same table. There is no spin bound anywhere, so a node that never settles is waited on forever -- the two mutants the gate reports as timeouts.
- [Stale refusals — a rule that outlives its reason is a defect too](04-development/STALE-REFUSALS-2026-10-02.md) — Spec 20 refused timeout and allow_failure on a decide 'until spec 17'. Spec 17 landed and the timeout now works: a decide with timeout zero fails as timed out. The refusal was stale and blocked a working feature. The other half is worse than refused -- allow_failure is accepted, looks honoured, then the synthetic route node stops the run anyway.
- [The ten levels, mapped to our tools](04-development/TEN-LEVELS-MAPPING.md) — IndyDevDan's ten levels, and the documented failures we inherit as requirements.
- [Development](04-development/README.md) — Design decisions and implementation notes.

### 05-operations
- [The decision ledger](05-operations/DECISION-LEDGER.md) — The redb ledger: schema, single-writer model, budgets, querying.
- [Observability — what can be seen, and what could not](05-operations/OBSERVABILITY.md) — Measured state of observability in the deployment: the ledger that was silently off, the operating configuration, the measured cost per call, and the questions still unanswerable.
- [Readiness and the priority queue](05-operations/READINESS-AND-QUEUE.md) — How 'is Laya running?' is answered, and where a priority queue earns its place.
- [THE SPEC GATE — is the spec set finished, and what does that cost](05-operations/SPEC-GATE.md) — Measured 2026-10-02, on this project, with the installed laya. Every number below came from a run, not from reading code.
- [Supervising the decision head](05-operations/SUPERVISION.md) — How the Laya sidecar stays up across crashes and reboots, and why it is supervised on the host rather than inside this container.
- [A flake in the test harness — ETXTBSY on the fake MCP server](05-operations/TEST-HARNESS-FLAKE.md) — The suite is not deterministic: fake_server names its scratch file from the process id plus a tag, every test shares one pid, so two tests with the same tag write and exec the same path and the kernel refuses with Text file busy. Measured 1 failure in ~20 binary runs, load-dependent, and proven not to come from the change that was being verified.
- [Token economy — what it saves, what it costs, how it was measured](05-operations/TOKEN-ECONOMY.md) — The measured answer: 101,840 tokens saved per triage round (43x), 0 tokens of context cost on allow/ask, and 178 tokens ever on two blocks. Includes the 263x-overstated first measurement and why a phrase match is not attribution.
- [Tool coverage — what has actually been exercised](05-operations/TOOL-COVERAGE.md) — The 7 advertised tools against what the ledger shows has RUN: three have effectively never been called. Includes the SDD+TDD gate graph, the paired control, and the break-the-control check that proves the pair means something.
- [Operations](05-operations/README.md) — Running the server, the ledger, budgets.

### 06-research
- [Design (v1) — superseded in part](06-research/01-design.md) — The original design. Read DESIGN-AMENDMENTS first: the review corrected five blockers in it.
- [Adversarial review by pi + glm-5.3](06-research/02-analysis-glm-5.3.md) — The full review: five blockers, ten majors, fact-by-fact corrections.
- [The review brief](06-research/02a-analysis-brief.md) — The exact prompt the reviewer ran against, so the review is reproducible.
- [Four architecture questions (threading, config, graph spec, scheduler)](06-research/03-four-architecture-questions.md) — Discussion only: the priority queue as admission control and a missed fourth parallelism axis; where config should live and why redb does not speed the load; our own graph specification and mermaid export; and whether to build a scheduler.
- [04 — JEV / TypeSafe AI ("System One") — the upstream video, reviewed for ideas](06-research/04-jev-typesafe-ai-video-review.md) — Source: "JEV / TypeSafe AI API & Rust - Let make a rust lb" — Jeremy Chone (@jchone), live 2026-09-19, 2h44m (9,863 s), https://www.youtube.com/live/pU73lYF7R1I.
- [LAYA SEAM REVIEW + FIX PLAN — 2026-10-06](06-research/05-laya-seam-review-2026-10-06.md) — <!-- Filed 2026-10-06T14:30Z by the Hermes orchestrator (Marcelo Correa). Produced by: pi + glm-5.3:cloud (ollama-cloud), the two-lane ANALYSIS lane (read-only tool grant),
- [Research](06-research/README.md) — Numbered research write-ups: convention and index.

## This project is self-contained

Every fact in this tree stands on its own: what we rely on from outside is stated
as a property of an interface, never as a path or line number into another
repository. The behaviour we depend on is enforced by tests in `crates/`, so a
change anywhere else cannot move this project silently.

**Standalone by design** — one project, one repository, no external association.

## Specs

Specs are not docs — they are the implementation contract, and they live in
`specs/` with TDD tables and runnable exit criteria. See [`../specs/`](../specs/).
