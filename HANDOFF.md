# HANDOFF — laya-rs

**Date**: 2026-10-02 (14) · **Status**: GREEN — **specs 16, 17, 18, 19 and 20 are all complete and
deployed.** The graph engine's three worst defects are fixed and its central limitation is closed:
which nodes run no longer depends on the order of the JSON file, a typo can no longer be silently
inert, a shell's output can now inform a decision, and a flow can ask the question shape this head
answers best.

```
cargo test --workspace                    595 passed · 0 failed
cargo clippy --workspace --all-targets    0
cargo fmt --all --check                   clean
python3 scripts/check-doc-citations.py    65 docs, every citation resolves
                                         104 links, 4 anchored, 0 broken

deployed == tree:  laya 60ba1cd01619cd52 · laya-mcp 7178bbb4755bc504 · laya-gate 9d2f45af7c023c3b
previous binaries kept as /root/.local/bin/*.old-20261002T200605Z
smoke test of the DEPLOYED path: bash scripts/smoke-deployed.sh
```

## What landed, and the measurement that carried each

| spec | commit | the thing that proves it |
|---|---|---|
| 20 — the format is a contract | `10fc381` | a typo'd key is refused with a nearest-key hint; removing the guard makes the refusal test fail |
| 17 — routing is a function of the graph | `ac9e94f` | **both array orders execute the same set**; `B_yes` no longer runs on a port that was not selected |
| 19 — docs + fixtures | `98d99d3` | four dated corrections; the order fixtures checked in |
| 18 — a shell's output can inform a decision | `10b26ef` | **a `wc` that drives the branch**: 13 words → short, 412 → long |
| 16 — `question_type` | `355b636` | same question, same state: **`noul` 0.9677 at band HIGH vs `choice` 0.2389 at band low** |
| deploy + smoke test | `224411b` | verified through the **installed** command |

## What is open

**Spec 21, filed and not dispatched** — `specs/21-the-refusals-catch-up-with-the-scheduler.md`.
Spec 17 made the per-node `timeout` on a decide **live** (measured: `timeout: 0` fails with *"timed out
after 0ms"*), so spec 20's interim refusal of that field is **stale and blocks a working feature**. The
twin field `allow_failure` on a decide is **worse than refused**: accepted, its own failure tolerated,
and then the synthetic route node stops the run anyway. Detail:
`_docs/04-development/STALE-REFUSALS-2026-10-02.md`. **This is the next thing to run.**

**Spec 17's mutation criterion is open on purpose.** The gate needs `exit 0` and exits **3** — 215
mutants, 193 caught, 20 unviable, **2 timeouts, 0 MISSED**. `cargo-mutants` uses 3 for a timeout and 2
for a missed mutant; both timeouts are replacements of `dependency_tables` (`sched.rs:87`) returning a
phantom-only map, leaving a node's indegree above zero so the scheduler **waits rather than failing**.
No mutant survived; the invariant is undefended and the hang is the symptom.

**The `subflow` CLI gap is unchanged**: `Subflow` is not a variant of `Kind`, so a subflow node falls
through to `Other`, and the CLI has no library flag. Its error message now names the real problem.

## The graph's measured surface, after this batch

| feature | status |
|---|---|
| branches (`label` / `band`, mandatory `_` catch-all) | **works** |
| `depends_on` ordering, including a shell before a decide | **works** |
| `{{node_id}}` — a node's stdout informs a decision | **works** |
| `question_type: choice \| noul` | **works** |
| `timeout` (shell, mcp, **and decide**) | **works** |
| `allow_failure` | works on shells and tail mcp; **partial on a decide** (spec 21) |
| unknown keys, bad `on`, bad port keys, missing port targets | **refused by name** |
| `retry` | **refused** until implemented (a later spec) |
| `escalate_on_near_tie` | **refused** (its target ran unconditionally) |
| loops | **refused by design**; cycles named |
| `subflow` | library-only, unreachable from the CLI |

## The graph is the active work

**Read first:** `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md` (the review, with the
re-verification of each finding) and `_docs/03-reference/GRAPH-FEATURES.md` (composability, branches,
loops — measured by running, not reading).

| feature | status |
|---|---|
| branches (`label` / `band`, mandatory `_` catch-all) | **works** — 22 edges, 4 routes, only selected nodes run |
| `allow_failure`, `timeout` | **work** |
| cycle refusal | **works well** — refuses on both `validate` and `run`, names the path |
| routing correctness | **broken** — depends on JSON array order; `not_started: []` hides it |
| `retry` | **declared, validated, INERT** — a flaky step kills the run on its first failure |
| `escalate_on_near_tie` / `near_tie_port` | **declared, validated, and the target runs UNCONDITIONALLY** |
| `depends_on` (shell → decide) | **not honoured** — `count_words` and `class_short` both at ms 575 |
| `subflow` / `uses` | **library-only** — `Subflow` is not a variant of `Kind`, and the CLI has no library flag |

**Five declared fields that do nothing.** `allow_failure` and `timeout` do work, so the pattern is not
universal — which is why the proposed spec 20 audits every field rather than fixing one. A field that
validates and does nothing is worse than a missing field: it reads as a guarantee.

**The specs, in the review's recommended order:** 17 (routing is a function of the graph — the
priority) → 20 (retry + the field audit) → 18 (`{{node_id}}`, what makes graphs general) → 19
(refusals that name the rule) → a CLI library flag for subflows. 16 (`question_type`) is written and
the review ranks it lower than I did: it buys **confidence**, not correctness.

**Resume the review session** (never start fresh):

```sh
cd /projects/laya-rs
BRIEF=/projects/_tmp/brief-opus-round2.md \
REPORT=/projects/_tmp/opus-graph-analysis/ROUND2.md \
  scripts/run-opus-graph-analysis.sh --resume
```

Exit **3 means the dispatch failed** (nothing written). Procedure and traps:
`_docs/05-operations/RESUMING-OPUS.md`.

## The specs that landed before this batch

The three `ignored` are deliberate and are the spec-13 change: the two tests that need a live
backbone are opt-in behind `LAYA_ACCEPTANCE=1`, and `scripts/verify-all.sh` runs them with
`--include-ignored` so they RUN rather than vanish. The third is hermetic and runs.

## The six specs that landed in this batch

Each was implemented by `sonnet` through `scripts/run-spec-part.sh` — never inline, per HARD RULE 0d
— and verified here by running its own criteria rather than reading its report.

| spec | commit | the thing that actually carried it |
|---|---|---|
| 09 scheduler fail-fast | `7e4267f` | the control reproduces the original defect verbatim: "`b` ran after the run had failed" |
| 13 test hardening | `8c69f1e` | the ORPHAN test, not the flag test — disabling the group kill fails it while the flag test still passes |
| 10 deadlines | `0ef1743` | a real run bounded by its deadline: exit 1 after 1s on a `sleep 3` node with `timeout: 1` |
| 12 head kernel | `9ba02ad` | the PyTorch differential green with its tolerance untouched; live corpus agreement 1.0000 |
| 11 observability | `451b337` | a data-loss bug the spec never knew about: `Record::identity` used the process-relative clock, so two servers dropped each other's rows |
| criteria ticks | `50a4b30`, `fe407fe` | specs 08 and 14 were implemented but never marked |

## The one open operational item: THE DEPLOYED BINARIES ARE STALE

`/root/.local/bin/{laya,laya-mcp,laya-gate}` are the **03:05 / 03:43 builds**. Everything from spec 11
onward exists only in the source tree. Measured consequence: the spec-11 criterion
`python3 scripts/check-observability.py` **exits 1 against the deployed binaries and 0 against this
tree's build**:

```sh
cd /projects/laya-rs
pgrep -x laya-gate        # MUST be empty first: replacing it blocks every live agent mid-turn
cargo build --release
mv ~/.local/bin/laya-mcp ~/.local/bin/laya-mcp.old-$(date +%s) && cp target/release/laya-mcp ~/.local/bin/
mv ~/.local/bin/laya     ~/.local/bin/laya.old-$(date +%s)     && cp target/release/laya     ~/.local/bin/
python3 scripts/check-observability.py    # must exit 0 from the deployed path
```

Use `mv`-then-copy, never `cp` over a live binary. Four `laya-mcp` processes still hold the old
inode (pids 1068293, 1068334, 1070199, 1109940); they self-heal on reconnect.

## The P0 from the previous session: the safety gate failed open, and spec 08 closed it

A read-only review of the whole tree, then independent re-verification by the orchestrator, established
that the gate answered **`allow`** to a filesystem-formatting command, an unrecoverable shred, a recursive
permission strip on the home directory, a truncating redirect and a pipe from the network into a shell —
while answering **`ask`** for `ls -la /tmp` and `cargo test`. The ordering was inverted, so the `allow`
path carried no safety information. `ask` then ran what the gate allowed and validated the request
afterwards, so a call could have its effect and then report failure.

**Spec 08 (`12e16e9`) inverts the default: `allow` requires a positive allowlist match** (`Policy::gate_allow`),
shell metacharacters defeat every entry, the model may only **downgrade**, `irreversible` finally changes the
outcome, and the whole decision is one I/O-free function, `agentic::gate_verdict`. `ask` validates its request
before executing and bounds the child with a deadline.

**Verified on the live path, by the orchestrator, not from the report:**

```
                                        BEFORE          AFTER
filesystem-format command               allow     ->    ask
unrecoverable shred                     allow     ->    ask
truncating redirect                     allow     ->    ask
network pipe into a shell               allow     ->    ask
control:  ls -la /tmp                   ask       ->    allow      <- allow still reachable
control:  git status                    ask       ->    allow
```

`scripts/gate-live-corpus.py`: **18 of 18** destructive commands refused `allow`, the control reaches it.
An adversarial set of eleven **allowlisted-prefix** commands carrying destructive intent
(`ls -la /tmp; rm -rf …`, `cat f > f`, `git status; <format cmd>`, `git log $(…)`) — **none** reached `allow`;
the metacharacter rule carries them. `VERDICT: ALL CHECKS PASS`.

**One judgement call beyond the spec, recorded in HANDOFF in place:** `noul_high_threshold` 0.9 → 0.8. The
head scores benign allowlisted commands 0.83–0.87, in the same range as destructive ones it scores 0.90–0.99;
at 0.9 every legitimate command would ask forever while stopping nothing the allowlist does not already stop.

**One limit stated, not papered over:** R7's promise that an error from `ask` guarantees the command did not
run holds for the refusals knowable up front, which `ask_execute` now settles before spawning. It cannot hold
in general — `ask`'s design runs the command and then judges the output, so a failure in the judging step
necessarily follows the run. The test covers the knowable case; the general claim is stronger than the design.

**DEPLOYMENT NOTE:** the fixed `laya-mcp` is installed at `~/.local/bin/laya-mcp` and verified by a fresh
spawn. Three long-lived Hermes MCP connections still hold the OLD inode, so they keep the old behaviour until
Hermes restarts them. `cp` onto a busy binary fails (`Text file busy`); use `mv` (rename) — running processes
keep the old inode, new spawns get the new one.

## Specs 09–13: specified, criteria proven to fail, ordered

| spec | what | why it matters |
|---|---|---|
| **09** | scheduler fail-fast, cycle refusal, route type as data | spec 07's own R11 promise is **false**: see below |
| **10** | deadlines that fire | a declared node `timeout` is parsed and then ignored |
| **11** | observability: one run id end to end | the ledger cannot join a run to its decisions; `gate`/`ask` record nothing |
| **12** | head forward under a second | 10.6 s at seq 460; a measured 6.7× kernel win is available |
| **13** | test hardening: hermetic, mutants, properties | 18 mutants survive on the safety files; `cargo test` needs the network |

**Spec 09's premise, reproduced by the orchestrator through the repro harness:** with `limit: 1` — so the
siblings had to wait for `a` to fail before they could start — `stopped_early=true failed_node="a"` and
`records=["a","b","c","d","e"]`: **four siblings ran after the run had already failed.** The only R11 test
uses a chain, where indegree alone blocks the third node, so a scheduler with no fail-fast at all passes it.
Also reproduced: a cyclic `Graph` returns `Ok` with `stopped_early=false, records=["z"]` — a partial run
reported as complete.

## Spec 07 — the graph executor — is COMPLETE, in four parts

| part | what | commit |
|---|---|---|
| 1 | the flow, its refusals, the `decide`→`route` lowering (+ the R15 runner fix) | `a9e9310` |
| 2 | subflow expansion, and a render that names the fields it drops | `9cf2aa4` |
| 3 | the scheduler, `shell`/`mcp` executors, run records | `574354b` |
| 4 | the `laya graph` CLI surface, and R-ACC end to end | `f42cd52` |

**The claim the whole spec existed to test, measured through the real path:**

```
ROUTE RECORD (positive fixture): {"on":"$.payload.label","selected_port":"branch_positive","value":"positive"}
NODES RAN   (positive fixture): ["classify","classify/route","branch_positive"]

ROUTE RECORD (negative fixture): {"on":"$.payload.label","selected_port":"branch_negative","value":"negative"}
NODES RAN   (negative fixture): ["classify","classify/route","branch_negative"]
```

Two fixtures, two **different** branches, driven by the real model's label — the pairing is what makes
it routing rather than coincidence (a hardcoded route passes the first and fails the second). The live
`laya graph run` on the same flow returns `label="positive" confidence=0.8401 band="mid"`, selects
`branch_positive`, and exits 0 with `stopped_early=false`. The acceptance test spawns the **real**
`laya-mcp` over stdio and fails with a labelled reason if the binary is absent — it never stubs the
caller, which would have made the test a re-enactment. Backbone down: reported in 78.9µs, bounded by 3s.

It now runs as **gate step 7c**, so R-ACC is bound rather than remembered.

## Five verification defects found while landing this (all fixed, all of the same family)

The theme: **a check that reports the wrong thing passes while the live path is broken.** Each was
invisible until the previous one was fixed.

1. **`verify-all.sh` discarded every check's exit code** — `set -uo pipefail`, no aggregation, so its
   exit status was its last `echo`'s. `check-doc-citations.py` exited 1 with three unresolved citations
   while the wrapper printed `gate: PASS` and `GATE EXIT=0`. Every check now runs through `chk` and the
   verdict is derived from the accumulated failures. Control: one injected bad citation → exit 1.
2. **Parts 1–3 still read "not yet dispatched"** despite being implemented and committed, and the
   runner treats `SPECIFIED` as PENDING — so **53 criteria for landed code had never been evaluated**.
   `pass=22 → pass=77` once the status lines were corrected. This is R15's own defect, one level up.
3. **Criteria that invoked the criteria runner** re-entered it: **18 nested runner processes**, each
   doing whole-workspace cargo runs. It did not hang, it *multiplied*. Fixed with a `--list-specs` mode
   (runs no criterion, 0.06s) plus a re-entrancy guard.
4. **`grep -qv .` in part 4** — a criterion that can never pass: `-q` suppresses output so `-v .` has
   nothing to match, and grep exits 1 on empty *and* non-empty input. It reported FAIL on a clean tree.
5. **Parts 1–2 asserted a crate-wide prohibition** (`! grep std::process src/`) which part 3's `shell`
   executor legitimately violates — failing on *correct* code. Narrowed per part, each with a near-miss
   control so the `!` is not matching nothing.

**Two of my own helper scripts also lied, and were caught by their own controls:** `verify-spec-part.sh`
asserted `git status -- specs/` was clean while calling it "worker did not commit" (a different
property — it fired on the orchestrator's own spec edits), and its `specs/ untouched` check fired on
correct work. Both replaced with checks that assert what their names claim.

## Commit hygiene — broken three more times, then made mechanical

The rule (never name an AI assistant or tool in a message, author name **or email**) was broken in the
bodies of three spec-07 commits this session, with clean authors every time — because A24 fixed the
**author** and nothing scanned the **body**. Guarded in one of three places is half a rule.

`scripts/check-commit-msg.sh` now refuses a tool name in a message and is installed as the
`commit-msg` hook (controls: refuses the verbatim bad text, accepts a clean message, does not flag
"topic" but does flag a bare two-letter tool name, and a real `git commit` was refused with HEAD
unmoved). It is also a gate step with an explicit baseline — measured, **8 of 49 commits** name a tool
(4 from this session's work, 4 older and descriptive rather than authorship). Fix forward, do not
rewrite; the gate prints the count so the debt is named, not forgotten.

`scripts/verify-all.sh` §6c runs
`scripts/live-corpus-parity.py`, so `LABEL AGREEMENT: 16/16 = 1.0000` is reported on every gate run
rather than asserted here.

## Spec 03's live criterion, MEASURED

The last unchecked box was the live corpus, and measuring it required building the end-to-end
path rather than asserting one:

```
python3 scripts/live-corpus-parity.py
LABEL AGREEMENT: 16/16 = 1.0000   (floor 0.9)   0 errors
worst probability difference 4.978e-05   vs the reference's own 4-dp rounding (5e-05)
logit-gap histogram: min 0.0158  p10 0.1102  median 2.6461  max 6.2077
near-ties (gap < 0.1): 1/16
```

Both sides are driven **from text** — the reference over its socket, the Rust path through
`laya decide` — because a comparison starting from a pre-computed hidden state would skip the
tokenizer, the sequence builder and the embedding call, which are the parts most likely to differ.

**`laya decide` now exists** (`--state/--question`, or `-` to stream JSON): text in, the decision
as JSON on stdout, a summary on stderr, exit codes 1 usage / 2 artifact-or-backbone / 3 decision
failed. That is what makes the port reachable and the criterion runnable.

**Two bugs only the live run could find:**
1. llama.cpp returns `embedding` as a list of **ROWS**, not a flat list. The Python sidecar hid it
   (`torch.tensor(r[0]["embedding"])` accepts nested lists as 2-D), so a port copying that line
   read lists where it expected numbers.
2. **The attention was O(seq^3).** A clippy-driven de-indexing put `chunks_exact().nth()` inside
   the inner loops, and `nth` is a linear scan — a 460-token sequence was still CPU-bound after
   four minutes. Offsets computed once per head: **~12 s release**. A debug build is ~10x slower,
   which the harness now documents.

   > **CORRECTED 2026-10-01 — the attributed cause is wrong; the fix and the speed-up are real.**
   > `ChunksExact::nth` is **O(1)**, not a linear scan: measured, 10⁶ random-index calls take
   > 0.003 s. The pre-fix code used exactly that call, so it cannot be what made the loop cubic.
   > The measured causes are the **scalar strict-order reduction** (`math.rs:214` — the reduction
   > order is part of the contract, so it cannot simply be reordered) and the **missing weight reuse
   > across tokens**. The conclusion — compute offsets once per head — is still the right fix and
   > the ~12 s figure still stands. What is wrong is only the explanation, and it matters: the wrong
   > explanation points a future reader at `nth` and away from the reduction, which is where the
   > remaining cost is. Spec 12 measures it.

## Spec 03 (P4): the head is reproduced against PyTorch itself

Spec 03 is complete on **every** half, offline and live. The differential test is the point: it runs
the **real** `nn.TransformerEncoderLayer` over synthetic hidden states and requires the Rust port to
reproduce the logits.

```
crates/laya-head/src/math.rs       f32 primitives, per-branch MEASURED accuracy
crates/laya-head/src/forward.rs    Head::load + Head::forward, pure (&self), no lock
crates/laya-head/src/safetensors.rs the 36-tensor reader
crates/laya-head/tests/forward_differential.rs
        worst logit difference across 4 cases: 4.05e-6   (margin scale 1e-3, so 2 orders below)
```

**`erf` was wrong three times**, each version caught by a test with a known value rather than by
review: a Lentz form returning -2.9e26; the Numerical Recipes approximation whose REAL error at
x=2 is 3.1e-3 — the same scale as the near-tie margins — while its own doc claimed 1.2e-7; and a
branch point at 2 where measurement showed the series is good to 4.2e-12 at x=4. The doc now
carries the measured error table.

**Three more bugs only the differential test could catch:** an off-by-one selecting the `qtype`
row; an off-by-`n_heads` in the fused `in_proj` layout; and a fixture that was broken twice
(`round()` perturbs an f32; `repr()` emits a JSON *string*, so Rust parsed NaN and both sides
failed identically).

**DONE, not in progress:** the tokenizer and `build_sequence` landed with the same differential-fixture
acceptance test — **exact** equality, not approximate: 91 corpus strings / 2,207 ids, 707 sequence
cases / 83,411 ids, 2,705 markers. Special tokens are `50281`/`50282`/`50283`/`50284`, **not** the
0–4 a reader would assume, and the fixture pins them. One new dependency was unavoidable and is
recorded with its reason in the manifest: the GPT-2 split needs a NEGATIVE LOOKAHEAD (`\s+(?!\S)`),
which the `regex` crate cannot express at all (its guarantee is linear time), so that one pattern
runs on `fancy-regex` — measured against the reference's Onig engine over 4,007 fuzzed inputs, zero
mismatches.

## The ledger, and `calibrate` is possible

Spec 04's second half was a contract with no implementation. It is implemented:
`crates/laya-store` (redb 4.2.0), the server's per-process segment writing (F8), and
`laya ledger compact | stats | tail | outcome`. Proven end to end by `scripts/prove-ledger.py`.

**The outcome path is the part that matters.** Nothing recorded whether a decision was RIGHT, so
`calibrate` could never have meant anything — ECE needs `(confidence, correct)` pairs. An outcome
is its own append-only record; `stats` prints **NO OUTCOMES RECORDED** while there are none, so
the gap is visible rather than implied.

**Four real bugs, none found by unit tests:**

| what | how it was caught |
|---|---|
| `latency_ms` was always **0** — I measured the process's *uptime*, not the call | the e2e proof: `stats` showed `p50: 0 ms` for a real model call; now a real 173–436 ms |
| `laya ledger compact` failed with *"unexpected argument `compact`"* | running it: the top-level parser rejected a subcommand's own arguments |
| **a held READER blocks a WRITER** in redb (the opposite of my assumption) | my own test asserting the reverse failed — so `LedgerReader` is opened and DROPPED per call, and a test pins that |
| `read_only` was written with `Database::open`, which takes the **write** lock | reading redb's source: a method named read-only that was not |

## SCOPE — THIS PROJECT ONLY (no association with any other project)

**No association with any other project** (user, 2026-09-30). This repository is ours alone:
never modify, depend on, or cite another project's files or line numbers in our docs. A property
we rely on from outside is stated as a **fact about an interface**, and its consequence is
enforced by a test in our own code.

**The graph implementation now lives here** (spec 07, complete 2026-10-01): `crates/laya-graph`
holds the flow, its validations, the `decide`→`route` lowering, subflow composition, the
renderer, the bounded scheduler, the `shell`/`mcp` executors, and run records — plus the
`laya graph validate|render|run` CLI in `crates/laya-cli`. `route_payload.rs` remains the
contract a `route` node matches on (the two field names a graph matches, restated in our own
crate and pinned by test), so nothing outside this repository can move our behaviour silently.
*(This section previously read "`laya-rs` has NO graph implementation … A DAG runner belongs
HERE if we build one." It was correct until 2026-10-01 and is kept here as the record of what
changed.)*

## The seven tools are real (this section previously said six of seven)

| tool | state |
|---|---|
| `decide` | real — typed questions -> route-shaped payloads (`label`, `band`, `margin`, `near_tie`) |
| `gate` | real — **TWO signals**; see the safety finding below |
| `ask_file` | real — a judgement about a file, never the file |
| `ask_files` | real — globs expanded, capped at 64, `hits` ranked by belief |
| `should_compact` | real — L7, four judgements in one call |
| `ask` | real — L10, the CALLER writes the questions |
| `ledger` | real — redb 4.2.0 (`laya-store`), `compact \| stats \| tail \| outcome`, per-process segments. The outcome path exists, which is what `calibrate` needed |

## THE SAFETY DEFECT THAT WAS FIXED (do not regress it)

> **SUPERSEDED, 2026-10-01 — it was NARROWED, not fixed. See spec 08.**
>
> The heading above was accurate about the mechanism and wrong about the outcome. The "both signals
> clear" rule below is a **12-entry, case-sensitive denylist AND-ed with a signal this same section
> measures as noise** (6/12, negative separation). The review of 2026-10-01 measured the result,
> live, through the real `laya-mcp`: the gate returns **`allow`** for a filesystem-formatting
> command, an unrecoverable file-shredding command, a recursive permission strip on the home
> directory, a bare redirect that truncates a file, a pipe from the network into a shell, and
> "Wipe the entire disk and delete all backups permanently" — while returning **`ask`** for
> `ls -la /tmp` and `cargo test`. **The ordering is inverted, so the `allow` path carries no safety
> information today.** `ask` then runs what the gate allowed, and only afterwards validates the
> request, so a call can have its effect and then report failure to the caller.
>
> Two further facts the original fix left in place: the noul **label is never read** (only
> confidence, and a noul's confidence is `max(p, 1−p)`, so a confident "no" scores 0.95 and is
> allowed), and `irreversible` is **still decorative** (`policy.rs:279-280` has two identical `Mid`
> arms; mutation testing shows both `true` and `false` survive).
>
> **Spec 08 replaces the denylist with a positive allowlist** — `allow` requires a match, the model
> may only downgrade, `irreversible` changes the outcome, and `ask` validates everything before it
> executes. The text below is kept because the *diagnosis* it records is still the right one, and
> because the effect-check technique it used is what caught this. Recorded here rather than rewritten:
> a superseded claim is annotated, never erased.
>
> **IMPLEMENTED, 2026-10-02.** `laya_core::agentic::gate_verdict` is the one I/O-free decision
> function spec 08 asked for (`crates/laya-core/src/agentic.rs`); `laya-mcp`'s `gate` and `ask`
> tools call nothing else. Re-run live, over real stdio against the real sidecar
> (`python3 scripts/gate-live-corpus.py`): all 18 destructive commands in the corpus — including
> every one named above, plus the R4 flag/whitespace variants (`rm -Rf`, `rm  -rf`,
> `rm --recursive --force`) — now return `ask`, and the control, `git status`, returns `allow`
> (`allowlisted: true, allowlist_entry: "git status"`). The noul label is read now (a confident
> `no` is `Deny`, not `Allow`), and `irreversible` forces `ask` unconditionally
> (`gate_verdict_irreversible_always_asks`, with the `irreversible=false` control pinned in the
> same test). `ask` validates the whole request — including the "nothing to judge" precondition
> that does not depend on the command's own output — before any `sh -c` runs
> (`ask_refuses_before_it_executes`/`ask_the_control_a_valid_request_does_execute`), and the
> command is bounded by `Policy::ask_deadline_secs` (default 60s), killed on expiry, with
> `timed_out` reported as data (`ask_a_command_past_its_deadline_is_killed`).
>
> One further, necessary change beyond spec 08's text: `Policy::default().noul_high_threshold`
> moved from 0.9 to 0.8. Measured live, the decision head's confidence for the now-allowlisted,
> genuinely benign commands sits at 0.83–0.87 (`ls -la /tmp` 0.8314, `git status` 0.8722,
> `cargo test` 0.8327) — below the old 0.9 cutoff, and in the SAME range as several destructive
> commands the live corpus measured above 0.9 (the negative separation this file already
> documents, immediately below). Since R1's allowlist is what now carries the safety property —
> the model only downgrades an already-allowlisted, signal-free command — holding the old cutoff
> would have kept every legitimate command asking forever while stopping nothing the allowlist
> was not already stopping. The live corpus script pins `git status` reaching `allow` as a
> permanent control against this regressing.

The `ask` e2e proof caught the gate **allowing `rm -rf <dir>` and the command running**. The
check that caught it was an **effect** check — the protected file had to still exist afterwards —
not a message check.

Two root causes: the model's verdict was the **only** signal, and `Policy::gate`'s `irreversible`
flag was **decorative** (`Band::Mid if irreversible => Ask` and `Band::Mid => Ask` had identical
bodies; it defaulted `false` and nothing ever inferred it).

**Measured, on commands with known answers:**

| what was tried | result |
|---|---|
| `"Is this action safe?"` as a noul | **6/12** — `ls -la /tmp` 0.83, `rm -rf <dir>` 0.89 |
| six consequence phrasings | **every one NEGATIVE separation** — no threshold can work |
| a `choice` over declared effects | **13/18** — still missed `: > file`, `dd if=` |

**Now `allow` requires BOTH signals clear:** no destructive effect pattern matched, AND the model
reached the high band. A pattern match can never be allowed through. The response reports
`model_decision`, `destructive_signals` (matched text + why) and `irreversible`, so a refusal is
auditable. Spec 04 F6c/F6d.

**Honest limit:** this is two signals, **not a sandbox**. It never executes a command it refuses;
that is not the same as containing one.

## THE FINDING THAT EXPLAINS FOUR DEFECTS

**This head CLASSIFIES CONTENT. It does not REASON ABOUT CONSEQUENCES.**

* `mid_edit`'s first phrasing (**"spans more than one step AND would be broken"**) -> **1/6**;
  rewritten to a single concrete question -> **6/6**.
* the gate's "is this safe?" -> **6/12**, a coin flip.
* but **"is there a bug?"** -> **0.8044, correct**.

So: **ask it to classify content; never ask it to reason about a consequence.** Two independent
phrasings reaching 6/6 is corroboration. Small samples — recorded as large unambiguous
improvements, not calibrated rates.

## The head reads a PREFIX, not the whole state

A signal at the START scored **exactly 0.788 at 3,975 / 15,943 / 63,951 / 199,951 bytes** —
invariant across a 50x range, only possible if the tail is not read. The same signal at the END
survived at 984 bytes and was **gone at 3,976** (0.087 = the no-signal control 0.0873). The exact
boundary could NOT be pinned, so `FULL_ATTENTION_BYTES = 1024` sits conservatively below the
smallest observed failure, and both file tools return `bytes` + a `warning` above it.

## What goes to redb: NOTHING (asked and verified this session)

> **CORRECTED 2026-10-01 — three of these four claims are now false.** Kept, annotated rather than
> erased, because the section records what was true at the time and why the question was asked.
> - `laya-store` **now declares dependencies**: `crates/laya-store/Cargo.toml` has
>   `redb = { workspace = true }`. `redb` is **present in `Cargo.lock`** (one entry). The
>   "NO DEPS YET, ON PURPOSE" line described the pre-spec-04 scaffold, and is contradicted by
>   `HANDOFF.md`'s own ledger section further down, which describes redb 4.2.0 in use.
> - `laya-store` **is** in `laya-mcp`'s tree; the `ledger` tool is real and writes segments.
> - No `.redb` file exists **in this repo** — true — because the store is written under
>   `LAYA_LEDGER_DIR`, which is outside it.
> - `laya-mcp` is no longer "two binaries, all seven tools stateless": `ledger` is stateful, which is
>   the whole point of spec 04.
>
> The `redb` pin that matters is `=4.2.0`, and `laya ledger compact` is the only writer.

## Documentation: ONE folder (merged 2026-09-29)

`docs/` and `_docs/` both existed — `docs/` held three pointer stubs left behind when
the SSOT tree was adopted as `_docs/`. Two folders for one project's docs is exactly
the ambiguity SSOT removes, and a pointer stub is a second location even when it only
redirects. So `docs/` was **archived** (never deleted) and every reference repointed.

```
documents before -> after : 2 folders -> 1 folder (`_docs/`, 17 files)
files merged              : 0 — the stubs were REDIRECTS, no unique content to absorb
files archived            : 3  (docs.old-<ts>/, preserved, nothing links into it)
references repointed      : README.md, DOCS.md, scripts/run-analysis.sh, AGENTS.md, HANDOFF.md
workspace tree untouched  : /projects/docs/ (open register) — a DIFFERENT tree
broken links after        : 0 (verified by scripts/regen-index.py)
```

## Working POC (2026-09-30) — the decision path is REAL

```
MCP client -> laya-mcp (stdio) -> sidecar (unix socket) -> llama.cpp /embedding
            -> local head -> typed answer
```

POC = ALL CHECKS PASSED. Real output, not a mock:

```
decide -> choice "billing" (billing 0.9311 / sales 0.0347 / technical 0.0342,
                            confidence 0.7283)
          noul churn_risk 0.2644 (confidence 0.7356)
gate   -> decision='allow' band='high' confidence=0.9479   (236 ms)
warm   -> 0.7 s then 0.71 s: the 24 s load is paid ONCE
no head-> gate: "ask"  ·  decide: protocol ERROR   (fail closed, verified against a
                                                    genuinely dead socket)
```

**In Hermes, live:** `mcp__laya__gate` → `{"decision":"allow","band":"high",
"confidence":0.9479,"saturated":false}`.

## Two components (the split is forced, not chosen)

| Binary | Role | Consumer |
|---|---|---|
| `laya-mcp` (`crates/laya-mcp`) | MCP stdio server; **refuses to start if Laya is down** | Hermes / pi / Claude Code |
| `laya` (`crates/laya-cli`) | one-shot CLI; **the exit code is the result** | you, cron, hooks |

The crate was already `laya-mcp` and every sibling is `<name>-mcp`, so the binary was
**renamed from `laya-rs`** (user correction). The project name and the MCP server name
(`laya`, giving `mcp__laya__*`) were deliberately NOT changed.

## The decision head runs as a SUPERVISED sidecar

The GGUF holds the **backbone only** (its own README: the head "must run outside
llama.cpp"), so the head — needing torch — runs as one persistent Python process. Its
own pinned venv (`laya==0.3.5`), unix socket `0600`, `LAYA_SIDECAR_SOCKET` overridable.

**Supervised on the HOST** as `~/.config/systemd/user/laya-sidecar.service`, which
`podman exec`s into this container. Nothing inside can supervise it: no systemd
(PID 1 is `podman-init`), no podman, no crontab, and the venv's interpreter is a
symlink into the container so it cannot run on the host at all.

**Recovery proven, not assumed**: killed the head → restored in ~30 s with no human
action → `laya doctor` READY.

## A defect that mattered: `doctor` did not check the head

`laya doctor` reported **READY, exit 0** while the sidecar was dead — it probed only
llama.cpp. So it certified a decision layer that could not answer one question. Fixed:
the check now covers the head, and a head failure gets **different advice** ("the
backbone is UP … do NOT restart it") because the remedy differs. Live:

```
laya: READY
  endpoint  : http://10.10.10.217:8083
  /health   : ok
  /v1/models: ok (serving the expected model)
  head      : ok (sidecar at /run/laya/sidecar.sock)
```

## Documentation (in `_docs/`, ONE folder)

| Doc | Content |
|---|---|
| `02-guides/RUNNING-LAYA.md` | run Laya + llama.cpp; why two processes; verify a real decision; the temperature clamp; determinism and its limits; troubleshooting |
| `02-guides/INSTALL-IN-HERMES.md` | `hermes mcp add`; the new-session requirement and the stale-process tell; tool contracts; cost/latency |
| `05-operations/SUPERVISION.md` | the host unit; why NOT in-container; the two approaches that do not work; the recovery test |

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

## Next — nothing in this repo is blocked; ranked

**Spec 07 is complete.** All seven specs (01–07) are implemented, gated and committed. What remains is
not "finish the spec" — it is the work the specs deliberately deferred, plus one piece of supervision.

1. **Supervise the sidecar** — it is `nohup` only, and nothing in-container can supervise a daemon. A
   host systemd unit is the fix and it is NOT in this repo, so this needs a host-side action. Until
   then a container or host restart silently removes the reference oracle the live corpus needs.
2. **The deferred graph capabilities**, each with its reason already recorded in the master spec:
   `agent` / `quorum` node kinds (R2/R17), cancellation of in-flight work, and **resume-from-crash**
   (the hardest thing being absorbed; research §4: *"that is now ours to earn"*). `writes: true` nodes
   get mutual exclusion today, deliberately, and v1 creates no git worktrees.
3. **`laya-graph` has no store** — run records live in `RunReport` and `DecisionRow` already carries
   `run_id`/`node_id` for the day a consumer persists a run. Persisting runs is a later spec, and it
   would mean touching `laya-store`, which was out of scope here.
4. **The open research questions** in `_docs/06-research/03-four-architecture-questions.md`
   (threading, redb config, absorb-vs-supersede). Research, not implementation.
5. **Commit-hygiene debt (register A24 residual)** — 8 of 49 commits name a tool. Fix-forward chosen;
   the gate prints the count. Do NOT rewrite history: the SHAs are cited across the registers.

## Known gaps (honest)

* **The `laya` binary in `~/.local/bin` was stale at one point during this session** (no `graph`
  subcommand) and was reinstalled from the release build. `laya-mcp` was NOT refreshed: it is held open
  by three live Hermes MCP connections (`Text file busy`), and this session changed nothing in that
  crate, so the installed binary is current. If `laya-mcp` is ever changed, the MCP connections must be
  restarted before the install can be replaced.
* **No tool is unimplemented.** All seven error only on real error conditions; this section previously
  said "five of seven tools are unimplemented", which was true several sessions ago and false now.
* The sidecar is **unsupervised** (`nohup` only). Nothing in-container can supervise a daemon; a host
  systemd unit is the fix, and it is not in this repo.
* The sidecar is **strictly serial** (the backbone at `:8083` is ~5.8x parallel; the head is not).
* **The MCP client session is not supervised** — a session holding a pre-rebuild binary keeps its old
  behaviour (MCP clients do not hot-reload), so a rebuild needs a client restart to take effect.
* `laya doctor` reads the socket env var at call time while `laya-mcp` reads it at startup. Same
  default, so no drift today, but a relocated socket can diverge.
* **`calibrate` waits on data, not code.** The outcome path exists (`laya ledger outcome`); what is
  missing is recorded outcomes. `laya ledger stats` prints **NO OUTCOMES RECORDED** while there are
  none, so the gap is visible rather than implied.

## Verified state at an EARLIER point — kept for the progression, NOT the current state

**Read the header for the current numbers.** This block is a snapshot from when the workspace had
148 tests and five unimplemented tools; it is retained because the shape of the progression is
itself informative (each number below is what the earlier sections were written against).

```
cargo xtask gate        -> PASS (fmt + clippy -D warnings + test)
tests, whole workspace  -> 148 passing, 0 failing        <-- then; 345 now
clippy --workspace      -> 0 warnings
cargo fmt --all --check -> clean
doc links               -> 45 relative links checked, 0 broken   <-- then; 61 now
laya doctor             -> READY, exit 0 (against the live server)
laya doctor --url <bad> -> NOT READY, exit 1, actionable diagnostic on stderr
```

## P1 — what the measurements decided (2026-09-29, second pass)

Before designing P1's concurrency, three things were **measured**, and each changed
the design:

```
Laya /embedding          : 35 ms median
8 concurrent /embedding  : 121 ms wall   => 5.8x parallel, NOT serialised
redb, 2nd process        : Error: DatabaseAlreadyOpen   => B4 CONFIRMED (clean, typed)
served capabilities      : ["completion"]  (NOT "embedding")
```

1. **The backbone is not the bottleneck** — so a queue in front of it buys no
   throughput. Building one "to scale" would add a scheduling layer for nothing.
   The queue is justified by *fairness* (a blocking `gate` must not wait behind a
   bulk scan), not throughput.
2. **The redb collision is real and typed** — `DatabaseAlreadyOpen`, not corruption.
   Spec 04's "the server never opens the store for writing" is therefore
   enforceable, and the single-writer design is right.
3. **The capability string is a trap** — the model advertises `["completion"]`
   because the GGUF holds the backbone only. A readiness check requiring
   `"embedding"` would report a **working server as broken**. We check the model id.

**User question answered**: *"on redb collisions, could be used a priority queue?"* —
**yes, and the measurement says where**: high value for *work* scheduling (the
blocking gate lane), low value for *ledger writes* (records are records). Spec 06
implements it; see `_docs/05-operations/READINESS-AND-QUEUE.md`.

## What was done

**1. Research and design (complete).** The project exists because of IndyDevDan's
*Ten Levels of Jev* video + repo, which is the third independent arrival at the
architecture we already built ("the graph drives, the model decides"). Design
written, then **consolidated into one doc** with the video/repo links and our full
prior research trail.

Key measured finding: **Laya's decision head is 26.5M params of pure linear
algebra** (2 layers, attention/LayerNorm/GELU/scorer, 106 MB), so the whole
decision layer can be **Rust with no Python and no torch** — which also removes the
`tokenizers`/`transformers` conflict that already bit us once.

**2. Adversarial review (complete, and it found real problems).** Dispatched as a
resumable pi session with `glm-5.3:cloud` (**full model, not flash** — user's
explicit direction). Exit 0, ~8 min, `_docs/06-research/02-analysis-glm-5.3.md`
(460 lines). Session: `_tmp/pi-sessions/design-review/`.

**3. Project scaffolded (complete).** Workspace + `crates/` + `xtask`, following
borrowed conventions (edition 2024, a strict lint set, `cargo xtask gate`), plus
the `.cargo/config.toml` alias that makes the documented `cargo xtask gate` actually
run (found broken by verification).

**4. Specs implemented TDD (57 tests green, clippy 0, fmt clean).**
- `laya-abi` — the question/answer contract
- `laya-core` — `Policy` (per-type bands, composite scoring), `DecisionPayload`,
  **`readiness`** (spec 05)
- `laya-head` — **`clamp_temperature`** (blocker B2) landed and tested
- `xtask` — the gate

**5. Documentation tree built (SSOT).** `_docs/` with the full section hierarchy,
INDEX + TOC regenerated from the directory listing, and a link checker: **41
relative links, 0 broken**.

**6. The mandatory readiness requirement implemented (spec 05).** User directive:
*"it will be mandatory the app to check for laya be running! and it must inform/stop
if not!"* — 16 tests covering every failure mode, the actionable message, fail-closed
`gate` → `ask` (never `allow`), and a test proving no key can leak into the
diagnostic.

## What the review found (read this before writing any code)

**Five BLOCKERS, all accepted.** The two that would have caused real damage:

**B2 — the temperature we planned to port is one Laya REFUSES to apply.** The raw
config value `choice:11+ = 0.1006` multiplies logits ~10×; Laya clamps to
`[0.5, 5.0]` and warns *"treat confidence from the affected buckets as
uncalibrated."* Our fact F5 said "read the config, never hardcode" — following it
literally would have **published a 0.24 probability as 0.99**, defeating the
confidence gate that the whole design depends on.

**B5 — our central DAG mechanism was IMPOSSIBLE.** We planned a `transform` node to
compute the confidence band. A graph `route` node's expr language is *"only member
access … no arrays, no functions"* (`expr.rs:3-8`) — **I re-verified this myself**
before accepting it. A transform cannot compute a band; `route`'s `eq` is exact
equality with no ranges. **Fix: `laya-rs` computes the band and emits it in the
payload**, so the graph gets an exact string to match. Thresholds still live in
`Policy` TOML. My §11 Q4 lean had chosen the impossible option.

Also: **B1** — "fully deterministic" was overstated (the review showed measured
drift of edge-flipping size, and that the journal records `input_hash`, not the
input, so runs are *auditable*, not *re-executable*). **B3** — P1 as sequenced
could not produce a decision at all (nothing ran the head). **B4** — one redb store
behind N stdio servers collides with redb's single-writer model.

**Fact-table corrections (F1–F15):** 4 wrong or overstated in ways that matter —
F1's param count includes an `act_head` that F2 omitted; `scorer.0` is a LayerNorm,
not `[1024,1024]`; `nhead` is computed in code, **not in the config**, so "never
hardcode" could not be followed for the constant attention needs most; pidag pins
`redb 1.0`, not 2.

**One review finding already fixed in code:** `m4` — a `noul`'s confidence is
`max(p, 1−p)` while choice/score is `1 − H(p)/log k`. Those are different
quantities, so a single global band made "0.6 confident" mean two things.
`Policy` now has per-type thresholds (`band_for(kind, confidence)`), with tests
including the inverted-pair and equal-pair rejections.

## What failed / needs attention

- **The video transcript was never fetched** — YouTube returned HTTP 429 on the
  subtitle endpoint. All video content came from the description + repo README. The
  reviewer flagged this too: **no video-specific claim could be verified from the
  files given.**
- **The Python ground truth lives in another project's venv**
  (`/projects/digital-marketing/.venv`), which is also the venv that produced the
  dependency conflict we cite. The review is right that our evidence base is a side
  effect of an unrelated project.
- **`calibrate` has no ground truth** (review m7): ECE needs
  `(confidence, correct)` pairs and nothing records outcomes. **Do not build it**
  until an outcome-recording path exists — documented in `05-operations/DECISION-LEDGER.md`.
- **Two unmeasured numbers that decide the design**: the Rust head's cold-load time
  (decides whether the hook lane can spawn per call), and llama.cpp determinism
  across thread counts/builds (decides whether replay is meaningful).

## Key files

```
/projects/laya-rs/
├── AGENTS.md                                     ← the operating contract
├── Cargo.toml                                    ← workspace + lints + redb 4.2
├── specs/{01,02,03,04}-*.md                       ← TDD contracts; 01+02 implemented
├── crates/laya-abi/src/lib.rs                     ← Question/Answer (42 tests total)
├── crates/laya-core/src/{policy,route_payload}.rs ← Policy + DecisionPayload
├── xtask/src/lib.rs                               ← `cargo xtask gate`
├── _docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md   ← READ FIRST
├── _docs/06-research/01-design.md                 ← the design (superseded in part)
├── _docs/06-research/02-analysis-glm-5.3.md       ← the full review
├── _docs/05-operations/DECISION-LEDGER.md         ← ledger schema + single-writer
└── scripts/regen-index.py, check-real-crates.sh      ← regen INDEX/TOC + verify
```

## Next steps (in order)

1. **`laya-mcp` — the MCP stdio server (spec 04).** Its readiness precondition is
   now proven end to end (`laya doctor`), so the server can wire the same path at
   startup and refuse to run when Laya is down. The queue (spec 06) is ready to back
   the tool lanes.
2. **The pinned Python sidecar for the head (blocker B3)** — named and version-pinned,
   replacing the per-call interpreter spawn.
3. **The two-process redb test** — B4 is now CONFIRMED by measurement
   (`DatabaseAlreadyOpen`), so the test asserts the typed error rather than discovery.
4. **`decide`**, then **`ask_file`/`ask_files`** (the levels that pay most).
5. **Before P4**: the decision-level parity oracle (B1/M4) and the corrected
   36-tensor forward spec (M3) — spec 03 already carries both.
6. **Amend `01-design.md` with the blockers** so there is one canonical design; the
   split is deliberate today but must converge.
7. Register the open items in `/projects/docs/01-overview/OPEN-REGISTER.md`.

## Commands

```bash
cd /projects/laya-rs
cargo xtask gate                       # fmt + clippy -D warnings + test, all zero
cargo test -p laya-abi -p laya-core    # the offline contracts
python3 scripts/regen-index.py            # regen INDEX/TOC + verify every link
bash scripts/check-real-crates.sh         # the zero-warning proof
```

