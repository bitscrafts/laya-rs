# laya-rs — table of contents

Flat list of every document. See [`INDEX.md`](INDEX.md) for structure and the
reading order.

## 01-overview
- [What laya-rs is](01-overview/README.md)
- [](01-overview/README.md#the-three-properties)

## 02-guides
- [Guides](02-guides/README.md)
- [Graph example — the `wc` word count, and what it exposed](02-guides/GRAPH-EXAMPLE-WORDCOUNT.md)
- [wc-classify-measured.flow.json — a word count that drives the branch](02-guides/wc-classify-measured.flow.json)
- [Graph example — the guessing game, and the boundary it is built to state](02-guides/GRAPH-EXAMPLE-GUESSING.md)
- [guess-the-number.flow.json — one round of the guessing game as a graph](02-guides/guess-the-number.flow.json)
- [guess-the-number.control.flow.json — the pair's control](02-guides/guess-the-number.control.flow.json)

## 03-reference
- [Suggested graph specification, and how a flow becomes a graph](03-reference/GRAPH-SPEC.md)
- [The flow format contract — what changes, why, and what it costs](03-reference/GRAPH-FORMAT-CONTRACT.md)
- [The examples need laya-mcp on PATH — measured, and it is NOT intermittent](03-reference/EXAMPLES-NEED-LAYA-MCP-ON-PATH.md)
- [Graph features — composability, branches, loops: what is implemented](03-reference/GRAPH-FEATURES.md)
- [Reference](03-reference/README.md)

## 04-development
- [Design amendments — what the adversarial review changed](04-development/DESIGN-AMENDMENTS-2026-09-29.md)
- [P1 status — and the measurements that shaped it](04-development/P1-STATUS.md)
- [The ten levels, mapped to our tools](04-development/TEN-LEVELS-MAPPING.md)
- [Development](04-development/README.md)
- [Opus review — running generic graphs through laya](04-development/OPUS-REVIEW-GENERIC-GRAPHS.md)
- [Analysis — why a generic graph cannot run today](04-development/GENERIC-GRAPHS-ANALYSIS.md)
- [Engine defects — three confirmed findings in the graph executor](04-development/ENGINE-DEFECTS-2026-10-02.md)
- [The scheduler's single guard, and the mutants that hang it](04-development/SCHEDULER-SINGLE-GUARD.md)
- [Stale refusals — a rule that outlives its reason is a defect too](04-development/STALE-REFUSALS-2026-10-02.md)
- [The head's question-shape limits — measured](04-development/HEAD-QUESTION-SHAPE-LIMITS.md)

## 05-operations
- [Readiness and the priority queue](05-operations/READINESS-AND-QUEUE.md)
- [The decision ledger](05-operations/DECISION-LEDGER.md)
- [Observability — what can be seen, and what could not](05-operations/OBSERVABILITY.md)
- [Operations](05-operations/README.md)
- [Tool coverage — what has actually been exercised](05-operations/TOOL-COVERAGE.md)
- [A flake in the test harness — ETXTBSY on the fake MCP server](05-operations/TEST-HARNESS-FLAKE.md)
- [Resuming the opus analysis session](05-operations/RESUMING-OPUS.md)
- [Token economy — what it saves, what it costs, how it was measured](05-operations/TOKEN-ECONOMY.md)
- [Resume prompt — 2026-10-02 (read this first after the restart)](05-operations/RESUME-PROMPT-2026-10-02.md)
- [Restart state — 2026-10-02](05-operations/RESTART-2026-10-02.md)

## 06-research
- [Design (v1) — superseded in part](06-research/01-design.md)
- [Adversarial review by pi + glm-5.3](06-research/02-analysis-glm-5.3.md)
- [The review brief](06-research/02a-analysis-brief.md)
- [Research](06-research/README.md)
- [Four architecture questions (threading, config, graph spec, scheduler)](06-research/03-four-architecture-questions.md)

## 99-archive
- [Archive](99-archive/README.md)
