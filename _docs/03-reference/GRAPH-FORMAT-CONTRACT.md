# THE FLOW FORMAT CONTRACT — what changes, why, and what it costs

**Reference for the change specified in `specs/20-the-format-is-a-contract.md`.**
Measured 2026-10-02. Every "before" in this document was **run through `laya graph validate` and
`laya graph run`** on the live head; every "after" is what the spec requires. Nothing here is
inferred from reading code unless it is marked **inference**.

---

## 1. Why the format changes at all

**The format accepts any key, so a typo of any field is silently inert.** `Flow` and `Node` carry no
`#[serde(deny_unknown_fields)]`. The consequence is not cosmetic:

```
q1-unknown-key-typo     b has "depend_on": ["a"]     laya graph validate -> VALID
    0 ms  b   b-ran          <- ran at 0 ms, BEFORE its supposed predecessor finished
    0 ms  a   a-done (1002 ms)
q1c-control            b has "depends_on": ["a"]     laya graph validate -> VALID
    0 ms  a   a-done
 1002 ms  b   b-ran          <- ordered
```

One transposed letter deletes an ordering guarantee, `validate` calls the flow valid, the run exits
0, and the report gives no sign. This is why the earlier finding of "five inert fields" was a
miscount: **the inert set is unbounded, because every misspelling is a member.** Field *values* are
unchecked too (§3).

An independent review ranked this change **first** — above the scheduler fix — because it costs hours
rather than days and has **no execution blast radius**. The scheduler defect (which nodes run depends
on array order) is more severe but far more invasive.

## 2. The twelve changes

Legend: **P** = permanent and unconditional. **I** = interim refusal, lifted by the spec named.

| # | today | after | kind |
|---|---|---|---|
| 1 | any key accepted on `Flow`/`Node`; a typo is inert | `deny_unknown_fields`; unknown key refused, **naming it and the nearest known key** | **P** |
| 2 | `inputs` accepted; `$.inputs.*` documented as substitution in 3 places and in the `prompt` doc comment | **`inputs` and `$.inputs.*` refused**, and every advertisement removed | **P** |
| 3 | `on` accepts any string; anything but `band` silently means `label` | `on ∈ {label, band}`, else refused | **P** |
| 4 | port keys unchecked; a typo is hidden by `_` | port keys must be options or `_` (`on=label`), band names or `_` (`on=band`) | **P** |
| 5 | a port target naming no node runs **nothing**; `not_started: []` | every port target must exist | **P** |
| 6 | `run` may be absent on a `shell`; succeeds with an empty command | `shell.run`, `mcp.tool`, `decide.question`+`prompt` are required for their kind | **P** |
| 7 | `version: 999` accepted | `version == 1` | **P** |
| 8 | `timeout`/`allow_failure` declared and **dead on a `decide`** | refused on a `decide` until **spec 17**, then real | **I — 17** |
| 9 | `retry` declared, validated, **inert** | refused on every node until implemented for non-`decide`, non-`writes` nodes | **I — later** |
| 10 | `escalate_on_near_tie` declared, and the target **runs unconditionally** | refused until near-tie routing exists | **I — later** |
| 11 | a node id may contain `/`, colliding with the synthetic route id | refused | **P** |
| 12 | a `subflow` node falls through to `Other`; the error blames `depends_on` | the message says *"`subflow` needs a library, which the CLI does not load"* | **P** |

### 2.1 Rows 1 — strictness

The single highest-value change, because it closes the *class*: a misspelling can no longer be inert,
whatever the field. The refusal should read:

```
laya: node `b` has an unknown key `depend_on` — did you mean `depends_on`?
      (unknown keys are refused: a key the engine does not read would be silently ignored)
```

The nearest-known-key hint is what makes a one-second fix out of what is otherwise a hunt.

### 2.2 Row 2 — the format advertises a feature that does not exist

`_docs/03-reference/GRAPH-SPEC.md` shows this as a working example, at lines ~32, ~40 and ~103:

```json
{ "inputs": ["issue"],
  "nodes": [ { "id": "d", "kind": "decide", "prompt": "$.inputs.issue" } ] }
```

And `crates/laya-graph/src/flow.rs:169` documents the field itself the same way:

```rust
/// `decide`: the state to hand Laya, e.g. `$.inputs.issue`.
pub prompt: Option<String>,
```

**Measured, it does nothing:**

```
q6-inputs-ref    inputs: ["doc"], prompt: "$.inputs.doc"    validate -> VALID
    d   label=no  band=low  conf=0.3985
        <- the head was asked about the literal string "$.inputs.doc"
```

The head did not complain. It answered — at band **low**, which is the tell: it was handed a string it
could make nothing of. A flow author reading the reference doc writes a flow whose decisions are made
over the text `$.inputs.doc`, and every `validate` and `run` says the flow is fine.

This is the row where the honest move is **shrinkage**: the format loses a feature it never had. The
refusal must be explicit, because the doc taught people to write it:

```
laya: `inputs` is not implemented — nothing substitutes `$.inputs.*`; a prompt is literal state text
```

### 2.3 Rows 3–5 — the values the engine branches on

Four measured silent misroutes, each with a **passing** `validate` and `exit 0`:

| probe | what the author wrote | what happened |
|---|---|---|
| `q2-on-typo` vs `q2c-on-band` | `on: "bnad"` with band ports | routed on **label** (`no` → `_` → `esc`). The control with `on: "band"` → `go`. |
| `q13-band-with-label-ports` | `on: "band"`, ports keyed `yes`/`no`/`_` | every answer goes to `_` (`high` → `FALLBACK`) |
| `q3-port-key-typo` | ports `{"yes","noo","_"}` | `no` → **FALLBACK**; the `_` satisfies the coverage check, so the typo is invisible |
| `q4-port-target-missing` | `"no": "nosuch"` | `selected_port: "nosuch"`; **nothing ran**; `completed`, `not_started: []` |

Causes, from the code: `lower.rs:112-115` maps any `on` other than `"band"` to `label`;
`validate_decide` checks only that ports **cover the options** — never that port **keys** are options
(or band names), never that port **targets** exist; and `prune_unreached`/`remove_node` then quietly
drop a missing target.

Row 5 is the worst of the four: the run reports `completed` with `not_started: []`, so **the flow
appears to have run everything when it ran nothing on that branch.**

### 2.4 Rows 8–10 — the interim refusals, and why a refusal is not a failure

These three fields are declared, accepted, and do not do what they say:

- **row 8** — `timeout` and `allow_failure` work **on a shell and on a tail `mcp` node**, and are
  **dead on a `decide`**, because decides run in the phase where node-level timeout and failure
  policy are not applied. (Measured: a `decide` with `timeout: 0` completed in 106 ms and routed; the
  control `mcp` node with `timeout: 0` failed `timed_out=true`.)
- **row 9** — `retry: {attempts: 3}` on a flaky shell: **one attempt**, run fails on the first error.
- **row 10** — `escalate_on_near_tie`'s target has **no inbound edge**, so it is a root of the tail
  and **runs on every run** when the flag is set. Measured: `human` is unconnected in the render and
  ran anyway.

**Refusing is the correct interim state, and the message must be actionable:**

```
laya: `retry` is not implemented — the run fails on the node's first error.
      Wrap the command in a shell loop, or remove `retry`.
laya: `escalate_on_near_tie` is not implemented — and its target node runs on EVERY run.
      Remove the flag and the `near_tie_port` node until it is.
```

**Each interim refusal must name the spec that lifts it, and the test table records which.** A refusal
that outlives its reason is the same defect class as a field that does nothing: it reads as a
deliberate design when it is an accident of sequence.

`retry` is refused rather than implemented because implementing it correctly is **not a basic**: a
retried `decide` re-asks the head and writes a second ledger row; a retried `writes: true` node
re-applies an edit; and `NodeRecord` has no attempt field, so a retry that happened is
indistinguishable from one that did not. Those are decisions, not a patch.

### 2.5 Row 12 — a message that sends the reader to the wrong field

```json
{ "id": "v1", "kind": "subflow", "uses": "build" }
$ laya graph validate parent.flow.json
laya: node `v1` depends_on unknown node `build`
```

The node contains **no** `depends_on`, and `build` is a **flow** name, not a node. The cause: `Subflow`
is not a variant of `Kind` (`Shell | Mcp | Decide | Route | Other(String)`), so the node falls through
to `Other` — yet `expand.rs` handles subflows with unit tests. The schema and the expander disagree
about what a subflow node is. The CLI additionally cannot load a library
(`crates/laya-cli/src/graph.rs:50-51`), so every `subflow` node is unresolvable through the CLI.

## 3. The field-effect test — the part that lasts

A spec audits once. **New fields arrive one at a time — that is how these were found**, each by
accident. So the durable fix is a test that no future field can escape.

```rust
enum Effect {
    /// Two fixtures differing ONLY in this field; both run through validate -> lower -> run_flow
    /// with a ScriptedCaller (label chosen per node id, no head needed). `differs` must return true.
    Observable { with: &'static str, without: &'static str,
                 differs: fn(&RunReport, &RunReport) -> bool },
    /// Refused until implemented; validate must return an error containing this text.
    Refused { fixture: &'static str, error_contains: &'static str },
    /// Carries no behaviour by design. The reason is required and reviewed.
    Documentary { reason: &'static str },
}
const NODE_FIELDS: &[(&str, Effect)] = &[ /* every field of Node */ ];
const FLOW_FIELDS: &[(&str, Effect)] = &[ /* every field of Flow */ ];
```

**Test 1 — `every_serialised_field_has_a_row`** is the load-bearing one: build a **fully populated**
`Node` and `Flow` (every `Option` is `Some`, every `bool` true), serialise, and require the keys to
**equal** the table's names. **Adding a field without a row fails.** A fully populated value is
required because `skip_serializing_if` hides `None` — a `None`-populated value would let a field
escape silently, which is the bug this test exists to prevent.

**Test 2 — `every_row_holds`** iterates the table.

**Test 3 — `an_unknown_key_is_refused`** pins the root cause with a `depend_on` fixture.

**The mandatory control:** make one `Observable` field inert on purpose — have `lower` drop `writes` —
and watch test 2 fail; delete one row and watch test 1 fail. **A table test nobody has seen fail is
decoration**, and this project has paid for that lesson four times in one day.

Wire-level fields (`question_type`, `options` → `criteria`) are observable in **`laya-cli`**, not in
`laya-graph`; they get the same table shape there, asserting on the bytes a fake server receives. That
pattern already exists in `the_decide_translation_also_carries_the_ids`.

## 4. What this costs, and what it does not

**Compatibility was measured, not assumed.** The review scanned every flow JSON under `_docs/` and
`crates/laya-graph/tests/fixtures/`: **none uses a key outside the schema.** Strictness therefore
breaks no committed flow. The exit criteria still require re-validating **every** committed flow
through the CLI, because a claim of compatibility is worth exactly what its verification is worth.

**One thing to check when the attribute lands:** `expand.rs` parses subflow nodes off the raw `Value`,
so whether such a node reaches `Node`'s derive at all must be confirmed rather than assumed
(**inference** in the review, and it must be resolved by running, not by reading).

**What does not change:**
- the wire contract — `decide` still lowers to an `mcp` node plus a `route` node; the payload shape to
  the head/server is untouched;
- the `Flow`/`Node` field **names** for every field that works;
- execution behaviour for any flow that was already valid and honest.

**A guard for the worker:** if adding `deny_unknown_fields` requires touching a test that encoded a
now-refused shape, that test must be **re-pinned with a comment naming the refusal**, or removed with
the reason recorded — never silently relaxed. A refusal is only as good as the tests that stay honest
around it.

## 5. The honest summary

The format was a **wish list**: it declared fields, `validate` agreed, and the engine did something
else. `retry`, `escalate_on_near_tie`, `near_tie_port`, `depends_on` between a shell and a decide, and
`subflow` do not work; `inputs` was documented in the reference and never existed; and **any typo**
adds itself to that set.

`allow_failure` and `timeout` **do** work — on the paths where they are implemented. So the problem
was never "the engine ignores flags". It is that **nothing bound a declared thing to an observable
effect**, at any of three layers: the parse layer (keys and values unchecked), the execution layer
(edges unchecked against what runs), and the report layer (`RunReport` has no `skipped` list and no
attempt count, so the gap is invisible).

This spec closes the **parse layer**. Spec 17 closes the execution layer. The `skipped` list — spec
17's R3 — closes the report layer. **None of the three alone closes the class.**
