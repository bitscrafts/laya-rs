# laya-store

The embedded **decision ledger**, on [redb](https://crates.io/crates/redb). Every decision is
recorded so that cost and latency are visible rather than assumed, a routing decision can be joined
to the run that consumed it, and a threshold change can be tested afterwards against recorded
decisions.

It is **not** a place for state or secrets: the ledger stores `state_sha256`, never the state.

## The single-writer design

redb admits one writing process. A server therefore never opens the store: each `laya-mcp` appends
to its own segment file, `ledger-<pid>.jsonl`, in a directory (`LAYA_LEDGER_DIR`). One process,
`laya ledger compact`, merges segments into `ledger.redb` and **moves** them to `ingested/` rather
than deleting them, so compaction is idempotent and the raw evidence survives.

## Modules

The ledger and its queries (counts, latency percentiles, saturation rate), and `calibrate`
(calibration statistics over recorded outcomes).

## Depends on

`redb`, `serde`, `serde_json`. No other workspace crate.

## Test

```bash
cargo test -p laya-store      # 48 tests, offline, temporary directories only
```
