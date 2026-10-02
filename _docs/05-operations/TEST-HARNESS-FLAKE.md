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

---

## SECOND INSTANCE (2026-10-02, during spec 21) — the same family, in `sched.rs`

```
cargo test --workspace: FAILED
  sched::tests::a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not
```

Timing-sensitive (a 30 s sleeper with a 0.5 s deadline, asserting that queued siblings were not
dispatched). It then passed 3/3 alone with the change in question, 3/3 with it stashed, and **0 failures
in 10 runs** when measured directly here.

**Two instances is a family, not two anecdotes.** Both observed failures share the same shape:

| test | what it asserts | why it can race |
|---|---|---|
| `a_failure_stops_queued_siblings_from_starting` | siblings `b`/`c`/`d` never ran after `a` failed | `a` is `sleep 0.2; exit 1`; whether the siblings are already dispatched depends on how the scheduler's first loop iteration interleaves with that 200 ms |
| `a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not` | same, against a 0.5 s deadline on a 30 s sleeper | same window, different trigger |

**Both assert a property that is about ORDERING under a wall-clock race, and both can only fail when
the machine is loaded enough that the sibling wins the race.** Neither has ever failed in a quiet run,
in either tree, which is why neither is reproducible on demand.

**The honest conclusion: the suite is not deterministic, this is a known family, and neither instance
is attributed to any change.** It has been measured at 0/20 before and after a change, 0/10 and 0/10
here — so a single observed failure is a real signal that the timing window exists, not evidence about
the change that happened to be in flight.

**If someone wants this fixed, the fix is not a longer sleep** — it is to assert the PROPERTY
deterministically (a scripted caller that blocks until released, so the ordering under test is
controlled rather than raced), the same technique spec 21 used to make its decide-timeout tests
meaningful: an *instant* scripted caller finished before a zero deadline could fire, so the test was
vacuous until a 200 ms `SlowCaller` was introduced. **A test whose control is a stopwatch is a test
that will fail on someone else's machine.**
