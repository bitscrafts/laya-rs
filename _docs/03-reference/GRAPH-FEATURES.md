# GRAPH FEATURES — composability, branches, loops: what is implemented

**Measured 2026-10-02**, by running `laya graph validate|run` — not by reading code. Every answer
below has a command and its output. The head and sidecar were live throughout.

---

## The short answer

| feature | status | evidence |
|---|---|---|
| **Branches** | **YES — implemented and working** | a committed example renders 22 edges and takes 4 routes in one run |
| **Composability (subflows)** | **IN THE LIBRARY, NOT IN THE CLI** | `expand()` handles it and has unit tests; `laya graph` has no library flag, so a `subflow` node is rejected through the CLI |
| **Loops** | **NO — refused by design** | a 3-node cycle is refused with its path named, in both `validate` and `run`; the schema has zero loop fields |

---

## 1. Branches — YES

`laya graph render` on the committed branching example prints **22 edges**. A run takes **4 routes**
and executes only the nodes on the selected paths:

```
routes taken: 4
  {"on":"$.payload.band","value":"mid","selected_port":"gt300_ask"}
nodes that RAN: ['gt300_gate', 'gt300_ask', 'gt100_gate',
                 'gt100_ask', 'class_short', 'count_words']
```

Two branch kinds exist and both work: route on `label` (exact match of the answered option) and route
on `band` (`low`/`mid`/`high`, computed by us from the policy thresholds). A `_` catch-all port is
**mandatory** — the engine refuses a flow whose ports do not cover every outcome.

**Caveat, measured and serious.** Which branch a *nested* decision takes is not reliable when two
decisions are involved: routing depends on **node order in the JSON array**, not only on the graph.
See `OPUS-REVIEW-GENERIC-GRAPHS.md` C1 — same content, different array order, different nodes
executed, and the run report says nothing. Simple branching works; a *tree* of decisions mis-executes
unless the parent is listed before the child.

## 2. Composability — the library has it, the CLI does not

**In the library: implemented, with tests.** `expand(&root, &library)` substitutes a `subflow` node
by its child flow, prefixing ids (`v/compile`), refusing an unbound argument by name
(`FlowError::UnboundArgument { argument: "suite" }`), and handling two uses of one child without id
collisions.

**Through the CLI: rejected.** Our probe:

```json
{ "flow": "parent", "version": 1,
  "nodes": [ { "id": "v1", "kind": "subflow", "uses": "build" } ] }
```

```
$ laya graph validate parent.flow.json
laya: node `v1` depends_on unknown node `build`
```

Three findings in that one line:

1. **`Subflow` is not a variant of `Kind`.** The enum is `Shell | Mcp | Decide | Route | Other(String)`
   and `subflow` falls through to `Other`. Yet `expand.rs` and its tests clearly handle it — so the
   schema and the expander disagree about what a `subflow` node is.
2. **The error message is wrong.** It says `depends_on unknown node \`build\`` for a field that
   contains no `depends_on` and where `build` is a flow name, not a node. A flow author is sent to
   look at the wrong field.
3. **The CLI cannot load a library.** `crates/laya-cli/src/graph.rs:50-51` says it outright: *"empty
   library: the CLI does not yet accept a library file — out of scope for [the current part]"*. So
   every `subflow` node is unresolvable from the CLI, even a correct one.

**So composability exists as library code and is unreachable from the tool a flow author uses.**

## 3. Loops — NO, and the refusal is good

A 3-node cycle (`a → b → c → a`):

```
$ laya graph validate cycle.flow.json
laya: dependency cycle: a -> c -> b -> a
exit=1
$ laya graph run cycle.flow.json
laya: dependency cycle: a -> c -> b -> a
exit=1
```

Refused in **both** commands, with the **cycle path named**. And there is no loop construct to reach
for instead: `grep -cE 'loop|iterate|until|repeat' crates/laya-graph/src/flow.rs` → **0**.

This is a deliberate design, not a gap: the graph is a **DAG**. Spec 17 reinforces it, and spec 06's
`specs/06-priority-queue.md` models the worker pool rather than looping the graph. An iterative
workflow must be expressed either as a bounded unrolled sequence of nodes, or outside the graph.

**Worth noting:** the cycle refusal is one of the few things in this engine that behaves exactly as
documented — it refuses loudly, names the offender, and does so on both entry points. That is the
standard the rest of the engine does not yet meet.

---

## Reproduce

```sh
export PATH=/root/.local/bin:$PATH
cd /projects/laya-rs
bash /projects/_tmp/test-engine-features.sh
```

The probe flows are in `/projects/_tmp/engine-features/` (`parent.flow.json`, `child.flow.json`,
`cycle.flow.json`).

## Not claimed

- Whether a **nested** subflow (a child that itself uses a subflow) expands correctly — `expand.rs`
   has an `ExpansionCycle` error which implies recursion is bounded, but I did not run one.
- Whether the `Other(String)` fall-through for `subflow` is a schema bug or an unfinished migration —
  both are plausible from the evidence and I did not find a commit that decides it.
- Loop-like behaviour achieved by re-running the same flow from outside (a shell `for` loop invoking
  `laya graph run` repeatedly). Untested; it is the obvious workaround if iteration is needed.
