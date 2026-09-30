# Design amendments — what the adversarial review changed (2026-09-29)

**Status**: MANDATORY AMENDMENTS. Read this **before** `DESIGN.md`, and treat these
as corrections to it. Where they conflict, this document wins.

**Source**: `ANALYSIS-glm-5.3.md` — an adversarial review by pi + `glm-5.3:cloud`
(full model, resumable session). Review brief: `_tmp/../docs/ANALYSIS-BRIEF.md`.

**Why this file exists**: the review found five BLOCKERS, several of them in claims
the design labelled "verified". Per SSOT, the design is NOT silently rewritten —
the review is preserved verbatim, and this document carries the corrections so the
disagreement stays visible.

---

## 1. The five blockers, and the resolution

### B1 — "Fully deterministic" is implementation-scoped, not absolute. **ACCEPTED**

The argmax observation is real (review verified `agent.py:341`, no sampling
anywhere), **within one fixed build**. But drift of edge-flipping size is
*measured*: the design's own Δp = 0.0006 is one probability, one state. A **Rust
head adds a second independent drift source**, and batch padding changes kernel
shapes. F15 cannot rescue replay because the journal records `input_hash`, not the
input.

**Consequence the review is right about:** a near-tie can flip an edge between the
Python path (P1) and the Rust head (P4) — and replaying a journalled run can
re-route. The design's "tune thresholds without silently rerouting" claim was
**too strong**.

**Resolution — the payload must carry the tie margin:**

| change | where |
|---|---|
| every payload carries `margin` (top1 − top2 logit gap) | `laya-abi` (`Answer`), `laya-core` (`DecisionPayload`) |
| a `margin_floor` in `Policy`; a margin below it routes to the escalate/catch-all port, never to a confident action | `laya-core::policy` |
| every ledger row carries **provenance**: laya version, head hash, backbone quant+hash, batch shape | `laya-store` |
| the parity oracle becomes **decision-level** — label agreement + margin histogram over a corpus — not `Δp ≤ 0.001` on one value | spec 03 |

**Language we must stop using:** "fully deterministic" → **"deterministic within
a pinned implementation; auditable across implementations via the journal, with
near-ties escalated rather than decided."**

### B2 — The temperature we planned to port is a temperature Laya REFUSES to apply. **ACCEPTED — this one would have broken the whole gate**

`common.py:224-240`: `TEMP_MIN = 0.5`, `TEMP_MAX = 5.0`; `agent.py:205-220` clamps
every config temperature at load and warns. The raw config value
`choice:11+ = 0.1006` multiplies logits ~10×, which would publish "a 0.24 top
probability as 0.99".

**Our F5 said "read the config, never hardcode" — and quoting the raw value would
have shipped exactly the bug the clamp exists to prevent, defeating the confidence
gate that is the design's central safety mechanism.**

**Resolution:** the Rust head's port spec carries `clamp_temperature` verbatim
(clamp to `[0.5, 5.0]`, NaN → 1.0); a clamped bucket is flagged in the answer and
the ledger; confidence from an affected bucket is marked **uncalibrated**.

### B3 — P1 as sequenced cannot produce a decision at all. **ACCEPTED**

`:8083` serves the **backbone only**. The head, the tokenizer and `build_sequence`
are Python today. P1 said "usable today, no Rust head needed, proxying :8083" —
but nothing in P1 runs the head. It would silently depend on the
digital-marketing venv, which is the fragile dependency P4 exists to remove.

**Resolution:** P1 names its head runtime explicitly — a **persistent Python
sidecar pinned to a laya version** (not a per-call interpreter spawn), and the
pinned version is recorded. P4 replaces it. "No Rust head needed" becomes "no Rust
head, but a pinned sidecar is required".

### B4 — One redb store behind N stdio servers collides with redb's single-writer model. **ACCEPTED**

Every consumer spawns its own `laya-mcp` against one embedded store. A hook
firing while a session holds the store is the **normal** case, and the second
writer cannot open it. Also F10 was half wrong: amem pins `redb = 2`, **pidag pins
`1.0`** — different majors.

**Resolution:** a `laya-mcp` process opens the store **read-only** and appends
decisions to a **per-process segment file**; a `laya ledger compact` command (the
`laya-cli` binary) is the single writer that merges segments. The server never
needs write access, so the collision cannot occur. Verified by a one-line
cross-process test in P1 (spec 04).

### B5 — The confidence-band mechanism, as drawn, is IMPOSSIBLE in pii-graph. **ACCEPTED — and independently re-verified**

The review is right, and I checked the source myself before accepting it:

```
crates/pii-graph/src/expr.rs:3
  "Only object member access is supported: $.‹key›(.‹key›)*. No arrays, no functions."
crates/pii-graph/src/expr.rs:17-32   eval = a pure key-walk
crates/pii-graph/src/nodes.rs:43-62  transform emits a member value or a constant
crates/pii-graph/src/nodes.rs:113    route's `eq` is exact serde_json::Value equality
```

`transform` **cannot compute** `band` from `confidence` — there is no comparison,
no arithmetic, no conditional. And `route` cannot express a range. So the design's
"transform → band (thresholds in CONFIG, one line)" — which §5 called *"forced by
F8/F9"* — **cannot be built**, and my §11 Q4 lean ("transform, not Laya") was
choosing the impossible option.

**Resolution — the band is computed by `laya-rs` and emitted in the payload:**

```jsonc
// a laya node's payload
{ "label": "bug_in_code", "band": "high", "confidence": 0.93, "margin": 0.41, ... }
```

`route on = "$.payload.band"` then works with the **existing** expr language,
because it is plain member access and exact string equality. This is the review's
option (a), and it inverts §11 Q4. The thresholds still live in **config** (the
`Policy` TOML) — they are simply applied by `laya-rs` instead of by a transform
node, so the "thresholds in one reviewable place" property survives intact.

**And it costs nothing in pii-graph — but it is honest to say it is not free:**
the review's finding **m3** is that a `laya` node kind needs a new `NodeFactory`
registered in pii-server's `registry()` **and** `lint_registry()`. §11 Q1's lean
("wrapper first, no pii-graph change") is therefore **technically true and
practically misleading**. Corrected.

---

## 2. MAJOR findings accepted, with the change they force

| # | finding | change |
|---|---|---|
| M1 | **V6 does not protect cyclic graphs** — it is skipped when V3 finds a cycle, and returns `None` if its own topo sort fails. `failover.toml` (the design's own showcase) has a retry cycle. | §5 must state the **acyclicity condition**. Do not advertise inherited safety for a diamond added to a cyclic graph. Near-ties use `any`/`allsettled` joins downstream of a router. |
| M2 | **Latency story mixes three paths.** "~0.3 s" is the in-process PyTorch number; the HTTP path measured **608 ms**; hooks add process spawn + head load (cold load measured at **23 s** for the Python stack). | §6's "cheap enough for EVERY tool call" **contradicts** §9.3 and is withdrawn. A per-integration-point budget must be measured on the real path; the hook lane needs a **persistent server**, not a per-call spawn. |
| M3 | **F2 is wrong/incomplete**: omits all four `act_head` tensors (which F1's own param count *includes*); `scorer.0` is a **LayerNorm [1024]**, not `[1024,1024]`; `nhead = 16` is computed in code, **not in the config**, so "read the config, never hardcode" cannot be followed for the constant attention needs most; `max_prefixes 6` is **unused** by inference. | Rebuild §8 from the real 36-tensor forward pass: `act_head` decision made explicitly, `nhead`, eps, GELU variant, `-1e4` mask, gather, padding mask, `type_emb` add, the act-head feature vector, `build_sequence` constants, collate padding. |
| M4 | **The parity oracle cannot certify the port** — it keeps the tokenizer and `build_sequence` in Python and swaps only the encoder. | The oracle tests the **Rust binary from text** (its own tokenizer, its own `build_sequence`) against a pinned Python reference over a corpus: label agreement, margin histogram, ECE. |
| M5 | **Rules-first ordering silently dropped.** 039 F5 / 040 §3.5.1 say the model must NOT sit in front of classification that is already correct and free. | Write the invariant into §5/§7: **static rules match first; the model classifies only the unresolved remainder**; every gate names its deterministic companion control. |
| M6 | **`route` as an MCP tool contradicts our own thesis** — an agent calling `route` selects edges outside any graph, which is the injection failure mode the design exists to prevent. Also: pii-mcp returns errors as **successful** results, and grants workspace-wide `FS_READ\|FS_WRITE\|PROC\|MEMORY` on every call. | **Drop `route` from the MCP surface** (expose it as a pure `match(value, rules)` helper, or not at all). Define per-tool least-privilege scopes; use the protocol error surface; `gate` fails **closed**; `ask_file` gets a defined read scope. |
| M7,M8,M9,M10 | (language/script guard; ask_file secret policy; runtime pinning; the two-lane loop) | See §4 checklist of the review; each is a spec item. |

## 3. Corrections to our facts table (F1–F15)

**Replace the design's facts table with this.** The review checked each one against
source:

| fact | verdict |
|---|---|
| **F1** | **Partly wrong.** The 26,512,134-param count **includes** the 263,938-param `act_head` that F2 omitted (26,248,196 without). Also: the port needs the **tokenizer + `build_sequence`**, which F1 never mentioned. |
| **F2** | **Wrong/incomplete.** Missing all four `act_head` tensors; `scorer.0` is a LayerNorm `[1024]`, not `[1024,1024]`; 36 tensors total; "no exotic ops" glosses gather/topk/entropy/masked_fill and the in-code `nhead`. |
| **F3** | **Correct in substance, stale citation.** Argmax is at `agent.py:341` (not :316). "No seed pinning" holds only within a fixed implementation — see B1. |
| **F4** | **Correct math, wrong consequence.** Argmax is invariant for label routes ✓, but "can NEVER change the EDGE" is **false for band/score routes** (m5) — including the design's own second route. Scope it. |
| **F5** | **Dangerously incomplete** — see **B2**. |
| **F6** | **Verified live** 2026-09-29 (`/health` → ok). |
| **F7** | **Real number, wrong inference.** Δp=0.0006 quantifies Q8_0 backbone quantisation, not Rust-head fidelity. Not evidence for the port. |
| **F8** | **Accurate for this checkpoint, overstated as a fact.** If the Rust port drops `act_head`, the field vanishes and the tool contract diverges from Laya's documented response. Decide explicitly. |
| **F9** | **Fine** — but note unclamped sharpening (B2) would manufacture fake saturation. |
| **F10** | **Half wrong.** pidag pins `redb 1.0`; amem pins `2`. rmcp 3.2 ✓. |
| **F11** | **Accurate** ✓. |
| **F12** | **Accurate** ✓. |
| **F13** | **Overstated.** V1 checks edges against rule-declared ports — it says nothing about labels, and a catch-all port with **no outgoing edge** is only a V5 *warning*: the unwired catch-all is a silent dead end, not an error. |
| **F14** | **Overstated** — see **M1**. Acyclic only. |
| **F15** | **Overstated.** Runs are **auditable** (values are journalled) but **not re-executable** (`Started` records `input_hash`, not the input). Use "auditable", not "re-executable". |

**Meta-correction:** the design said "measured 2026-09-24" while citing laya
**0.3.4** line numbers; the venv has **0.3.5**. Several "measurements" were
inherited from docs 039/040 rather than re-taken. **Every number must now carry
when it was taken and against which version.**

---

## 4. What is confirmed good (keep, don't relitigate)

- **The seam is right** — decision as `$.payload`, `route` branches, the graph owns
  every edge. Confirmed against `nodes.rs` and proven in production by
  `failover.toml`, whose catch-all `{port="secondary"}` is the pattern we
  generalise.
- **Argmax with no sampling** — a real and useful observation.
- **F4's separability insight** (temperature tunes confidence, not the label) is
  correct *where it applies* (label routes) and worth keeping once scoped.
- **MCP stdio + the spec-22 bounded-lifetime pattern** correctly identified and
  copyable (`main.rs:144-158`).
- **Phasing the Rust head after the levels that pay** is the right instinct.
- **The offline-mock / live-suite test split** is the right test shape.
- **Two lanes, never one** — correct posture, matches the video and 039/040.

---

## 5. The re-issued order of work

The review's own conclusion is that **the design must be re-issued before any
code**. The revised sequence:

1. **Amend the design** with §1–§3 above (this document is that amendment).
2. **P1 redefined**: a `laya-mcp` that proxies a **pinned Python sidecar** for the
   head, computes `band` and `margin` itself, and enforces the catch-all-adjacent
   rules. Includes the one-line **redb cross-process test** (B4).
3. **Spec 02 (route payload, done)** and **spec 01 (policy, done)** already carry
   the corrected contracts; spec 01's exit criteria are **green** (42 tests, 0
   clippy warnings, fmt clean).
4. **Before P4**: a decision-level parity oracle (B1/M4), the full 36-tensor forward
   spec (M3), and the `clamp_temperature` port (B2).
5. **Do not build** the transform-computed band (B5) or an MCP `route` tool (M6).

**Unresolved and honestly listed** (review §6): video/repo-specific content could
not be verified from the files (the transcript was 429-blocked); llama.cpp
determinism across thread counts/builds is unmeasured; the Rust head's cold-load
time — the number that decides the hook lane — is unmeasured anywhere.
