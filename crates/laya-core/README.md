# laya-core

The pure decision layer: the logic that must be reviewable and testable **without a model**.

## Modules

| module | what it owns |
|---|---|
| `policy` | thresholds, confidence bands, composite weights. Every number a decision depends on lives here, in the caller, never in a prompt |
| `route_payload` | the shape a graph's `route` node consumes, and the two fields it matches on |
| `readiness` | whether the model server is usable, and what every tool must do when it is not (never `allow`) |
| `queue` | a bounded priority queue so a blocking `gate` call is not stuck behind bulk work |
| `agentic` | level 10: the **caller writes the questions** (`ask`); it composes a state from a summary, files and a command and validates the questions before anything runs |
| `ask_file`, `ask_files` | scoped file questions: path scope checks and the prefix-length warning |
| `compact` | the `should_compact` decision |

## Rules

- **No network, no MCP, no HTTP.** Adding any is a defect: reviewable logic must not depend on a
  server being up. The only crate that talks to the network is `laya-backbone`.
- A band is computed here, never by a graph transform (a `route` expression cannot compute one).

## Depends on

`laya-abi`, `glob`, `serde`, `serde_json`, `thiserror`, `toml`.

## Test

```bash
cargo test -p laya-core
```

185 tests, all offline.
