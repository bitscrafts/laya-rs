# Spec 22 — a binary finds its sibling
#: depends=- status=- block=-

## Overview

A `decide` node lowers to an `mcp` node and the CLI **spawns `laya-mcp` as a child process**, resolving
the name through the **caller's `PATH`**. So every worked example in `_docs/02-guides/` works or fails
according to how the reader's shell happens to be set up:

```
WITHOUT /root/.local/bin on PATH   -> FAILED gt300 | failed to spawn `laya-mcp` … No such file or directory
WITH    /root/.local/bin on PATH   -> ALL OK
WITH LAYA_MCP_BIN=…/laya-mcp       -> ALL OK
```

Same flow, same machine, same minute — **deterministic**, and it depends only on the environment. This
is why the failure looked intermittent for several checks: my own scripts disagreed about `PATH`.

**Project**: `/projects/laya-rs`
**Phase**: 22 — a binary finds its sibling
**Source**: `_docs/03-reference/EXAMPLES-NEED-LAYA-MCP-ON-PATH.md` (measured 2026-10-02).

## Why this matters beyond convenience

Requiring the caller to arrange `PATH` means **the CLI can spawn a different `laya-mcp` than the one
shipped beside it** — an older build, from another directory, with different behaviour. That is the same
class as the stale-binary trap that cost three specs an exit-criterion correction today: a check that
passes while exercising the wrong artefact. A binary that prefers its own sibling removes the class for
the CLI-to-server edge as well.

## Requirements

### R1 — resolution order, stated and tested
`laya` resolves `laya-mcp` in this order, and a test pins each step:

1. **`LAYA_MCP_BIN`** if set — the existing documented escape hatch; if it is set and does not resolve,
   **fail with that name in the message** rather than silently falling through (a set variable that is
   ignored is the defect this project keeps finding).
2. **The sibling of the running `laya`** — the same directory as `std::env::current_exe()`.
3. **`PATH`** — today's behaviour, kept so nothing regresses.
4. Otherwise the existing error, which already names `LAYA_MCP_BIN`.

### R2 — the message names what it tried
When nothing resolves, the error lists the paths tried, in order. Today's message says only *"set
`LAYA_MCP_BIN` if it is not on PATH"*, which sends the reader looking at one option when three existed.

### R3 — the resolution is observable
A flag or a `doctor` line reports which `laya-mcp` was chosen and how (env / sibling / path). A silent
choice is indistinguishable from a wrong one; this is the same reason the deploy script prints hashes.

### R4 — the examples no longer depend on the caller
After this, every worked example under `_docs/02-guides/` must run from a shell with **no** laya
directory on `PATH` — verified by running one with `env -i`-style minimal `PATH`, not by reading code.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `the_sibling_is_preferred_over_path` | a copy of `laya` with a distinguishable `laya-mcp` beside it, and a different one earlier on `PATH` | the SIBLING is spawned, proven by which one answers |
| `the_control_path_is_used_when_there_is_no_sibling` | `laya` in a directory with no sibling | the `PATH` one is spawned — the fallback must still work |
| `a_set_but_unresolvable_laya_mcp_bin_fails_by_name` | `LAYA_MCP_BIN=/nope` | an error naming `/nope`, **not** a silent fall-through to `PATH` |
| `the_control_a_resolvable_laya_mcp_bin_is_used` | `LAYA_MCP_BIN` pointing at a real binary | that binary is spawned (near-miss: the refusal must not fire on a valid value) |
| `the_error_lists_every_path_tried` | nothing resolvable | the message contains the env name, the sibling path and `PATH` |

The first two rows are a pair, and the last three are the controls: **a resolution order that always
picks one candidate passes row 1**, so the near-misses are what make it a test.

## Exit Criteria

- [x] `cd /projects/laya-rs && cargo test -p laya-cli --lib the_sibling_is_preferred_over_path 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-cli --lib the_control_path_is_used_when_there_is_no_sibling 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-cli --lib a_set_but_unresolvable_laya_mcp_bin_fails_by_name 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] `cd /projects/laya-rs && cargo test -p laya-cli --lib the_error_lists_every_path_tried 2>&1 | grep -qE '^test result: ok\. 1 passed'`
- [x] **the examples run with no laya directory on PATH** — every node, including the decide, must succeed. Run it and check the DECIDE by name, not just that *something* succeeded:

      cd /projects/laya-rs && env PATH=/usr/bin:/bin ./target/debug/laya graph run _docs/02-guides/wc-classify-measured.flow.json 2>/dev/null | python3 -c "import sys,json; d=json.loads(next(l for l in sys.stdin if l.startswith('{'))); bad=[r['node_id'] for r in d['records'] if not r['success']]; print('FAILED:', bad) if bad else print('all nodes succeeded')"

  **This is the criterion the whole spec exists for**, and it must be run with the environment stripped, because the failure only appears that way. Do NOT substitute a `grep` for `"success":true`: **that grep is satisfied by the shell node alone** (measured: with a stripped PATH the decide failed while one `"success":true` was still printed), so it would pass while the defect is present. A criterion must be satisfied only by the thing it names.
- [x] **the control**: the same command WITH `/root/.local/bin` on `PATH` also succeeds (the fix must not break the documented setup)
- [x] `cd /projects/laya-rs && env -u LAYA_MCP_BIN ./target/debug/laya doctor 2>&1 | grep -iE 'laya-mcp|server'` : names which server was resolved (R3)
- [x] `cd /projects/laya-rs && cargo test --workspace`
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings` : 0
- [x] `cd /projects/laya-rs && cargo fmt --all --check` : clean

**Every criterion uses `-p <the owning crate>` and `ok\. 1 passed`** — a criterion that accepts
`ok. 0 passed` is satisfied by a test name matching nothing, which is how an earlier spec's criteria
passed with nothing implemented. **Verify each is non-vacuous before starting.**

## Guardrails
- Do NOT commit. Leave the work in the tree.
- Do NOT add a crate or dependency.
- **Do NOT change the documented precedence for `LAYA_MCP_BIN`** — it is set in the deployment and in
  the test helpers; it must keep winning when it resolves.
- Do NOT weaken an existing test. The test helpers set `LAYA_MCP_BIN`, so they should be unaffected —
  if one is not, re-pin it with a comment naming this spec rather than relaxing it.
- Do NOT change the wire contract or the payload shape.
- Do NOT modify `specs/`.
- **Run graphs, do not only read code** (operator instruction, verbatim: *"graph must be ran from
  laya!"*). Two criteria require real runs with a stripped environment, which is the only way this
  defect is visible.
- Do not delete anything; `mv` aside and say so.

---

## Verification of this spec (run here, 2026-10-02, after the worker finished)

### The criterion the spec exists for, and its control

```
$ env PATH=/usr/bin:/bin ./target/debug/laya graph run _docs/02-guides/wc-classify-measured.flow.json
  stripped PATH  -> ALL NODES SUCCEEDED        (before this spec: FAILED gt300)

$ env PATH=/root/.local/bin:/usr/bin:/bin ./target/debug/laya graph run …same flow…
  full PATH      -> ALL NODES SUCCEEDED        (the documented setup still works)
```

**All four other committed examples also succeed with a stripped `PATH`** (`wc-classify`,
`wc-classify-chained`, its control, `wc-classify-band`) — each runs a decide, so each was broken before.

### The resolution is observable, and a bad setting is refused by name

```
$ laya doctor
  laya-mcp  : /projects/laya-rs/target/debug/laya-mcp (resolved via sibling)

$ LAYA_MCP_BIN=/nope/nothing laya graph run …
  laya: LAYA_MCP_BIN is set to `/nope/nothing` but that does not resolve to a `laya-mcp`;
        it is not silently ignored — fix or unset it
```

That second line is the requirement that matters most: **a set-but-unresolvable variable must fail by
name, never fall through.** A set variable that is silently ignored reads as a working configuration.

### Everything else

| check | result |
|---|---|
| 4 named tests | **4/4 PASS**, each `ok. 1 passed` |
| `cargo test --workspace` | **605 passed, 0 failed** |
| clippy `-D warnings` / fmt / doc citations | 0 / clean / 69 docs resolve |

### Note on how this was found, and why it took three attempts

The defect was **deterministic all along** — it depended on whether the caller's `PATH` held the install
directory. It was first seen as an intermittent failure and chased as a race, because my own scripts
disagreed about `PATH` and **a nondeterministic cause and a differing environment look identical from a
single result.** `cargo run` masks it entirely (it puts `target/debug` on `PATH`), which is why several
verification passes missed it.
