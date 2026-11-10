# OPUS ROUND 2 — the build order, and what I had wrong

**claude + opus, 2026-10-02**, resumed session `d58b577f-fc19-48d5-b690-f6d7524ea4f8` (15 turns,
$4.27, 620 s). Full report: `_tmp/opus-graph-analysis/ROUND2.md` (346 lines), with every
probe flow and its run report under `round2/`.

Round 2 was asked: is the five-inert-fields finding the real problem or a symptom, what is the build
order, which basic did I miss, and **give me one working pipeline on disk, run through laya**. It
answered all four and ran 20+ probes to do it.

---

## The root cause, which I had not found

**The schema accepts any key.** `Flow` and `Node` have no `#[serde(deny_unknown_fields)]`. So a
**typo of any field is an inert field** — and F5's "five inert fields" is not a count, it is an
unbounded set. Re-verified here:

```
q1-unknown-key-typo   b has "depend_on": ["a"]     validate: VALID
   0 ms  b  b-ran          <- ran at 0 ms, BEFORE a finished
   0 ms  a  a-done (1002 ms)
```

One transposed letter silently deletes the ordering guarantee, and `validate` says the flow is
valid. **Field values are unchecked too** — four measured silent misroutes, each with a passing
`validate`: `on: "bnad"` routes on `label` regardless; `on: "band"` with `yes`/`no` port keys sends
every answer to the fallback; a port key typo (`noo`) is hidden by the `_` catch-all; and a port
target naming no node runs **nothing** and exits 0 with `not_started: []`.

## Its answer to my question: F5 is a symptom, not the finding

> F5 and C1 are the same absence at two layers: **nothing binds a declared thing to an observable
> effect.** At the parse layer, keys and values are not checked against what the engine reads. At
> the execution layer, edges are not checked against what runs. At the report layer, `RunReport` has
> no `skipped` list and no attempt count, so the gap is invisible.

**And it split the harm**, which is the part I had not seen:

| kind | members | why it ranks differently |
|---|---|---|
| **loud-inert** | `retry`, the subflow refusal | the run stops or `validate` refuses — a missing feature, not a lie |
| **silent-wrong** | near tie, decide `depends_on`, **every typo**, the four misroutes, decide `timeout`/`allow_failure`, `$.inputs` | all exit 0 |

**Attack the silent-wrong members first.** They are parse-and-validate problems, hours not days, and
with no execution blast radius — which is why its new spec A now ranks **above** the scheduler fix.

## Its re-ranked build order (my order was wrong again)

| # | what | why here |
|---|---|---|
| **1** | **spec A — "the schema is a contract"** (new) | closes every silent-wrong item that is not a scheduler problem, in hours, with no blast radius |
| 2 | **17** — route-aware scheduler, AND/OR edges, `skipped` | the only fix for C1; also makes decide `timeout`/`allow_failure` real |
| 3 | **18** — `{{node_id}}` | needs 17; makes graphs general |
| 4 | **16** — `question_type` | real but small; its value is confidence |
| 5 | **19, reduced** to the doc corrections and the fixtures | its R1 is wrong as written — see below |
| 6 | CLI library flag | last |

**I had 17 first and 16 first before that. Both times the review moved the cheap parse-layer contract
above the scheduler.** I have now been wrong about the ranking twice in the same direction: I keep
ranking the *interesting* fix above the *load-bearing* one.

## It caught my spec 19 rejecting the committed example

Spec 19 R1 would refuse three shapes. **Re-verified here** — all three are present in
`_docs/02-guides/wc-classify-chained.flow.json`, the working example its own control row requires to
stay valid:

```
gt100_ask.depends_on -> gt100_gate   (a port target of gt300_ask)   <- rule (b)
gt300_gate.depends_on -> count_words (a shell)                      <- rule (c)
...and the flow validates today.
```

The gate-then-ask pattern **requires** those shapes, and in that flow they do not mis-execute. The
refusals are too broad to be precise, and making them precise is as hard as fixing the scheduler.
**Spec 19 R1 must be dropped, not tuned.** Same defect in spec 17 R5 if it is ever invoked.

## It caught my spec 16's criteria being vacuous

**Re-verified here:**

```
$ cargo test -p laya-graph --lib the_documented_fields_exist_on_the_row
running 0 tests
test result: ok. 0 passed; 0 failed; ...     <- satisfies `grep -qE '^test result: ok'`
```

The test lives in **laya-store** (`crates/laya-store/src/lib.rs:1525`); in `laya-graph` it matches 0
tests and the criterion passes anyway. The other three spec-16 criteria have no grep at all and
target the wrong crate — the wire payload is built in `laya-cli`. **All four pass with nothing
implemented.** This is the exact defect class this project keeps paying for: a check that cannot find
its input must FAIL, not pass with zero.

## Two rows of my own field table were wrong

I wrote that `timeout` and `allow_failure` work. They work **on a shell and on a tail `mcp` node**;
on a **`decide`** they are dead — because decides run in phase 1, where they are not applied. My
measurement was made on a shell, so it passed. That is the same error as the brief that said the head
took 1.3 s: **a value measured on one path, asserted for all paths.**

## The working pipeline it built — the answer to "the honest interim"

A pre-commit router that sends a commit down the Rust path or the docs path by whether it touches
`crates/`. Re-run here end to end:

```
commit 1d72374 (docs only):  scope_gate no/mid -> scope_ask no -> witness literal=0 measured=0
                             -> docs_citations "59 docs, every citation resolves" -> DOCS PATH COMPLETE
commit c7dcff0 (3 files under crates/):  scope_gate yes/high -> scope_ask yes
                             -> witness literal=3 measured=3 -> rust_fmt "rustfmt: clean" -> RUST PATH COMPLETE
```

Different branch, driven by the value. And the control: **its own forged-literal run fails the
run** (`STOPPED EARLY at 'witness'`), so a stale or forged literal cannot exit 0.

The technique worth keeping: **the one thing that cannot be done inside the engine — putting run-time
data into a decision — is done by a 40-line instantiation script before `laya graph run`, and a
*witness node* re-measures the same value in-run and FAILS the run if the literal disagrees.** It
cannot prevent the wrong branch from starting (that is D1), but it makes the lie loud. Spec 18
removes the window.

And a measurement against my own design: asking the head over a **list of paths** ("does the commit
change any file under crates/?") answered **`yes` at band mid for the docs-only commit** — wrong. A
threshold over a **count** was right at 0.84/0.96. Set-membership is outside this head; that supports
spec 18's placeholder-plus-phrasing over a raw `prompt_from`.

## What I am doing about it

1. **Spec 20 → "the schema is a contract"** (was going to be "retry retries"): `deny_unknown_fields`,
   every value the engine branches on validated, the refusals, and **the field-effect test** that no
   future field can escape — because a spec audits once and new fields arrive one at a time.
2. **`retry` is refused, not implemented** — retrying a `decide` re-asks the head and writes a second
   ledger row; retrying a `writes: true` node re-applies an edit; and `NodeRecord` has no attempt
   field, so a retry that happened is indistinguishable from one that did not.
3. Spec 19's R1 dropped; specs 16, 17, 18 corrected per its list.

**The field-effect test, its shape** — the part no future field can escape:

```rust
const NODE_FIELDS: &[(&str, Effect)] = &[
    ("depends_on", Observable { .. }),   // two fixtures differing ONLY in this field; run both; they must differ
    ("retry",      Refused    { .. }),    // validate must return this error text
    ("timeout",    Observable { .. }),    // and it must hold for kind=decide too
];
// test 1: a FULLY POPULATED Node's serde_json keys must equal NODE_FIELDS' names.
//         Adding a field without a row fails. (Needs a fully populated value: skip_serializing_if hides None.)
// test 2: iterate the table; each row must hold.
// test 3: an unknown key must fail to parse. This pins N1.
```

Mandatory control: make one `Observable` field inert on purpose and watch test 2 fail; delete a row
and watch test 1 fail. **A table test nobody has seen fail is decoration.**
