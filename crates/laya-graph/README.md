# laya-graph

The generic **DAG engine**. Laya supplies a *value* (a `label`, a `band`); the graph owns every
edge. This crate parses a flow document, refuses what must be refused, lowers a `decide` node into
the `mcp` + `route` pair that actually runs, and executes the result.

## Node kinds

| kind | does | branches on |
|---|---|---|
| `shell` | runs a command | nothing |
| `decide` | asks the head a question (a model call) | `route` on the answer's `label` or `band` |
| `check` | runs a command for its **exit status** | `0` is `pass`, non-zero is `fail`; no model |
| `mcp` | calls one of the MCP tools | nothing |

**Comparison is a `check`; judgement is a `decide`.** A `check` must never be replaced by a `decide`:
the head is measurably wrong at numeric boundaries. See
[`GRAPH-DETERMINISM.md`](../../_docs/03-reference/GRAPH-DETERMINISM.md).

## Rules

- The `on` expression language is member access on a real payload field and nothing else, which is
  what makes the engine unable to invent an edge.
- A `route` has a **mandatory catch-all**.
- Independent nodes fan out up to a declared limit; `writes: true` nodes are mutually exclusive; a
  failure stops the run and reports `stopped_early` with the failing node.
- Depends on **neither `laya-store` nor `redb`**: run records are plain data in `RunReport`.

## Modules

`flow`, `on_expr`, `placeholder`, `expand`, `validate`, `lower`, `graph`, `sched`, `route_exec`,
`run`, `render`, `error`.

## Use

Through the CLI: `laya graph validate|render|run <flow.json>`. Examples are in
[`_docs/02-guides/`](../../_docs/02-guides/) and are executed by `tests/examples.rs`.

## Depends on

`laya-abi`, `laya-core`, `serde`, `serde_json`, `thiserror`, `blake3`, `tokio`.

## Test

```bash
cargo test -p laya-graph      # 174 tests listed; 5 live ones are #[ignore]
```
