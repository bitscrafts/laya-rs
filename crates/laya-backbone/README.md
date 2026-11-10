# laya-backbone

The **only crate that talks to the network**. Two jobs:

1. **Readiness.** Probe the model server (`/health`, `/v1/models`) and return an observation for
   `laya-core::readiness` to judge. The judgement is pure and lives in `laya-core`; this crate only
   supplies the observation.
2. **The backbone call.** `POST /embedding` to a llama.cpp server, returning the hidden states.
   There is no FFI: llama.cpp serves the encoder and Rust speaks HTTP.

The `sidecar` module is the unix-socket client for the persistent decision-head process that the
MCP server and the gate hook still use (see the [workspace README](../../README.md#the-sidecar-and-what-it-costs)).

## Configuration

`DEFAULT_ENDPOINT` is a placeholder for a private host. Set `LAYA_URL` (read by the binaries) or pass
`--url`. `DEFAULT_SOCKET` is `/run/laya/sidecar.sock`.

## Depends on

`laya-core`, `serde_json`, `thiserror`. No other network library is added here without a reason.

## Test

```bash
cargo test -p laya-backbone   # 27 tests
```

Offline: the HTTP behaviour is tested against local sockets, not a real model server.
