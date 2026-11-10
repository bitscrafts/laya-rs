# laya-cli

The terminal runner, binary **`laya`**.

```text
laya doctor [--url URL]                       readiness check; exit 0 ready, 1 not ready
laya decide --state STATE --question JSON     one decision, in-process, no Python
            [--url URL] [--head PATH] [--tokenizer PATH] [--timeout-ms N]
laya decide -                                 read {"state":…,"question":…} from stdin
laya ledger compact|stats|tail|outcome        inspect and maintain the decision ledger
laya graph validate|render|run <flow.json>    run a flow
```

`decide` loads the head and tokenizer, calls the backbone's `/embedding` over HTTP, and prints the
route-shaped payload. `--timeout-ms` bounds the backbone request (the default is two minutes);
zero or non-numeric values are refused. Without `--head` and `--tokenizer`, defaults point at paths
on the author's machine, so pass them explicitly; see the
[install guide](../../_docs/02-guides/RUNNING-LAYA.md).

## Modules

`decide`, `embedder` (the HTTP embedder), `graph`, `ledger`, `mcp_client`.

## Depends on

`laya-abi`, `laya-core`, `laya-backbone`, `laya-head`, `laya-graph`, `laya-store`, `tokio`, `ureq`,
`serde`, `serde_json`.

## Test

```bash
cargo test -p laya-cli        # 60 tests; hung, dead and slow backbones are tested on real sockets
```
