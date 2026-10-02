# GRAPH EXAMPLE — the guessing game, and the boundary it is built to state

**Measured 2026-10-02** by running every command shown here on the freshly built
`./target/debug/laya`. It is the second worked example beside
[`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md), built to exercise the surface a
person would actually use: **state from a command reaches a decision, and the decision's own
answer selects which branch runs.** The formats it follows are
[`GRAPH-SPEC.md`](../03-reference/GRAPH-SPEC.md) and its refusals
[`GRAPH-FORMAT-CONTRACT.md`](../03-reference/GRAPH-FORMAT-CONTRACT.md).

The two flows committed with this document:

| file | the round it plays | advice node reached (measured) |
|---|---|---|
| [`guess-the-number.flow.json`](guess-the-number.flow.json) | secret 41, guess 57 | `advise_lower` |
| [`guess-the-number.control.flow.json`](guess-the-number.control.flow.json) | secret 41, guess 3 | `advise_higher` |

---

## 1. The round is a graph, not a script

Six nodes, one judgement:

```
setup (shell: prints the round's two facts as one line)
   |
   +--> judge (decide, prompt == "{{setup}}")            <- the head judges the ACTUAL round
          |
          *-- yes --> advise_lower  (shell: "go lower")   --+
          *-- no  --> advise_higher (shell: "go higher")  -+   (join: section 4)
          *-- _   --> escalate (an unparsed answer is NAMED, never silently routed)
```

- **`setup`** (`shell`) is where the round's numbers live. It prints ONE line —
  `The secret is 41. The guess is 57.` in the main flow, `The secret is 41. The guess is 3.`
  in the control. The numbers are produced by the flow's own shell command, deterministically:
  there is no clock, no `$RANDOM`, nothing a second run would change (NF1 — section 6).
- **`judge`** (`decide`) has **`"prompt": "{{setup}}"`** — the placeholder is replaced, just
  before the call, by `setup`'s trimmed stdout (spec 18;
  `laya-graph::placeholder`, `crates/laya-graph/src/placeholder.rs:58`). **The numbers appear
  nowhere in this file except inside `setup`'s command.** That is the property the earlier wc
  examples were measured lacking (the count sat in the prompt as a literal while the `wc` node's
  output went nowhere — [`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md), the
  correction at its top). The question is a separate field: *"Is the guess higher than the
  secret?"* — prompt is the state, question is what is asked.
- **Why a 2-option `choice` on `label`.** A flow-authored `decide` is ALWAYS a `choice` on the
  wire (`LiveMcpCaller` rebuilds it as `{type: "choice", ...}` with `criteria` keyed by the
  options; a `noul` from a flow would lower to empty criteria, which `laya-mcp` refuses —
  measured, [`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md) §6.2). This head's
  measured strength is exactly the 2-option numeric comparison (15/15 on the boundary sweep,
  §6.3 of the same document). `judge` therefore asks the measured shape: two options, `yes` /
  `no`, routing on `label` (R1 fixes it), with ports `{"yes": "advise_lower", "no":
  "advise_higher", "_": "escalate"}`.
- **`advise_lower` on `yes`**, **`advise_higher` on `no`** — the graph owns every edge: the head
  answers ONE of the pre-drawn ports and can invent neither an edge nor a branch. A guess
  exactly EQUAL to the secret also answers `no` here (a two-option question covers the whole
  axis in halves); the two flows never hit that case, and the one-round boundary (section 3)
  makes it moot for the game.
- **`escalate` on `_`** is the catch-all a `decide` must carry: an answer the ports do not
  declare reaches a node that NAMES it — nobody routes it silently.

## 2. The measured runs — node by node

`./target/debug/laya graph run _docs/02-guides/guess-the-number.flow.json` (rc 0,
`stopped_early: false`), records by start time:

```
setup        started 0ms   exit 0   output: The secret is 41. The guess is 57.
judge        started 1ms   ok       payload: {"label":"yes","band":"low","confidence":0.2484,
                                     "probabilities":[0.7847,0.2153],"options":["yes","no"],...}
judge/route  started 244ms ok       {"on":"$.payload.label","value":"yes","selected_port":"advise_lower"}
advise_lower started 244ms exit 0   output: ADVICE: the guess is too high -- go lower
emit_move    started 244ms exit 0   output: MOVE: the round is judged -- the branch the graph
                                     selected is the player's move for this round

skipped: ["advise_higher", "escalate"]     (the branches the graph did not select)
```

The control, same command on `guess-the-number.control.flow.json` (rc 0, `stopped_early:
false`):

```
setup          started 0ms   exit 0   output: The secret is 41. The guess is 3.
judge          started 1ms   ok       payload: {"label":"no","band":"low","confidence":0.268,
                                        "probabilities":[0.2051,0.7949],"options":["yes","no"],...}
judge/route    started 170ms ok       {"on":"$.payload.label","value":"no","selected_port":"advise_higher"}
advise_higher  started 170ms exit 0   output: ADVICE: the guess is not higher -- go higher
emit_move      started 170ms exit 0   output: MOVE: the round is judged -- ...

skipped: ["advise_lower", "escalate"]     (the control's skipped list names the branch the
                                           main flow took — the branch-selection evidence)
```

Both calls were labelled correctly: `yes` for 57-over-41, `no` for 3-under-41, with margins
0.569 / 0.590 — the head is right on the shape it was measured strong at. **The bands are
`low` on both calls** (confidence 0.2484 / 0.268 against the policy's 0.5 line), so this pair
is also a small live specimen of the label-vs-band trade-off: R1 routes on `label`, and the
payload's `band` is right there in the record for the reader to see the honest uncertainty.
Which routing to prefer when a branch is CONSEQUENTIAL — the `band` gate that escalates low
confidence instead of acting on a disowned label — is the wc example's lesson, with its own
measured gate→ask shape ([`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md) §4 and
§6); this example is deliberately the plain `label` shape the spec fixes.

A judge of a run reads the report, never the exit code: `records`, `skipped`,
`stopped_early`, `failed_node`. A node that fails while the process exits 0 is exactly the
kind of thing `stopped_early` is there to expose.

## 3. THE BOUNDARY — one round per run (measured)

**This engine has no loops.** A cycle is refused, `retry` is refused, and no iteration
construct exists in the format. What that means is not hidden and not worked around:
**`guess-the-number.flow.json` plays ONE round and nothing more.** The flow is a round, not a
game.

Measured 2026-10-02, both sides of the claim:

- a cycle is refused AT LOAD — `laya graph validate` and `laya graph run` on this probe
  (3 shell nodes: `a` depends on `c`, `c` on `b`, `b` on `a`):

  ```json
  {
    "flow": "cycle-probe",
    "version": 1,
    "description": "a depends_on c, c on b, b on a -- the engine must refuse this, naming the path",
    "nodes": [
      { "id": "a", "kind": "shell", "depends_on": ["c"], "run": "echo a" },
      { "id": "c", "kind": "shell", "depends_on": ["b"], "run": "echo c" },
      { "id": "b", "kind": "shell", "depends_on": ["a"], "run": "echo b" }
    ]
  }
  ```

  ```
  $ laya graph validate cycle-probe.flow.json
  laya: dependency cycle: a -> c -> b -> a                     (exit 1)
  $ laya graph run cycle-probe.flow.json
  laya: dependency cycle: a -> c -> b -> a                     (exit 1, nothing ran)
  ```

  The refusal is `FlowError::Cycle`, rendered `dependency cycle: a -> b -> a`
  (`crates/laya-graph/src/error.rs:227`), produced by the DFS in
  `laya-graph::validate`. Behind it — for a graph that reaches the scheduler without going
  through `validate` — the run path has its own guard, `refuse_cycle`
  (`crates/laya-graph/src/sched.rs:579`), raising `RunError::Cycle` ("these nodes can never
  run: …", `crates/laya-graph/src/error.rs:327`), pinned by the committed test
  `a_cyclic_graph_is_refused_not_partially_run` (`crates/laya-graph/src/sched.rs:1594`) with
  its control (a diamond is not a cycle).
- `retry` is refused, and names the alternative (`crates/laya-graph/src/validate.rs`, spec 20
  row 9 — also re-measured 2026-10-02 by running a one-node flow carrying
  `"retry": {"attempts": 3, "backoff_ms": 10}`):
  `laya: node `a` sets `retry`, which is not implemented … Wrap the command in a shell loop, or
  remove `retry`` — exit 1.

**The two honest ways to play N rounds:**

1. **Instantiate N copies** of this one-round flow — N files, each with its round's guess in
   `setup`'s command, each judged and advised on its own run. A loop-free engine plays a loop
   the only way a static graph can: by having N rounds written down.
2. **Drive the loop outside the graph.** The CALLER (an agent, a script, the orchestrator)
   keeps the loop: run the flow, read which advice node the report names, pick the next
   secret/guess pair, write the next round, run again. The graph stays stateless round-shape;
   the iteration lives in whoever runs it.

**This document does not claim the graph loops, retries, or plays until it wins. It does not.**

## 4. The join on `emit_move`, and the engine fact that forces it

The two advice nodes converge on `emit_move` in the flow document — but **not** via
`depends_on` on the advice nodes, because of a measured engine fact:

- An edge is one of two classes (`crates/laya-graph/src/sched.rs:24`): a
  `depends_on` edge is an **AND** — if any predecessor is skipped, the node is skipped, and
  the skip propagates — while a route's port edge is an **OR** — the target is live when its
  route selected it.
- A decide's route always selects exactly ONE port; the branch it did not select is in
  `skipped`, and its downstream AND edges resolve dead. So a join node with `depends_on` on
  BOTH advice branches would be **skipped on every run**. This is pinned by the committed test
  `a_node_whose_dependency_was_pruned_is_skipped_and_named`
  (`crates/laya-graph/src/routing_order.rs:123`) over
  `crates/laya-graph/tests/fixtures/p7-join.flow.json`, not inferred here.
- `emit_move` therefore depends on `judge` — the node that always runs and always precedes the
  route selection. It fires exactly once per round, on the selected path after (concurrently
  with) the selected advice node, and on the `_` path beside the escalation. What it prints is
  worded to be true on both: the round is judged; the selected branch's record is the move.
- Two routes may share ONE target (`two_routes_sharing_a_target_both_reach_it`,
  `crates/laya-graph/src/routing_order.rs:137`), which is the engine's other join shape; this
  example needs no second head call, so it does not use one.

**Orphaned nodes run anyway — measured here by accident, recorded on purpose.** While breaking
the control (section 5), a port was retargeted so that NO edge reached `advise_higher`
anymore. The run still executed it: a node with no inbound edge is ready from the start — the
scheduler's ready set is every node with no unresolved AND and no unresolved port edge
(`crates/laya-graph/src/sched.rs:367`) — so it runs on EVERY run, the same defect class as the
near-tie target measured in [`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md) §6.9.
`validate` does not refuse an orphan: `PortTargetMissing` covers a missing target, nothing
covers a targetless node. The full pair reached `['advise_higher', 'advise_lower']` —
REPORTED, not fixed; `crates/` is outside spec 24's scope, and the fix belongs in `validate`.

## 5. The control actually controls — red by construction, and it went red

`check-guess-branches.py` (the checker the spec's exit criterion names; it lives in the
workspace scratch that criterion points at, and it runs BOTH flows through
`./target/debug/laya` and compares which advice node each reached). The comparison, inline,
for a reader of this repository alone — read both run reports and take the reached advice
node from each:

```sh
laya graph run _docs/02-guides/guess-the-number.flow.json         > /tmp/main.json
laya graph run _docs/02-guides/guess-the-number.control.flow.json > /tmp/ctrl.json
python3 - <<'EOF'
import json
reached = lambda p: sorted(r['node_id'] for r in json.load(open(p))['records'] if r['node_id'].startswith('advise_'))
print('BRANCHES DIFFER' if reached('/tmp/main.json') != reached('/tmp/ctrl.json')
      else 'BRANCHES IDENTICAL -- the control does not control anything')
EOF
```

On the pair as committed:

```
flow    reached: ['advise_lower']   skipped: ['advise_higher', 'escalate']
control reached: ['advise_higher']  skipped: ['advise_lower', 'escalate']
BRANCHES DIFFER
```

**The break-the-port drill (2026-10-02),** so "the branch works" is falsifiable and not a
self-report:

- **Break 2 — the one the control catches.** The MAIN flow's two port targets were SWAPPED
  (`yes -> advise_higher`, `no -> advise_lower`); `validate` still calls the flow valid (both
  targets exist), and the head still answers `yes`. Both flows then reached
  `['advise_higher']`:

  ```
  flow    reached: ['advise_higher'] skipped: ['advise_lower', 'escalate']
  control reached: ['advise_higher'] skipped: ['advise_lower', 'escalate']
  BRANCHES IDENTICAL -- the control does not control anything      (exit 1)
  ```

  Both files were restored from same-run copies and the checker returned to
  `BRANCHES DIFFER` (exit 0). **The pair collapses exactly when routing is broken** — the two
  runs' difference comes from the round's input (the guess), not from anything the control
  hardwires.
- **Break 1 — the one it does NOT catch, and what it teaches.** The CONTROL's `no` port was
  repointed at `advise_lower` so no edge remained into `advise_higher`. The run selected
  `advise_lower` — and `advise_higher`, now orphaned, ran anyway as a tail root (section 4);
  the pair reached `['advise_lower']` vs `['advise_higher', 'advise_lower']`, which still
  compares DIFFER. A checker that only compares the reached set cannot see a misroute once an
  orphan joins it — read `selected_port` in the route record, and treat any node that runs on
  every run as the tell of an edge that was lost. Restored from its same-run copy
  afterwards; `BRANCHES DIFFER` again measured.

## 6. Determinism — the branch follows the round, not the clock (NF1)

Each flow was run twice back to back and the reports compared:

```
MAIN flow:    run 1: advice=advise_lower  skipped=[advise_higher, escalate]  label=yes
              run 2: advice=advise_lower  skipped=[advise_higher, escalate]  label=yes
CONTROL flow: run 1: advice=advise_higher skipped=[advise_lower, escalate]  label=no
              run 2: advice=advise_higher skipped=[advise_lower, escalate]  label=no
```

Identical advice node and identical skipped set on both pairs — the numbers come from
`setup`'s own command. (The head's confidence happened to be bit-identical across these runs
too, 0.2484 and 0.268; across processes it is NOT guaranteed — one identical call was
measured wobbling Δ0.046 in [`GRAPH-EXAMPLE-WORDCOUNT.md`](GRAPH-EXAMPLE-WORDCOUNT.md) §6.9.
If a rerun here ever answers differently, suspect the round's input changed first, and do not
weaken the criterion.)

## 7. Reproduce

```sh
export PATH=/root/.local/bin:$PATH     # only if using the installed binary; the installed
                                       # one may be stale — build first, or do what every
                                       # criterion here does and run ./target/debug/laya
cd /projects/laya-rs
cargo build

./target/debug/laya graph validate _docs/02-guides/guess-the-number.flow.json
./target/debug/laya graph validate _docs/02-guides/guess-the-number.control.flow.json
./target/debug/laya graph run      _docs/02-guides/guess-the-number.flow.json
./target/debug/laya graph run      _docs/02-guides/guess-the-number.control.flow.json
```

`graph run` needs no `laya-mcp` on PATH: the decider's sidecar is resolved as a SIBLING of the
running binary (spec 22), measured here by running the flow under `env -i` (empty
environment) with the binary's absolute path — the run completed, called the head, and took
the same branch (`advise_lower`, `skipped: ["advise_higher", "escalate"]`). The shell nodes run
`sh -c` in the caller's working directory, which is why they print a line instead of reading a
path: **these flows read and write no files at all**, so there is no input document to track
(NF4) — compare the wc examples' original sin, an input document that lived outside the
repository and vanished in a clone.

Render (the graph owns every edge — ports drawn at author time):

```
n2 -->|no| n4     %% judge/route -> advise_higher : no
n2 -->|yes| n3    %% judge/route -> advise_lower : yes
n2 -->|_| n5      %% judge/route -> escalate : _
```

## 8. What this example does NOT claim

- It does not claim the graph loops, retries, or plays a game to completion. ONE round per
  run; N rounds need N instantiations or an external loop (section 3).
- It does not claim band-gating. It routes on `label` because R1 fixes `judge/route` on
  `label`; the band is in the payload and honestly low on these very calls — for consequential
  branching, read the wc example's gate→ask pattern.
- It does not claim the head answers `noul` from a flow — a flow decide is a `choice` on the
  wire, measured (wc example §6.2).
- Its numbers are IN `setup`'s command and nowhere else: swapping them between flows is the
  whole control story, and the prompt carries the placeholder, never a literal.

---

## Playing it: the game converges correctly but **cannot be won** (measured 2026-10-02)

Because the graph has no loop, playing means running the flow repeatedly with the guess substituted —
**the mode this document recommends**, driven here by a binary search that knows only what the graph
tells it. Six rounds against a secret of 41:

```
round  guess  label  branch          advice
    1     50  yes    advise_lower    the guess is too high -- go lower
    2     25  no     advise_higher   the guess is not higher -- go higher
    3     37  no     advise_higher   ...
    4     43  yes    advise_lower    ...
    5     40  no     advise_higher   ...
    6     41  yes    advise_lower    <- 41 IS the secret, and it still said "too high"
```

**The routing is right every round and the search converges in 6** — the graph works. But the game has
**no win condition**: the question is a two-option comparison, so when the guess *is* the secret the
graph still answers "it is higher". The player can land on the secret and never be told.

### The obvious fix is worse — measured, not assumed

A three-option question (`lower` / `equal` / `higher`) was tried:

| round | label | branch | verdict |
|---|---|---|---|
| guess 41 vs secret 41 | `equal` | `you_win` | correct |
| **guess 90 vs secret 41** | **`equal`** | **`you_win`** | **WRONG — declares a win 49 away** |

So it trades "cannot win" for **"declares a win on a wrong guess"**, which is the worse failure for a
game. This matches the head's measured behaviour on question shapes: it tracks a two-option comparison of
two numbers reliably, and does **not** track a three-way numeric judgement.

Carrying three named ports on a `noul` is refused rather than silently mis-routed:

```
laya: decide node `judge` has port key `equal`, which is not one of the label values ["yes", "no"] or `_`
```

That refusal is correct — it is the check that stops a mistyped port key being hidden by the catch-all.

### What to do instead

**Equality is a shell job, not a judgement.** Keep the graph asking the comparison it answers well (two
options, one boundary), and test the win condition outside the graph where the loop already lives — or
in a shell node, which does exact arithmetic and has no opinion to be wrong about.

The example is left as it is, and this section is the reason: it demonstrates the graph routing on a
decision **correctly**, and it documents — from a real played game — exactly where that stops being
enough. A reader who wants a winnable game now knows the shape to build, and knows that asking the model
for a three-way numeric comparison is not it.

