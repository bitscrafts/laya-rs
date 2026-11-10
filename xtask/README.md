# xtask

Recurring workspace tasks, so that a person and a CI job run **the same thing**.

```bash
cargo xtask gate
```

runs all three steps, in this order, and exits non-zero if any failed. It does **not** stop at the first
failure, so one run reports every problem:

1. `cargo fmt --all --check`: zero diffs
2. `cargo clippy --workspace --all-targets -- -D warnings`: zero warnings
3. `cargo test --workspace`: all tests, offline, zero model calls

The toolchain is pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), so the bar does not move
when a machine's toolchain does. A gate that only reports is not a gate: this one returns a non-zero
exit code.

For the whole picture (gate, doc citations, commit-message scan, the specs' own exit criteria and
the live checks) use `bash scripts/verify-all.sh`.

## Test

```bash
cargo test -p xtask
```
