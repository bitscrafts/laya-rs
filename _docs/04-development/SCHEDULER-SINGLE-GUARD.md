# THE SCHEDULER'S SINGLE GUARD, AND THE MUTANTS THAT HANG IT

**Measured 2026-10-02**, from the mutation gate's own preserved output at
`/projects/_tmp/mutants-out-laya-rs/mutants.out/`. This is the finding behind spec 17's one unticked
criterion, and it is more interesting than "the gate exits 3".

## The facts, from the gate's artefacts rather than a re-run

```
missed.txt   0 bytes          <- ZERO mutants survived
timeout.txt  2 mutants        <- both at crates/laya-graph/src/sched.rs:89, i.e. `dependency_tables`
caught.txt   193 mutants
unviable.txt 20
```

`cargo-mutants` classifies a **timeout** separately from a **miss** precisely because a hang is neither:
the tests did not pass, and they did not fail — they never finished. **0 missed is the property the gate
exists to protect, and it holds.**

## The mutated code, verbatim from the gate's own log

```diff
--- crates/laya-graph/src/sched.rs
+++ replace dependency_tables -> (…) with (HashMap::from_iter([(String::new(), 1)]), HashMap::new())
 fn dependency_tables(graph: &Graph) -> (HashMap<String, usize>, HashMap<String, Vec<String>>) {
-    let mut indegree = HashMap::new();
-    let mut successors: HashMap<String, Vec<String>> = HashMap::new();
-    for node in &graph.nodes { indegree.entry(node.id.clone()).or_insert(0); }
-    for edge in &graph.edges {
-        *indegree.entry(edge.to.clone()).or_insert(0) += 1;
-        successors.entry(edge.from.clone()).or_default().push(edge.to.clone());
-    }
-    (indegree, successors)
+    (HashMap::from_iter([(String::new(), 1)]), HashMap::new()) /* ~ changed by cargo-mutants */
 }
```

The mutant returns a table **inconsistent with the graph it was handed**: the indegree map holds one
phantom key (`""`) and no real node id at all, or a phantom key plus an empty successor list.

## Why it hangs — from the code, and honestly labelled as inference

- `dependency_tables`'s own doc comment says it is *"built once and shared by `topo_order` and `run`, so
  the two can never disagree about the graph's shape."* A mutation breaks that shared assumption: the
  function now returns a table about **no node in the graph**.
- `refuse_cycle` (`sched.rs:526`) is **the only cycle guard**, and it is built on `topo_order`, which is
  built on the **same** `dependency_tables`. So a table that is wrong in this way defeats the one check
  that protects the DAG assumption.
- **There is no spin bound anywhere in the scheduler.** Greps for `steps`, `debug_assert`,
  `unreachable!`, `spin` and `iterations` in `sched.rs` return nothing. The dispatch loop
  (`while let Some(id) = pop_first(&mut tracker.ready)`, `sched.rs:405`) re-inserts nodes through
  `resolve`, so its termination rests entirely on "the graph is a DAG and every node is settled once".
- The timeouts are reported as **`0s build + 37s test`** — the hang is in a test, not the build.

**What I could NOT determine:** which test hangs. `cargo-mutants` captures output only for mutants that
produce it, and its log files for these two contain the diff and no test transcript — a hanging test
prints nothing. **Inference:** a test that drives the scheduler with a graph and awaits completion will
await forever when the tracker never settles a node. I did not reproduce it, so I do not claim the
mechanism — only the code shape that permits it.

## Why this is worth a spec and not just a footnote

**A scheduler that waits forever instead of reporting is a production failure mode, not a test
artefact.** It holds fanout permits while spinning, so in a real run it would block the worker pool and
never return a report. Today the only thing standing between that and a user is one function's
correctness, enforced by nothing.

The right hardening is not "make the mutants pass" — an inconsistent table is a bug that should be
impossible, not tolerated. It is:

1. **Assert the invariant at the source.** `dependency_tables` must contain an entry for every node id
   in `graph.nodes`. A violation should fail fast with a message naming the missing ids, because every
   consumer — `topo_order`, `refuse_cycle`, the `Tracker` — depends on it.
2. **Bound the scheduler's progress.** A step counter (or an assertion that every iteration either
   spawns, settles or skips a node) turns a hang into a named failure in a second, instead of a 37-second
   silence with no test name. **A hang with no name is the worst possible signal** — it is exactly why
   these two mutants are classified separately and why the gate exits 3 rather than 2.

Both are filed as `specs/23-the-scheduler-cannot-wait-forever.md`.

## The reading this produces about the gate itself

The criterion *"`mutants-gate.sh` exits 0"* is **weaker than the property it means to protect** — the
same complaint the project recorded before: *exit criteria weaker than the repo's gate is what produces
every avoidable failure*. The property is **zero missed mutants**, which holds. `exit 0` additionally
demands **no hanging mutants**, which is a different and stronger claim — and turning it into a pass
would mean asserting that an inconsistent dependency table is a *viable* mutation, which it is not.

**So the honest criterion is: `missed.txt` is empty, and every timeout is explained.** That is what the
spec 17 criterion should have said, and this document is the explanation.
