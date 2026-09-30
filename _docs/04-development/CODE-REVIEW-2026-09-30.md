# Code review — laya-rs, 2026-09-30

**Method**: deterministic audit (`codebase-audit` 2.0.0) + hand-verification of every
PROVEN finding + targeted reads of the fail-closed paths. No claim below is from the
tool's summary alone; each was re-read at its `file:line`.

**Verbatim audit result**:

```
audit        codebase-audit/2.0.0
repo         /projects/laya-rs
check        new_proven
verdict      BLOCKED
proven       15
suspect      218
by_check     A0=1 A1=40 A11=3 A13=46 A5=1 A8=142
RUN OUTCOME: repo=/projects/laya-rs verdict=BLOCKED proven=15 suspect=218 new=15 rc=1
```

`--selftest` = **PASSED** (`each check above FIRED on a fixture`), so a zero from any
check is meaningful rather than "never ran".

**Read `BLOCKED` with the split below.** The repo-root run mixes shipped code with
40+ scratch scripts, and that mixture distorts the result in both directions — it
under-counts `crates/` (see the CORRECTION section) and it counts `_tmp/` noise as
findings:

| Where | PROVEN (repo-root run) |
|---|---|
| `crates/` (the shipped code) | 8 |
| `scripts/laya-sidecar.py` | 1 |
| `_tmp/` scratch | 6 |

**The authoritative figure for the shipped code is 18** (11 A1 + 7 A3), from auditing
`crates/` directly. Use that number; the 8 above is what the root run could see.

---

## CORRECTION AND ADDENDUM (2026-09-30, later the same session)

The split above was hand-classified by `file` path. Re-running the audit on the **source
subtree** — the shipped code by itself — produces a materially different and more
accurate result, and it exposed findings the whole-repo run could not see:

```
RUN OUTCOME: repo=/projects/laya-rs/crates verdict=BLOCKED proven=18 suspect=51 new=18 rc=1
by_check     A0=1 A1=41 A3=26 A5=1
```

**The `crates/` run is the one to trust**, and `proven=18` (A1=11, **A3=7**) is the
correct figure — *not* the `proven=11` a first pass of this same command reported. See
the tooling note below; that discrepancy is itself the finding.

Why the subtree run differs from the repo-root run:

* **The reference set is the tree scanned.** `_tmp/fix-queue.py` contains verbatim
  copies of the queue's test assertions, so `QueueStats::stats()` **looked referenced**
  in the whole-repo run. It is not — every real call site is inside
  `crates/laya-core/src/queue.rs`'s own `#[cfg(test)]` module. Scratch files posing as
  references suppressed genuine findings.
* **The repo-root run silently skipped A3** (`by_check A0=1 A1=40 A11=3 A13=46 A5=1 A8=142`
  — no A3), despite `cargo machete` finding unused deps. The subtree run reports
  `A3=26`.

### Additional findings

* **A1 — `QueueStats::stats()` (`queue.rs:173`)**: zero real callers. Every `.stats()`
  call in the workspace is inside `queue.rs`'s own test module. Consistent with C2 —
  the counters are written but nothing outside the tests reads them.
* **A1 — `PriorityQueue::capacity()` (`queue.rs:167`)**: only called from a test
  (`queue.rs:482`). Its own doc says "The configured capacity", implying a production
  reader.
* **A1 — `QueueStats::sheds` reads: 0 in production.** Produced and never consumed,
  which is what a queue that never sheds produces.

### A3 = 26 findings, 7 PROVEN — unused dependencies

`cargo machete` agrees. One correction to my own suspicion: **`ureq` in `laya-backbone`
really is unused** — `ureq::` appears only in `laya-mcp` and `laya-cli`. My first probe
matched the *doc comments* that mention `ureq` by name, which is exactly why a
deterministic tool beats reading.

### Two A1 hits that are FALSE POSITIVES — do not "fix" these

`main` in `crates/laya-cli/src/main.rs:142` and `crates/laya-mcp/src/main.rs:34` are
process **entry points**. They have no in-code caller by design. The tool's `--selftest`
covers framework-handler detection; a bare `fn main` in a binary crate is the residual
case. Recorded so nobody wires `main` to something.

### Tooling note: A3 disappeared silently in ~1 of 9 identical runs

Nine runs of the **same command on the same tree**:

| observed | frequency | `by_check` | proven |
|---|---|---|---|
| normal | 8 of 9 | `A0=1 A1=41 A3=26 A5=1` | **18** |
| degraded | 1 of 9 | `A0=1 A1=41 A5=1` — **A3 absent, no error** | **11** |

In the degraded run A3 produced **no findings and no "check could not run" notice**, so
7 PROVEN findings vanished and the total dropped 18 → 11 with nothing in the output
saying so. The only reason it was caught is that two runs of the same command gave
different totals.

Cause is most likely my own concurrency: the degraded run overlapped with hand-run
`cargo machete` and `cargo build`, so `cargo machete` may have failed under lock
contention and A3 passed the failure through as silence — the skill's own documented
pitfall (*"never treat a failed external command as 'no findings': surface it as a
finding"*).

**Two honest notes on my own process here.** I first called this "not reproducible" after
three agreeing runs — that was premature; three agreeing runs do not bound an
intermittent failure, and the baseline run immediately contradicted me. And the first
`proven=11` I quoted was a degraded run, which would have under-reported the tree's real
state had I not re-run it.

**Operational consequence, regardless of cause**: when `by_check` does not list A3, treat
A3 as **not run**, never as clean — absent-from-`by_check` and found-nothing are
indistinguishable in this output. Cross-check the `proven` total against the baseline
count (`18`), which is what caught it.

### Baseline recorded

`_docs/04-development/audit-baseline.json` — **18 entries**. The known set is now
acknowledged, so the gate fails only on something **new**:

```
RUN OUTCOME: repo=/projects/laya-rs/crates verdict=OK proven=18 suspect=51 new=0 rc=0
```

A baseline is an acknowledged list, not a fix: every entry still needs the decision in
the table below.

---

## What holds up (measured, not asserted)

* **Zero panic sites in production Rust.** All 142 A8 hits are in `_tmp/`. The
  workspace `unwrap_used = deny` / `panic = deny` / `indexing_slicing = deny` lints are
  genuinely holding — verified by stripping `#[cfg(test)]` and re-scanning.
* **148 tests / 5,130 LOC = 1 test per ~34 LOC.** Unusually high, and the tests are
  behavioural (fail-closed paths, near-miss controls), not smoke tests.
* **The readiness fix is real and complete.** `probe_head` is wired into `probe()`,
  `probe()` into `main`, and `main:43` interpolates the verdict — which keeps
  `Display for Readiness` live.
* `docs.old-<ts>/` was archived, not deleted — the no-destroy rule held.

---

## CRITICAL — wired into the build, unreachable in behaviour

### C1. `laya-head` — the temperature clamp — has no production caller

```
laya_head::   occurrences in crates/ : 0
laya-head     declared as a dep of laya-mcp at crates/laya-mcp/Cargo.toml:21
```

The 125-line crate exists for exactly one reason: blocker **B2** — the shipped config
temperature for `choice:11+` is `0.1006`, which, applied raw, multiplies logits ~10×
and publishes a **0.24 probability as 0.99**, defeating every downstream confidence
gate. `clamp_temperature` is called **only from its own tests**; `temperature_was_clamped`
has **zero** non-test references anywhere.

*Not a live bug today* — the live path is the Python sidecar and laya 0.3.5 clamps
correctly (confirmed verbatim by the runtime warning). But the Rust implementation is an
**unwired stub**, so `specs/03-rust-head.md` (P4) would land on a clamp nothing calls.
This is precisely the class the audit exists for: `pub` in a library, silent compiler,
green tests.

### C2. The queue's fail-closed guard is unreachable — depth is always 0

`crates/laya-mcp/src/lib.rs:802-814`:

```rust
if matches!(name.as_str(), "gate" | "decide")
    && let Ok(mut q) = state.queue.lock()
{
    if q.push(priority, name.clone()).is_err() { /* "queue is full; fail closed" */ }
    let _ = q.pop();          // <-- same lock scope, nothing between push and pop
}
```

Push and pop happen inside **one** critical section with nothing in between, so the
queue depth is always 0 outside the lock and `capacity` (256) is never reached. **The
"the work queue is full; a gate that cannot run must fail closed" branch cannot fire in
production**, and neither can any shed.

Worse, the only test (`a_full_gate_queue_refuses_rather_than_silently_dropping`,
`lib.rs:1293`) constructs `ServerState::new("http://x:1", 0)` and calls `q.push(...)`
**directly** — it drives the library, never `dispatch`. So spec 06's headline safety
property is both unreachable *and* untested on the live path. That is the
"test re-enacts a neighbour instead of the live artefact" pattern.

`QueueStats::sheds` has 0 production reads (tests: 1) — consistent with a queue that
never sheds.

### C3. `laya-store` (the redb ledger) is a 59-line stub declared as a dependency

Its own header says **`STATUS: NOT STARTED`**. Declared in `laya-mcp` and `laya-cli`;
`laya_store::` occurrences: **0**. `cargo-machete` flags 6 of its deps unused
(`blake3`, `laya-abi`, `laya-core`, `redb`, `serde`, `serde_json`, `thiserror`).
`specs/04-mcp-server-and-ledger.md` promises a ledger; the `ledger` MCP tool returns
`not implemented`. Consistent with the docs — but it is 59 lines of contract comment
with no code, carried as a build dependency.

---

## HIGH

### H1. `server_capabilities()` is dead — CORRECTED, see the closure section

`lib.rs:829` built `ServerCapabilities` with `.enable_tools()` and was never called;
`impl ServerHandler for LayaMcpServer {}` has an empty body.

**My conclusion from this — "the intent does not hold, rmcp's defaults are advertised" —
was WRONG.** `#[tool_handler]` auto-generates `get_info()` when the impl does not define
it (rmcp-macros `tool_handler.rs:97-106`), and the macro builds
`ServerCapabilities::builder().enable_tools().build()` — byte-for-byte what the helper
built. The capabilities were always tools-only and correct.

The real defect is narrower: a **second, unread source of truth**. A reader would
reasonably believe editing that helper changed what the server advertises. It would not.
Fixed by deleting it and writing down where the truth actually lives, rather than by
wiring it in — wiring it in would have recreated the duplication without changing
behaviour.

### H2. `temperature_was_clamped` — the B2 *signal* also has no consumer

Even with the clamp applied, nothing reads "this bucket was clamped". The B2 remedy was
two-part: apply the clamp **and** surface which buckets are uncalibrated. The second
half is unwired.

### H3. `temp_tag` (`laya-abi:51`) — 0 references

Built to construct Laya's `temperature_by_options` keys (`choice:2`, `choice:3-5`,
`noul:2`). The doc comment even cites the verified key shapes. Nothing calls it.

### H4. `with_criteria` (`laya-abi:128`) — 0 references

The builder for a question's optional criteria block. The MCP adapter builds the
criteria object inline instead, so the typed path is bypassed.

### H5. The rename left a stale prefix in the shipped binary

`crates/laya-mcp/src/main.rs:44` still prints `laya-rs:` — the exact defect the
rename set out to remove, and which I reported as fixed. My rename script replaced only
the prefixes I enumerated and missed this one. Consequence: the *first* line a client
sees when the server refuses to start still names a binary that no longer exists.

### H6. `dispatch` is not the function rmcp calls

`lib.rs:747` `dispatch` exists solely to type-assert `dispatch_shim`, which is what
`new_dyn` actually receives. The doc comment says it exists "so the shape rmcp requires
is declared once and this implementation cannot silently drift from it" — but a
`let _: DispatchFn = dispatch_shim;` inside an uncalled function cannot catch drift at
the call site. The intent is not achieved; `dispatch_shim` is what must match.

---

## MEDIUM — hygiene and claim-vs-reality

* **Not a git repo.** 5,130 LOC, 7 specs, 20 docs, zero commits. No diff against spec,
  no rollback, no bisect.
* **15 unused dependencies** (cargo-machete, hand-verified). Corrected one of my own
  suspicions: `ureq` in `laya-backbone` genuinely is unused — `ureq::` appears only in
  `laya-mcp` and `laya-cli`. My first probe matched doc comments, which is why
  machete is worth having.
* **A5**: `crates/laya-core/examples/payload_demo.rs` — functions, no assertion in the
  file. Expected for an example, but it sits in a test path and needs an explicit
  decision rather than a silent pass.
* **`_tmp/` holds 68 files** (48 `.py`, 16 `.sh`) — and these are not passive scratch:
  the `fix-*.py` / `wire-*.py` scripts *edited the source*. Part of the tree's history
  is encoded in throwaway scripts, including a doc-comment mangling that shipped and
  was only caught by a test. Archive or discard deliberately.
* **`.venv-laya` (5.4 GB) lives inside the project** and is what the audit scanned as
  Python (the A11 `activate_this.py` hits are venv-generated).
* **`docs.old-1790724524/`** retained correctly, but nothing points at it.
* **`laya doctor` reads `LAYA_SIDECAR_SOCKET` at call time; `laya-mcp` at startup.**
  Same default, so no drift today — but a relocated socket can diverge, and a health
  check must probe what the server actually uses.

---

## Ranked fixes

| # | Fix | Effort | Why this order |
|---|---|---|---|
| 1 | `main.rs:44` `laya-rs:` → `laya-mcp:` | 1 min | A defect I reported as fixed, still shipped |
| 2 | Delete or wire `laya-head` | 15 min | Unwired safety mechanism; blocks P4 landing correctly |
| 3 | Fix the queue: remove the no-op guard, or make it real | 1–2 h | A guard that cannot fire reads as protection it does not provide |
| 4 | Wire `server_capabilities()` into `impl ServerHandler` | 15 min | The declared intent is not in effect |
| 5 | `cargo machete` + remove the unused deps (**26** per the `crates/` run) | 30 min | Cheap; shrinks the build and the claim surface |
| 6 | Remove `laya-store` from the build until it has code | 10 min | 59 lines of comment carried as a dependency |
| 7 | `git init` + first commit | 5 min | Everything above is unreviewable as a diff without it |
| 8 | Decide on `temp_tag` / `with_criteria` / `stats` / `capacity`: wire or delete | 30 min | Unwired API surface |
| 9 | Archive `_tmp/` scripts into `_docs` or discard | 30 min | Source-editing history living in scratch — and it corrupted this audit's first pass |
| 10 | Baseline the audit (`--update-baseline`) and **re-run it on `crates/`**, not the repo root | 10 min | The root run skipped A3 and let scratch files pose as references |

**Items 1–4 are the ones that matter**: each is a mechanism that reads as present and
load-bearing while contributing nothing to behaviour. That is the same class as the
`doctor` defect fixed earlier — *a check that passes while the live path is broken*.

---

## CLOSURE — the fixes, applied and verified (2026-09-30)

The review's ranked list was worked top-down. Every claim below is from a run, not an
intention.

| # | Item | Outcome |
|---|---|---|
| 1 | `main.rs:44` stale `laya-rs:` prefix | **Fixed** — now `laya-mcp:` |
| 2 | `laya-head` unwired | **Dep is removed, crate kept.** It is P4 work (spec 03); the manifest now says so, so a shell cannot read as an implementation |
| 3 | The unreachable queue guard | **Removed, with the root cause named** (below) |
| 4 | `server_capabilities()` | **Removed, and my H1 conclusion corrected** (above) |
| 5 | 26 unused deps | **0 remaining** — `cargo machete` reports none |
| 6 | `laya-store` in the builds | **Removed from both binaries**; crate kept with a `STATUS: NOT STARTED` manifest note |
| 7 | Not a git repo | **Committed** — `4ad7dd2`, 76 files, 1.1 MB, nothing heavy leaked |
| 8 | `temp_tag` / `with_criteria` / `stats` / `capacity` | **Baselined with reasons** (below) — these are not defects |
| 9 | `_tmp/` holding source-editing scripts | **Split and archived** (below) |
| 10 | Re-baseline the audit | **`proven` 18 → 10**; the 8 that remain are accounted for |

### Item 10 — the audit's PROVEN count, before and after

```
before:  proven=18  (11 A1 + 7 A3)
after:   proven=10  (10 A1 + 0 A3)
```

The 8 that went away: `server_capabilities` (deleted) and the 7 A3 findings (deps
removed). Nothing was suppressed or baselined away to make the number move.

**The 10 that remain, each with its reason — none is a defect:**

| Finding | Why it is not a defect |
|---|---|
| `main` × 2 (`laya-cli`, `laya-mcp`) | process entry points; no in-code caller by design |
| `fmt` × 2 (`queue.rs:83`, `readiness.rs:81`) | trait impls. `Display for Readiness` **is** reached — `main.rs:43` interpolates it, which A1 cannot see |
| `dispatch` (`laya-mcp:749`) | type-assertion helper for `dispatch_shim`, which is what rmcp binds |
| `temperature_was_clamped` (`laya-head:73`) | the B2 *signal*; its consumer is part of the unwritten P4 forward pass |
| `temp_tag` (`laya-abi:51`) | for building Laya's `temperature_by_options` keys; unwired until P4 |
| `with_criteria` (`laya-abi:128`) | typed builder the MCP adapter bypasses by building criteria inline |
| `stats` / `capacity` (`queue.rs`) | test-only readers of a component that is itself not wired |

Recorded rather than deleted because each is either unreachable-by-design (entry points,
trait dispatch) or belongs to P4. The ones that matter are the last five: they are the
**P4 surface**, and P4 will wire them or delete them. That is now explicit.

### Item 3 — the queue, and the root cause

The guard was removed, and so was everything feeding it: the `queue` field, the
`queue_capacity` constructor argument, the priority classification, the import, and the
test that drove the field directly.

**The root cause is architectural, not a wiring mistake:** this server serves ONE client
over stdio on a `current_thread` runtime, so dispatch is already serial and there is no
concurrent work to prioritise. The concurrency in this system lives in the backbone,
measured at **5.8× parallel with headroom** — so there is no throughput problem either.
A queue was solving a problem this process does not have.

`laya-core::queue` keeps its implementation and its 16 tests: it is a correct, tested
component (spec 06). What changed is that nothing claims the server uses it.

**A regression guard on an absence** now exists, and it was **proven to fire**:

```
injected `PriorityQueue` into production code
  -> FAILED: "`PriorityQueue` is back in production code — see the note in `dispatch`"
  -> restored byte-exact -> passes
```

An assertion that a thing is ABSENT is unusual, so it gets a near-miss control: the test
also pins that a queue cannot come back without someone reading the reason.

### Item 9 — the scratch directory, and a gate that was not a gate

68 files in `_tmp/`, and the split mattered:

* **`verify-all.sh` — the project's gate — lived in gitignored scratch**, referenced by no
  tracked file. A fresh clone could not verify anything. It is now `scripts/verify-all.sh`
  with 11 siblings, a `scripts/README.md` stating each one's role, and all live doc
  references repointed.
* **51 one-shot patch scripts** (`fix-*`, `wire-*`, `add-*`) **edited the source in
  place**. They are archived to `_docs/99-archive/one-shot-scripts-2026-09-30/` with a
  README saying *do not re-run these* — re-running an applied patch fails or
  double-applies, and a script that looks runnable and is not is a trap. One of them is
  kept as evidence: `fix-final-and-tests.py` is the script whose string replacement
  mangled a Rust doc comment that shipped.

**The move immediately caught its own class of bug.** After relocating the scripts, the
gate still printed `PASS` — but the `link check:` line had vanished, because
`verify-all.sh` still called `_tmp/regen-index.py`, which no longer existed. The script
now uses `$(dirname "$0")`, and the link check is back:

```
=== gate: PASS (all 3 steps) ===
TOTAL PASSING (whole workspace): 148
count: 0
fmt: CLEAN
link check: 54 relative links checked, 0 broken
```

**A check that disappears without a word is the same failure family as everything else in
this report.** It is worth noting that it was caught only because the *output* changed
shape, not because anything failed.

### The A3 ambiguity, now demonstrated in BOTH directions

Earlier in this review I flagged that a check missing from `by_check` is
indistinguishable from a check that found nothing. Both cases have now been observed on
this same tree:

| Observation | Was A3 skipped, or clean? |
|---|---|
| repo-root run: 15 unused deps provably existed, `A3` absent from `by_check` | **SKIPPED** — it reported nothing while machete found 15 |
| `crates/` run at 11 findings vs 18 across re-runs | **SKIPPED** — 7 findings vanished silently |
| `crates/` run now, `A3` absent, machete says 0 unused | **CLEAN** — verified independently |

Same output shape, opposite meanings. The mitigation stands: **cross-check the `proven`
total against the known baseline**, and treat a missing A3 as not-run until an
independent tool agrees.

### Final verified state

```
cargo xtask gate / scripts/verify-all.sh   PASS (fmt + clippy -D warnings + test)
tests, whole workspace                     148
clippy --workspace                         0 warnings
cargo fmt --all --check                    clean
doc links                                  54 checked, 0 broken
cargo machete                              0 unused dependencies
audit (crates/)                            proven=10, all accounted for above
git                                        4ad7dd2, 76 files, 1.1 MB
laya doctor                                READY (backbone + head)
```

### What is still open — not fixed, and not hidden

* **P4 is unwritten** (the Rust forward pass), and with it the consumers of
  `temperature_was_clamped`, `temp_tag`, `with_criteria`.
* **5 of 7 MCP tools** return `not implemented` rather than pretending.
* **No ledger** — `laya-store` is `STATUS: NOT STARTED`.
* **No near-tie guard.** The payload should carry the top1−top2 margin before P4, because
  the measured Δp = 0.0006 between backends is enough to flip a near-tie.
* **The MCP client session is not supervised** — a client holding a pre-rebuild binary
  keeps its old behaviour until it restarts.
