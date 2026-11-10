# laya-mcp

The **MCP stdio server**: one Rust binary that exposes the decision layer to any MCP host, with no
network listener. Binaries:

- `laya-mcp`: the server.
- `laya-gate`: one command judged, one verdict printed, one ledger row written, for a shell hook that
  cannot speak MCP. It calls the same function as the MCP `gate` tool, so a hook call and an MCP call
  are one decision. The **exit code carries the verdict**: `0` allow, `1` ask, `2` deny, `3`
  unjudgeable (no judgement was possible; only the caller knows what an unjudged action should cost).
  `ask` is not an error: it is the gate doing its job.

## Tools

`decide` · `gate` · `ask_file` · `ask_files` · `should_compact` · `ask` · `ledger`

`route` is deliberately **not** a tool: exposing it would let an agent select edges outside any
graph.

## Behaviour that matters

- **Readiness comes first.** The server probes the model server before serving and, if it is not
  usable, writes the reason to stderr and exits non-zero. It never serves in a degraded state.
- **Fail closed.** If the decision source is unavailable, `gate` answers `ask`, never `allow`.
- **The server never opens the ledger store.** It appends a segment file; see
  [`laya-store`](../laya-store/README.md).

## Configuration

| variable | meaning |
|---|---|
| `LAYA_URL` | the backbone server (default is a placeholder for a private host) |
| `LAYA_LEDGER_DIR` | where to append ledger segments; unset means no ledgering |
| `LAYA_SIDECAR_SOCKET` | the decision-head socket (default `/run/laya/sidecar.sock`) |

## Rules

- Must **never depend on `laya-graph`**: the judge does not depend on the executor.

## Depends on

`laya-store`, `laya-abi`, `laya-core`, `laya-backbone`, `rmcp`, `tokio`, `ureq`, `serde`, `serde_json`.

## Test

```bash
cargo test -p laya-mcp        # 87 tests; the real server is spawned over real stdio
```
