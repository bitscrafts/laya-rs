# Proposed specs: order and dependencies

All six are in `specs-proposed/`, written in the house format (Verified Premises, R/N requirements,
TDD table, Exit Criteria with controls, Guardrails, Files to Modify). **Every new exit criterion
was run against `b0ee704` and FAILS there.** Carried re-checks pass by design, as house style
labels them. Raw output: `notes/proposed-criteria-on-current-tree.txt`.

```
08 gate allowlist ──────────────┐
                                ├──► 13 test hardening (R2 mutants on agentic.rs; R3 metamorphic gate)
09 scheduler fail-fast ─────────┤
        │                       └──► 13 (R3 scheduler invariants use not_started)
        ├──► 10 deadlines (same RunReport/NodeRecord structs; land after 09)
        └──► 11 observability (RunReport shape; records spec 08's gate fields)
                    ▲
08 ─────────────────┘
12 head kernel ── independent ──► (future) RustHead: Judge in laya-mcp — needs 11's Judge seam + 12
```

| order | spec | depends on | why this position |
|---|---|---|---|
| 1 | **08: `allow` requires an allowlist; `ask` validates before executing** | none | The only irreversible failure mode, measured live with a proven effect. Everything else is refinement. Ship first, alone. |
| 2 | **09: scheduler fail-fast, cycle refusal, route type as data** | none | Small (one `AtomicBool`, one length check). It fixes a spec-07 promise that is false today and changes the `RunReport` shape that 10 and 11 extend, so it goes before them. |
| 3 | **13: test hardening** (R1 hermetic + R2 mutation gate first) | 08, 09 for parts of R2/R3 | R1 (`#[ignore]` live tests) and R2's mutant killers are S effort and stop regressions of 08/09 from passing silently. R3/R4 (properties, fuzz) can trail. |
| 4 | **10: deadlines that fire** | 09 | Edits `NodeRecord`/`sched.rs` after 09 to avoid conflicting changes. Moving `shell` to `tokio::process` is the riskiest mechanical change in the set, so it lands on a hardened suite. |
| 5 | **11: observability, one run id end to end** | 08 (gate fields), 09 (`RunReport`) | Additive schema plus the `Judge` seam. It records what 08 decides and what 09/10 report, so it should land after the things it records exist. |
| 6 | **12: head forward under a second** | none (independent) | Can run in parallel with 09–11 on a separate worker. It touches only `laya-head`. It goes last in priority because it is not a defect, and first in long-term value because it unlocks the in-process Rust head. |
| — | *(not written)* **14: serve decisions from the Rust head in-process** | 11 (`Judge`), 12 | Needs its own premises (memory budget, cold load measured at 0.149 s, supervision story). Write it only after 12's bench is green. |
| — | *(not written)* **15: `compact` in one transaction with an identity index** | none | Measured O(n·m) (`notes/repro-store.txt`), but not urgent at today's row counts. One S-effort spec when the ledger grows. |

**Parallelism:** 08 and 12 share no files and can be dispatched together. 09 → 10 → 11 share
`laya-graph/src/{sched,run}.rs` and must be serial.

**One spec per worker pass.** Each is sized to one implement-gate cycle. 11 is the largest (seven
requirements across five crates). If a worker stalls on it, split R1–R3 (correctness: join,
record all tools, `bucket_clamped`) from R4–R7 (events, timeline, stats).
