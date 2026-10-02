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

**Every test in the binary shares one process id**, so the path is unique only while every tag in the
file stays distinct -- a property maintained by hand in a file that grows.

> **CORRECTED 2026-10-02, a few hours after this was written.** The paragraph above originally said
> *"two tests that use the same tag therefore compute the same path"* and named the tag as **the**
> collision key. **That mechanism is not supported:** every `fake_server` call site in
> `crates/laya-cli/src/mcp_client.rs` uses a **distinct** tag (`hung`, `answers`, `ctx`, `ctx-decide`,
> `s16-default`, `s16-choice`, `s16-noul`, `s16-yolo`, `wire-{field}-{n}`), verified by listing them.
> So the class is real — **one** `ETXTBSY` in ~20 binary runs, never reproduced in 12 quiet runs or 8
> runs of the affected test alone — but **the mechanism was not established**, and a plausible story is
> not an explanation. The tag claim was a hypothesis that read like a finding, which is the failure this
> whole document is about.
>
> **What was done instead of guessing further:** the scratch path is now unique **per call** (an atomic
> counter appended), so two calls can never share a directory regardless of tag. That removes the
> possibility by construction rather than by the discipline of keeping tags distinct.

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

---

## A THIRD KIND OF DEFECT in this family: a check whose MECHANICS are wrong

Not a flake, and not a broken criterion — a **measurement** that cannot report the truth. Two instances
on 2026-10-02, both mine, both costing a wrong conclusion:

**1. The exit status of a piped command is the LAST command's.**
```sh
timeout 900 bash scripts/verify-all.sh 2>&1 | tail -25
echo "  verify-all exit=$?"        # <- TAIL's status, not the script's
```
This printed `exit=0` while the script was printing FAILs. **`verify-all.sh` exits 1 on failures and
always has** (its own code: `if [ "${#FAILS[@]}" -gt 0 ]; then exit 1; fi`). The check was fine; the
reading of it was not. Same family as:

**2. `if cmd | grep -q pattern` tests `cmd`'s exit status, not the match** — under `set -o pipefail`,
a refusal that fired correctly read as "no refusal".

**The rule: a check has two parts — the command and the reading of it — and either can be wrong.**
Before believing a negative or a zero, ask *what value am I actually reading?* In both cases the answer
was "the exit status of something other than the program under test".

### And a REAL defect the same output revealed

`scripts/verify-all.sh` prepended the cargo bin to `PATH` and **inherited the rest**, so every criterion
invoking the CLI failed with `laya: command not found` from any shell that lacked the install directory.
Measured from a bare environment: all of them fail. Fixed — it now sets the install directory itself and
**pins this tree's build** when one exists, so a criterion about this tree cannot silently test an
installed older binary. That is the same defect that cost **three specs** a criterion correction in this
session, so it is now impossible by construction in this script.

---

## Status of the harness flake, after the fix

| | |
|---|---|
| the class | real — `ETXTBSY` (`os error 26`) on `exec` of the fake server, 1 observed in ~20 binary runs |
| the mechanism | **not established.** An earlier claim (two tests sharing a tag) does not hold: all tags are distinct |
| the fix applied | the scratch dir is unique **per call** (atomic counter), so a collision is impossible regardless of tag |
| still open | the two **`sched.rs` timing-family** flakes, which are a different phenomenon (ordering under a wall-clock race) and are unrelated to this path |

---

## A THIRD check-mechanics bug, same family: `grep -c` exits 1 when the count is ZERO

Cleaning up this session's scratch worktrees, the guard
`dirty=$(git status --porcelain | grep -vc '^$' || echo 0)` reported **every clean worktree as DIRTY**,
because:

```
grep -c pattern          # prints "0" ... and exits 1 when nothing matched
grep -c pattern || echo 0   # so BOTH run: value becomes "0\n0"
[ "0\n0" = "0" ]         # false  -> "DIRTY", on a worktree with nothing in it
```

**`grep -c` conflates "found zero" with "failed",** and the `|| echo 0` fallback — which looks like the
careful thing to write — turned a correct zero into a poisoned value. `wc -l` is the right tool: it
always exits 0 and prints the real count.

This is the **seventh** check-mechanics bug of the session and the same family as the piped exit-status
reads: the command was fine, the *reading* of it was wrong, and the wrong reading was the one that
looked safer. Note the shape it shares with the others — **the fallback written to be defensive was
what produced the false verdict.** A guard that cannot distinguish "zero" from "failed" is not a guard.

The guard around a destructive operation must **fail safe**: if the count is unreadable, refuse and
report, never proceed. Here the bug was in the conservative direction (refusing to remove clean
worktrees), which cost nothing but would have cost a great deal had it been inverted.

---

## A FOURTH check-mechanics bug, and the sharpest of them all: a criterion satisfied by the WRONG node

Writing the exit criterion for the `laya-mcp`-on-`PATH` fix, I wrote:

```sh
… laya graph run example.flow.json 2>&1 | grep -qE '"success":true'
```

**Measured: with a stripped `PATH` the decision node FAILS and the criterion still passes**, because the
flow's *shell* node succeeded and printed its own `"success":true`. The criterion was satisfied by a node
other than the one it was about — the exact class this session spent a day removing, written into a NEW
spec by the same hand that had just been documenting it.

The corrected criterion names the decide and prints the failing node ids. **The general rule, now
learned four separate ways today: a criterion must be satisfiable ONLY by the thing it names.** A
`grep` for a token that several records share is not a check — it is a coin flip weighted in favour of
passing.

---

## REPRODUCED — the `sched.rs` timing flake, with numbers (2026-10-02, late)

Earlier in the session this family was recorded as *"observed once, never reproduced"*. **It has now
been reproduced, and the key question is answered.**

| run | load | PRE-17 commit (`10fc381`) | POST-17 (working tree) |
|---|---|---|---|
| first check | **4 busy-loop generators** | **3 failures / 20** | not run (killed mid-flight) |
| second check | 1 generator | 0 / 20 | 0 / 20 |

**The conclusion that matters: the flake fails at the commit BEFORE the scheduler change**, so it
**pre-dates spec 17** and is not a regression from it. That was previously only an inference from
measuring `0/20` in both trees — which is weak evidence, because 0/20 does not distinguish "the flake
does not exist here" from "the flake did not fire here". **3/20 at the older commit is direct evidence.**

**It is load-dependent.** The two runs used different load levels (4 generators vs 1), so their rates
are not directly comparable — the honest reading is *"it fires under heavy load and did not fire under
light load"*, not *"4× load gives 3× the failure rate"*.

**The two affected tests** are both assertions about ordering under a wall-clock race:

| test | how it races |
|---|---|
| `a_failure_stops_queued_siblings_from_starting` | `a` is `sleep 0.2; exit 1`; whether siblings are already dispatched depends on the scheduler's first loop iteration interleaving with that 200 ms |
| `a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not` | the same shape against a 0.5 s deadline on a 30 s sleeper |

### Why this is worth writing down rather than shrugging at

A test that fails 1-in-7 **under load** is a test that will fail in CI, on someone else's machine, at the
worst moment — and this project's own standard is that *a test whose control is a stopwatch is a test
that will fail on someone else's machine*. It is **not** a product defect (no node ran late; the
assertion is about dispatch ordering), but it **is** a defect in the harness.

**The fix is not a longer sleep.** It is to make the ordering deterministic — drive the scheduler with a
scripted caller that blocks until released, so the interleaving under test is *controlled* rather than
*raced*. Spec 21 hit exactly this and solved it that way: an **instant** scripted caller finished before
a zero deadline could fire, making its timeout test vacuous until a 200 ms `SlowCaller` was introduced.

**Status: open, documented, not attributed to any change.** Candidate for a small spec; it is behind
spec 23 in priority because it costs a flaky CI run rather than a wrong answer.
