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
- [Reference](03-reference/README.md) — Technical reference: the contract and the wire shapes.

### 04-development
- [Design amendments — what the adversarial review changed](04-development/DESIGN-AMENDMENTS-2026-09-29.md) — MANDATORY corrections to the design, with the five blockers and their resolutions.
- [P1 status — and the measurements that shaped it](04-development/P1-STATUS.md) — What was measured before designing (Laya concurrency, the redb collision), and what P1 delivered.
- [The ten levels, mapped to our tools](04-development/TEN-LEVELS-MAPPING.md) — IndyDevDan's ten levels, and the documented failures we inherit as requirements.
- [Development](04-development/README.md) — Design decisions and implementation notes.

### 05-operations
- [Readiness and the priority queue](05-operations/READINESS-AND-QUEUE.md) — How 'is Laya running?' is answered, and where a priority queue earns its place.
- [The decision ledger](05-operations/DECISION-LEDGER.md) — The redb ledger: schema, single-writer model, budgets, querying.
- [Operations](05-operations/README.md) — Running the server, the ledger, budgets.

### 06-research
- [Design (v1) — superseded in part](06-research/01-design.md) — The original design. Read DESIGN-AMENDMENTS first: the review corrected five blockers in it.
- [Adversarial review by pi + glm-5.3](06-research/02-analysis-glm-5.3.md) — The full review: five blockers, ten majors, fact-by-fact corrections.
- [The review brief](06-research/02a-analysis-brief.md) — The exact prompt the reviewer ran against, so the review is reproducible.
- [Research](06-research/README.md) — Numbered research write-ups: convention and index.

### 99-archive
- [Archive](99-archive/README.md) — Superseded material, kept with its consequence labelled.

## Related trees (linked, never copied)

- **pii** owns the graph engine. Its `route` contract, expr language and static
  checks live in `/projects/pi-improved/` source and `_docs/`. We **link**: a
  copied contract rots, a linked one stays true.
- **Workspace-level** docs (the open register, workspace rules) live in
  `/projects/docs/`. Our open items are registered there so they survive this
  project's sessions.
- **amem** holds the durable research trail (topics `laya-rs`, `pii`,
  `indydevdan-agentic-engineering`).

## Specs

Specs are not docs — they are the implementation contract, and they live in
`specs/` with TDD tables and runnable exit criteria. See [`../specs/`](../specs/).
