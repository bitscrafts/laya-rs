# A FLAKE IN THE TEST HARNESS — ETXTBSY on the fake MCP server

**Found 2026-10-02** while verifying spec 20. Not introduced by that change; recorded here so it is
not misattributed later and so the next person does not spend a session chasing it.

## The symptom

```
thread 'mcp_client::tests::the_run_and_node_id_reach_the_server_as_tool_arguments' panicked at
crates/laya-cli/src/mcp_client.rs:410:9:
assertion `left == right` failed
  left: Err("failed to spawn `/tmp/laya-mcp-client-ctx-2902127/fake-laya-mcp`
             (set LAYA_MCP_BIN if it is not on PATH): Text file busy (os error 26)")
  right: Ok("ok")
```

The product is fine. **The harness is racing itself.** `ETXTBSY` on `exec` means the file being
executed is still open for writing by another thread in this process.

## The cause

`fake_server` (test helper, `crates/laya-cli/src/mcp_client.rs:325`) builds its scratch path from the
**process id plus a tag**:

```rust
let dir = std::env::temp_dir().join(format!("laya-mcp-client-{tag}-{}", std::process::id()));
let bin = dir.join("fake-laya-mcp");
std::fs::write(&bin, script).expect("write script");
std::fs::set_permissions(&bin, Permissions::from_mode(0o755)).expect("chmod");
```

**Every test in the binary shares one process id.** Two tests that use the **same tag** therefore
compute the **same path**; `libtest` runs them on parallel threads; one writes while the other
`exec`s that exact path. The kernel refuses with `ETXTBSY` (`os error 26`).

The tag is the collision key, so the tags in use are the thing to check. A tag that appears on more
than one test is a latent failure, and the more tests are added to this binary the more likely the
collision becomes.

## Measured frequency — and why it is hard to attribute

| observation | count |
|---|---|
| `verify-all.sh` full run, under build load | **1 failure in 1 run** (the ETXTBSY above) |
| `cargo test -p laya-cli --lib`, direct, quiet | 0 failures in 12 runs (6 with spec 20, 6 without) |
| the test alone | 0 failures in 8 runs |

So it is **rare and load-dependent** — which is exactly the kind of defect that gets attributed to
whatever landed most recently. **It is not spec 20's**: that change is **+61/−0 lines** in
`mcp_client.rs`, the failing test's name appears **0 times** in its diff, and the racing helper's body
is **unchanged**.

## What the right fix is (not done here — it is outside spec 20's scope)

Make the scratch path unique **per test**, not per process:

- include the **tag AND a per-call counter** (or the test's thread id) in the directory name, or
- have each test pass its own tag and assert the tags are distinct, or
- write to a unique temp dir per `fake_server` call (`tempfile::tempdir()`-style), so two tests can
  never collide regardless of tag.

The first is the smallest and removes the class: **the path must not be a function of the process id
alone when tests run in parallel.**

## The lesson for this project's checks

The suite is **not deterministic**, and a flake costs a session to diagnose if it appears at the wrong
moment. Two consequences:

1. **Never accept a green suite as proof without naming what ran.** A run that fails 1-in-20 is a run
   that will fail on the commit someone else is watching.
2. **When a check fails once and passes on retry, that is a finding, not noise.** Here the retry
   passed 8/8 — the instinct to call it "flaky, moving on" would have buried a real race in the test
   harness, which is the same class of defect this project keeps finding in the engine.
