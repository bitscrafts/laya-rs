#!/usr/bin/env python3
"""Close the review: record the fixes, CORRECT the one wrong finding, update HANDOFF."""
import pathlib
import re

root = pathlib.Path("/projects/laya-rs")
review = root / "_docs/04-development/CODE-REVIEW-2026-09-30.md"
t = review.read_text()

# ---- correct H1 in place, rather than quietly deleting it ------------------
t = t.replace(
    """### H1. `server_capabilities()` is dead — and the capability declaration is not in effect

`lib.rs:829` builds `ServerCapabilities` with `.enable_tools()` and a doc comment
explaining that claiming more "would be a lie a client would act on". But:

```rust
#[tool_handler(router = Self::tool_router())]
impl ServerHandler for LayaMcpServer {}      // lib.rs:822 — EMPTY body
```

The trait impl is empty, so rmcp's **default** `get_info` supplies the capabilities.
The function that encodes the intent is never called, so the intent does not hold: the
advertised capabilities are rmcp's defaults, not this deliberate tools-only set.""",
    """### H1. `server_capabilities()` is dead — CORRECTED, see the closure section

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
behaviour.""",
)

# ---- append the closure section -------------------------------------------
t += """
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
"""

review.write_text(t)
print("review doc: H1 corrected + closure appended")

# ---- HANDOFF ---------------------------------------------------------------
h = root / "HANDOFF.md"
ht = h.read_text()
ht = re.sub(r"\*\*Date\*\*: 2026-09-30 · \*\*Status\*\*: GREEN[^\n]*\n[^\n]*\n[^\n]*",
            "**Date**: 2026-09-30 · **Status**: GREEN — reviewed, fixed, committed.\n"
            "The audit is baselined (`proven=10`, all accounted for in the review doc), so a\n"
            "NEW finding now fails the gate.", ht, count=1)

block = """
## Code review + fixes (2026-09-30)

`_docs/04-development/CODE-REVIEW-2026-09-30.md` — 10 ranked items, worked top-down.
Its CORRECTION and CLOSURE sections matter as much as the findings: **one finding (H1)
was wrong and is corrected in place**, and the two claims below were each wrong on first
telling.

**Three critical defects, all "wired into the build, inert in behaviour":**

1. **The queue's fail-closed guard was unreachable.** `push`/`pop` sat in one lock scope,
   so the queue could never fill and *"the work queue is full; a gate that cannot run must
   fail closed"* could never fire. Root cause is architectural: this server is serial (one
   client, current-thread runtime), so it has nothing to prioritise. Queue removed from
   the server; `laya-core::queue` keeps its implementation and 16 tests. A regression
   guard on the absence was written and **proven to fire** by injecting the symbol.
2. **`laya-head` had no production caller** — the B2 temperature clamp, called only from
   its own tests. Dep removed; the crate is P4 (spec 03) and its manifest now says so.
3. **`main.rs` printed `laya-rs:`** — a stale prefix from before the binary rename, which
   I had reported as fixed. It was not.

**Also fixed:** `server_capabilities()` deleted (it duplicated what `#[tool_handler]`
generates — my first conclusion that the capabilities were wrong was wrong);
**26 → 0 unused dependencies**; `laya-store` out of both binaries; **`verify-all.sh` moved
out of gitignored `_tmp/` into `scripts/`** (the project's gate was in scratch, so a fresh
clone could not verify anything); 51 one-shot patch scripts archived with a do-not-re-run
notice; **`git init` + first commit `4ad7dd2`** (previously nothing was reviewable as a
diff).

**Two methods notes worth carrying forward:**
* Audit `crates/`, **not the repo root**: the root run silently skipped A3 and let `_tmp`
  scratch files act as references, suppressing real findings.
* A check missing from `by_check` is indistinguishable from a check that found nothing.
  Both cases were observed on this tree. Cross-check `proven` against the baseline.

## Next (ranked, unchanged)

1. `ask_file` / `ask_files` — highest value; everything needed now exists.
2. The **near-tie margin** in the payload (top1−top2), before P4: the measured
   Δp = 0.0006 between backends can flip a near-tie, so it must escalate, not decide.
3. `should_compact`, then `ledger` (needs an outcome path first).
4. **P4** — the Rust head, which also wires `temperature_was_clamped` / `temp_tag` /
   `with_criteria` or deletes them.
"""
marker = "## Next (ranked)"
if marker in ht:
    ht = ht.replace(marker, block.strip() + "\n\n## Next (ranked, superseded by the section above)\n", 1)
else:
    ht = ht.rstrip() + "\n" + block
h.write_text(ht)
print("HANDOFF.md updated")
