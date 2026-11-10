# laya-rs — table of contents

Flat list of every document. See [`INDEX.md`](INDEX.md) for structure and the
reading order.

## 01-overview
- [What laya-rs is](01-overview/README.md)

## 02-guides
- [Graph example — the guessing game, and the boundary it is built to state](02-guides/GRAPH-EXAMPLE-GUESSING.md)
- [Graph example — the `wc` word count, and what it exposed](02-guides/GRAPH-EXAMPLE-WORDCOUNT.md)
- [Installing Laya in Hermes](02-guides/INSTALL-IN-HERMES.md)
- [Install and first answer](02-guides/INSTALL.md)
- [Running Laya + llama.cpp](02-guides/RUNNING-LAYA.md)
- [USING LAYA DAILY — the operator's guide](02-guides/USING-LAYA-DAILY.md)
- [Guides](02-guides/README.md)

## 03-reference
- [The examples need laya-mcp on PATH — measured, and it is NOT intermittent](03-reference/EXAMPLES-NEED-LAYA-MCP-ON-PATH.md)
- [GRAPH-DETERMINISM — comparison is a `check`, judgement is a `decide`](03-reference/GRAPH-DETERMINISM.md)
- [Graph features — composability, branches, loops: what is implemented](03-reference/GRAPH-FEATURES.md)
- [The flow format contract — what changes, why, and what it costs](03-reference/GRAPH-FORMAT-CONTRACT.md)
- [The graph seam — laya-rs drives a DAG's `route` node](03-reference/GRAPH-SEAM.md)
- [Suggested graph specification, and how a flow becomes a graph](03-reference/GRAPH-SPEC.md)
- [Reference](03-reference/README.md)

## 04-development
- [laya-rs — architectural review (2026-10-01)](04-development/ANALYSIS-2026-10-02.md)
- [Code review — laya-rs, 2026-09-30](04-development/CODE-REVIEW-2026-09-30.md)
- [Design amendments — what the adversarial review changed](04-development/DESIGN-AMENDMENTS-2026-09-29.md)
- [Engine defects — three confirmed findings in the graph executor](04-development/ENGINE-DEFECTS-2026-10-02.md)
- [Analysis — why a generic graph cannot run today](04-development/GENERIC-GRAPHS-ANALYSIS.md)
- [The head's question-shape limits — measured](04-development/HEAD-QUESTION-SHAPE-LIMITS.md)
- [Opus review — running generic graphs through laya](04-development/OPUS-REVIEW-GENERIC-GRAPHS.md)
- [OPUS ROUND 2 — the build order, and what I had wrong](04-development/OPUS-ROUND2-BUILD-ORDER.md)
- [P1 status — and the measurements that shaped it](04-development/P1-STATUS.md)
- [The scheduler's single guard, and the mutants that hang it](04-development/SCHEDULER-SINGLE-GUARD.md)
- [Stale refusals — a rule that outlives its reason is a defect too](04-development/STALE-REFUSALS-2026-10-02.md)
- [The ten levels, mapped to our tools](04-development/TEN-LEVELS-MAPPING.md)
- [Development](04-development/README.md)

## 05-operations
- [The decision ledger](05-operations/DECISION-LEDGER.md)
- [Observability — what can be seen, and what could not](05-operations/OBSERVABILITY.md)
- [Readiness and the priority queue](05-operations/READINESS-AND-QUEUE.md)
- [THE SPEC GATE — is the spec set finished, and what does that cost](05-operations/SPEC-GATE.md)
- [Supervising the decision head](05-operations/SUPERVISION.md)
- [A flake in the test harness — ETXTBSY on the fake MCP server](05-operations/TEST-HARNESS-FLAKE.md)
- [Token economy — what it saves, what it costs, how it was measured](05-operations/TOKEN-ECONOMY.md)
- [Tool coverage — what has actually been exercised](05-operations/TOOL-COVERAGE.md)
- [Operations](05-operations/README.md)

## 06-research
- [Design (v1) — superseded in part](06-research/01-design.md)
- [Adversarial review by pi + glm-5.3](06-research/02-analysis-glm-5.3.md)
- [The review brief](06-research/02a-analysis-brief.md)
- [Four architecture questions (threading, config, graph spec, scheduler)](06-research/03-four-architecture-questions.md)
- [LAYA SEAM REVIEW + FIX PLAN — 2026-10-06](06-research/05-laya-seam-review-2026-10-06.md)
- [Research](06-research/README.md)
