# P1 — status and what was measured first

**Date**: 2026-09-29 · **State**: P1's **pure core and readiness path are DONE and
green**; the MCP server itself (spec 04) is next.

## Verified (run, not asserted)

```
cargo xtask gate          -> PASS (fmt + clippy -D warnings + test)
tests, whole workspace    -> 103 passing, 0 failing
  laya-abi 8 · laya-core 62 · laya-backbone 19 · laya-head 5 · laya-cli 5 · xtask 4
clippy --workspace        -> 0 warnings
cargo fmt --all --check   -> clean
doc links                 -> 41 checked, 0 broken
```

## The two measurements that shaped P1

Everything below was measured **before** designing, because the answer changed the
design:

```
Laya /embedding          : 35 ms median (sequential), 103 KB of hidden states
8 concurrent /embedding  : 121 ms wall        => 5.8x parallel, NOT serialised
redb, 2nd process        : Error: DatabaseAlreadyOpen   => blocker B4 CONFIRMED
```

**Consequence 1 — the backbone is not the bottleneck.** 5.8× parallel headroom at
8 concurrent means a queue in front of it buys no throughput. Building one "to
scale" would have added a scheduling layer for nothing.

**Consequence 2 — the redb collision is real, and it is a clean typed error.** Not
corruption: `DatabaseAlreadyOpen`. So spec 04's guardrail ("the server never opens
the store for writing") is enforceable, and the single-writer design is the right
answer.

**Consequence 3 — the capability string is misleading.** The served model reports
`capabilities: ["completion"]`, **not** `"embedding"`, because the GGUF holds the
backbone only (which is also why the head must run outside llama.cpp). A readiness
check that insisted on an `"embedding"` capability would report a perfectly working
server as broken. We check the **model id** instead, and a test pins that.

## Your priority-queue question, answered with the measurement

> *"on redb collisions, could be used a priority queue?"*

**Yes — but the measurement moves where it earns its place.**

| job | needed for | priority's value |
|---|---|---|
| **Work scheduling** in front of the decision path | fairness: a blocking `gate` must not wait behind a bulk file scan | **high** — user-visible latency |
| **Ledger write serialisation** | solving B4 (one writer) | **low** — records are records |

So the queue exists to give the **gate lane precedence** and to **bound** the
system, not to raise throughput. Spec 06 implements it
(`laya-core::queue`, 16 tests) with three classes, FIFO within a class, and a
fail-closed shedding rule: **a `Gate` is never shed**, and if one must be refused
the answer is `ask`, never `allow`.

## What P1 delivered

**Implemented and green**
- `laya-core::queue` — the bounded priority queue (spec 06)
- `laya-core::readiness` — the readiness judgement (spec 05)
- `laya-backbone` — the probe **parsers** (`parse_models`, `from_health_response`,
  `from_transport_error`), all pure, all tested against the **live** response bodies
- `laya-cli`'s `laya doctor` — the end-to-end readiness command

**`laya doctor`, run for real:**

```
$ laya doctor
laya-rs: READY
  endpoint  : http://10.10.10.217:8083
  /health   : ok
  /v1/models: ok (serving the expected model)
exit=0

$ laya doctor --url http://127.0.0.1:9
laya-rs: Laya is not reachable/usable at http://127.0.0.1:9
  reason: connection failed: io: Connection refused (os error 111)
  check : curl http://127.0.0.1:9/health
  fix   : start the container `llamacpp-laya` (podman start llamacpp-laya)
  refusing to start: a decision layer that cannot decide must not run,
  and a gate that cannot ask must never answer `allow`.
exit=1
```

**Two defects found by RUNNING it, not by reading it:**
1. A **404 on a live server** was reported as *"connection failed"* — which would
   send a reader to restart a healthy container. Fixed: `ureq` reports HTTP errors
   as `Err`, so the status is now recovered and reported as `BadStatus { 404 }`.
   Three tests added, using the **exact string ureq produced**.
2. `cargo xtask gate` did not exist until `.cargo/config.toml` added the alias.

## Still to do in P1 (spec 04)

- the MCP stdio server itself, with the readiness precondition at startup
- the pinned Python sidecar for the head (blocker B3)
- the two-process redb test, now that B4 is confirmed
- `decide`, then `ask_file`/`ask_files`

## Files

```
crates/laya-core/src/{queue,readiness,policy,route_payload}.rs
crates/laya-backbone/src/lib.rs          <- the probe parsers
crates/laya-cli/src/main.rs              <- `laya doctor`
specs/{05-laya-must-be-running,06-priority-queue}.md
scripts/measure-and-prove.py                <- the concurrency + redb measurement
_tmp/redb-probe/                         <- the redb cross-process probe
```
