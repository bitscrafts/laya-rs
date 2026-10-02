# TOOL COVERAGE — what has actually been exercised, and what has not

**Measured 2026-10-02** from the ledger's per-tool block and a live `tools/list`. Written because
the honest answer to *"we tested file reading and compaction — how about the others?"* is **no, only
some**, and that gap is invisible unless you look at what has run rather than at what is registered.

---

## The 7 advertised tools vs what has run

`tools/list` advertises seven. The ledger records what has actually executed:

| tool | calls | evidence | verdict |
|---|---|---|---|
| `decide` | 23 | p50 322 ms | **exercised** — the ten-levels harness, the graph fixtures |
| `gate` | 96 | allow 27 / ask 68, p50 204 ms | **exercised heavily** — every state-changing tool call |
| `ask_files` | 5 | p50 8,909 ms | **exercised** — the token measurement |
| `ask_file` | 1 | 1,652 ms | **exercised once** |
| `ask` | 2 | both `refused` | **only the refusal path** — the success path has never run |
| `should_compact` | 0 | — | **NEVER RUN** |
| `ledger` | 0 | — | **NEVER RUN** as a tool (the CLI `laya ledger` is a different surface) |

**"Registered" is not "tested".** Three of seven tools have effectively never been exercised through
the MCP surface: `should_compact`, the `ledger` tool, and `ask`'s success path. A tool that is
advertised, enabled and never called is indistinguishable from one that is broken — which is the
finding that started this whole line of work.

## What HAS been exercised, per surface

| surface | status |
|---|---|
| file reading (`ask_file`, `ask_files`) | exercised; measured at 43× token saving |
| compaction (`laya ledger compact`) | exercised; idempotence proven by identity |
| the ledger read path (`stats`, `tail`) | exercised; per-tool block, p50/p95/p99 |
| the gate (all 4 exit codes) | exercised, including unreachable and head-down controls |
| the graph executor | exercised — ten fixtures, route nodes, cycle refusal, fail-fast |
| **SDD+TDD as a graph** | **added 2026-10-02** — see below |
| `should_compact` | **NOT exercised** — 0 calls |
| the `ledger` MCP tool | **NOT exercised** — 0 calls |
| `ask` success path | **NOT exercised** — both calls were refusals |

---

## The SDD+TDD gate — the winning shape, encoded as a graph

The verified workflow in this workspace is: **spec → RED test → GREEN → gate → review → commit**.
That shape is now a graph, so the decision of "did the criteria pass" is made by Laya and the
**graph owns the branch** — nothing merges on a red run.

`02-guides/sdd-tdd-gate.flow.json` — validate it, render it, run it:

```sh
laya graph validate _docs/02-guides/sdd-tdd-gate.flow.json
laya graph render   _docs/02-guides/sdd-tdd-gate.flow.json
laya graph run      _docs/02-guides/sdd-tdd-gate.flow.json
```

The shape: `run_criteria` (shell) → `judge` (decide) → route on the label → three terminal branches
`merge_green` / `report_red` / `escalate_unparsed`. The `_` catch-all is **mandatory** — Laya returns
a value, never an edge, and an unmatched label must not fall through silently.

### Measured, both states

```
passing criteria:  label=passed (0.8833)  -> selected_port=merge_green  -> green branch ran
failing criteria:  label=failed (0.6956)  -> selected_port=report_red   -> red branch ran
```

Identical graph, different state, **different branch**. Only 4 of the 6 nodes ran in each, so the
unselected branches were not executed.

### Why the pair is the point

One fixture proves nothing: a graph stuck on a single branch would pass it too. The control was
verified by **breaking it** — pointing both labels at `merge_green` — and the test failed with:

```
FAILING criteria reached the green branch -- nothing must merge on red
```

Restored, and the test passes again. That is the difference between a test and a decoration.

Permanent in `crates/laya-graph/tests/acceptance.rs` as `an_sdd_gate_merges_only_when_the_criteria_pass`
and `the_control_failing_criteria_are_refused_the_green_branch`. Both are `#[ignore]`d live tests,
opt-in behind `LAYA_ACCEPTANCE=1`, so the default suite stays hermetic (measured: 1 passed,
5 ignored with the env unset).

---

## What this does NOT claim

- **`ask_files` ranking is weak on large files.** Measured: 0.7389 on a 195 KB file for a question it
  only partly answers. The branch it took was right; the confidence was not high.
- **The gate graph judges a *result line*, not the criteria themselves.** `run_criteria` runs one
  command and prints it; the judge reads that line. A richer gate would run the whole criteria list
  and judge a summary — the shape is the same, the prompt input differs.
- **`should_compact`, the `ledger` tool and `ask`'s success path remain unexercised.** Recording them
  here is the point: this document exists so the gap is known rather than assumed closed.
