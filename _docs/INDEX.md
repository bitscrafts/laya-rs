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
| `99-archive/` | superseded material, kept with its consequence labelled |

## Documents

### 01-overview
- [](01-overview/README.md#the-three-properties) — 
- [What laya-rs is](01-overview/README.md) — The idea in four sentences, the three safety properties, and what is implemented today.

### 02-guides
- [Guides](02-guides/README.md) — How to build, gate and call laya-rs.

### 03-reference
- [Suggested graph specification, and how a flow becomes a graph](03-reference/GRAPH-SPEC.md) — A SUGGESTED (not decided) format for flows — nodes, kinds, composition — and the four-stage translation to a graph, with the invariants, the renderer's round-trip test, and a list of the weakest points to attack first.
- [Reference](03-reference/README.md) — Technical reference: the contract and the wire shapes.

### 04-development
- [Design amendments — what the adversarial review changed](04-development/DESIGN-AMENDMENTS-2026-09-29.md) — MANDATORY corrections to the design, with the five blockers and their resolutions.
- [P1 status — and the measurements that shaped it](04-development/P1-STATUS.md) — What was measured before designing (Laya concurrency, the redb collision), and what P1 delivered.
- [The ten levels, mapped to our tools](04-development/TEN-LEVELS-MAPPING.md) — IndyDevDan's ten levels, and the documented failures we inherit as requirements.
- [Development](04-development/README.md) — Design decisions and implementation notes.

### 05-operations
- [Readiness and the priority queue](05-operations/READINESS-AND-QUEUE.md) — How 'is Laya running?' is answered, and where a priority queue earns its place.
- [The decision ledger](05-operations/DECISION-LEDGER.md) — The redb ledger: schema, single-writer model, budgets, querying.
- [Observability — what can be seen, and what could not](05-operations/OBSERVABILITY.md) — Measured state of observability in the deployment: the ledger that was silently off, the operating configuration, the measured cost per call, and the questions still unanswerable.
- [Resume prompt — 2026-10-02 (read this first after the restart)](05-operations/RESUME-PROMPT-2026-10-02.md) — The conversation's state in one page: the one open item (whether the gate fires for a live session), the exact before/after check that answers it, what was finished, the token measurement, and the traps that cost time.
- [Restart state — 2026-10-02](05-operations/RESTART-2026-10-02.md) — What is deployed with hashes verified against the tree, the verification order, and the four things deliberately left unfinished.
- [Operations](05-operations/README.md) — Running the server, the ledger, budgets.

### 06-research
- [Design (v1) — superseded in part](06-research/01-design.md) — The original design. Read DESIGN-AMENDMENTS first: the review corrected five blockers in it.
- [Adversarial review by pi + glm-5.3](06-research/02-analysis-glm-5.3.md) — The full review: five blockers, ten majors, fact-by-fact corrections.
- [The review brief](06-research/02a-analysis-brief.md) — The exact prompt the reviewer ran against, so the review is reproducible.
- [Four architecture questions (threading, config, graph spec, scheduler)](06-research/03-four-architecture-questions.md) — Discussion only: the priority queue as admission control and a missed fourth parallelism axis; where config should live and why redb does not speed the load; our own graph specification and mermaid export; and whether to build a scheduler.
- [Research](06-research/README.md) — Numbered research write-ups: convention and index.

### 99-archive
- [Archive](99-archive/README.md) — Superseded material, kept with its consequence labelled.

## This project is self-contained

Every fact in this tree stands on its own: what we rely on from outside is stated
as a property of an interface, never as a path or line number into another
repository. The behaviour we depend on is enforced by tests in `crates/`, so a
change anywhere else cannot move this project silently.

**Standalone by design** — one project, one repository, no external association.

## Specs

Specs are not docs — they are the implementation contract, and they live in
`specs/` with TDD tables and runnable exit criteria. See [`../specs/`](../specs/).
