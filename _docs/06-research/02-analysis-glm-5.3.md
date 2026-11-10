# ANALYSIS — adversarial review of `docs/DESIGN.md` (laya-rs)

**Reviewer**: glm-5.3 (analysis only; no code, no `.rs`, no `Cargo.toml`).
**Date**: 2026-09-29. **Subject**: `docs/DESIGN.md` (2026-09-24).
**Method**: every load-bearing claim re-checked against the cited ground truth
(`nodes.rs`, `spec.rs`, `validate.rs`, `failover.toml`, `pii-mcp/src/main.rs`,
`laya_head.py`, `rl_agent_config.json`, `laya/agent.py` 0.3.5, `laya/common.py`,
`parity_test.py`, `hybrid_test.py`, docs 039/040, and — header only —
`laya-head.safetensors`). Read-only commands were used; nothing outside this
file was modified. Where I could not verify, §6 says so.

---

## 1. Verdict

The seam is right and is the seam our own prior research chose (039 J1 / 040
§3.2): a decision lands in `$.payload`, the existing `route` branches on it,
the graph owns every edge — verified against `nodes.rs` and proven in
production by `failover.toml`. The argmax observation (F3) is real and useful.
But the design's two headline guarantees are both overstated. "Fully
deterministic and replayable" is implementation-scoped, not absolute — the
design's own Δp=0.0006 is a measurement of drift large enough to flip a
near-tied edge, and the design has no tie-margin handling at all. "We inherit
the safety net" is false in any cyclic graph, including the failover ladder the
design names as its upgrade target. Worse, the §5 confidence-band pattern — the
one the design calls "forced by F8/F9" — is **not expressible** in pii-graph's
tiny expr language, so the design's central DAG mechanism cannot be built as
drawn. And two facts-table entries are wrong in ways that matter: F2's tensor
inventory omits the `act_head` tensors that F1's own param count includes, and
F5 quotes a temperature (`choice:11+ = 0.101`) that laya **refuses to apply** —
porting the config naively defeats the confidence gate, the design's central
safety mechanism. P1 as sequenced has no head runtime, and the shared-redb
architecture collides with redb's single-writer model. Fix B1–B5 and re-issue
the design before any code.

---

## 2. Findings (ranked)

### BLOCKERS

**B1 — "Same state → same port" is true only within one fixed implementation; a near-tie can flip an edge, and the design has no tie handling.**
Evidence: the argmax itself is verified — `agent.py:330-341`
(`z = logits/t_scale` → softmax → `keys[int(p.argmax())]`; no
`multinomial`/`torch.rand`/`np.random` anywhere in the decision path, grep
exit=1). But drift of edge-flipping size is *measured*: Δp=0.0006 is one
probability, one state, same Python head, different backbone
(`parity_test.py:50`; 040 §1) — a Rust head adds a second independent drift
source, and `collate_items` pads every item to the batch max
(`common.py:251-265`), so batch composition changes kernel shapes (whether the
padded kernels are bit-exact is unmeasured). F15 cannot rescue replay: the
journal records `input_hash`, not the input (`pii-abi/src/journal.rs:69-70`).
Consequence: P1 (Python head) and P4 (Rust head) can emit different labels for
the same state whenever the top-two margin is below accumulated drift —
exactly the flat 11+-option distributions the 255-option Choice advertises; a
journaled run re-routes on replay, so "tune thresholds without silently
rerouting" silently reroutes.
Fix: record the tie margin (top1−top2 logit gap) in every payload; route
margins below a floor to the escalate/catch-all port; journal provenance with
every decision (laya version, head hash, backbone quant+hash, batch shape);
make the parity oracle decision-level (label agreement + margin histogram over
a corpus), not Δp≤0.001.

**B2 — F5 quotes a temperature the runtime refuses to apply; a Rust port following F5 defeats the confidence gate.**
Evidence: `common.py:224-240` — `TEMP_MIN=0.5`, `TEMP_MAX=5.0`, NaN→1.0; the
docstring is explicit that an unclamped 0.1006 "multiplies [logits] ~10x: a
0.24 top probability is published as 0.99". `agent.py:205-220` clamps every
config temperature at load and emits a `RuntimeWarning`: *"Treat confidence
from the affected buckets as uncalibrated."* DESIGN F5 quotes the raw values
verbatim ("read the config, never hardcode") and §8's port spec applies them
with no clamp step.
Consequence: the Rust head sharpens 11+-option logits 10× and publishes coin
flips as 0.99 confidence — which is precisely what the L4/L6/L7 gate levels
consume. The design's core safety invariant ("gate on `confidence`") is
defeated by its own port spec.
Fix: port `clamp_temperature` verbatim; surface clamped buckets in the answer
payload and the ledger; treat affected-bucket confidence as uncalibrated.

**B3 — P1 is unbuildable as sequenced: nothing in it runs the head or the tokenizer.**
Evidence: :8083 serves the **backbone only** (040 §1: "the decision head …
must run outside llama.cpp"); the parity harness is a Python `laya.load`
monkeypatched at the encoder, feeding HF token IDs to `/embedding`
(`parity_test.py:18-38`). DESIGN §10 P1 says "usable today; no Rust head
needed; proxying :8083" — but the head, tokenizer and `build_sequence` are
Python today, and no P1 component is specified to run them (§8's `laya-backbone`
is an HTTP client; there is no Python bridge).
Consequence: P1 either silently depends on the digital-marketing venv and
spawns Python per call — the fragile dependency P4 exists to remove — or P1
must absorb P4 to produce decisions at all.
Fix: name P1's head runtime (persistent Python sidecar pinned to a laya
version, or absorb the Rust head), and say which laya version P1 proxies.

**B4 — One redb store behind N stdio servers collides with redb's single-writer model.**
Evidence: DESIGN §3 — every consumer (Hermes, pi, Claude, hooks) spawns its
own `laya-mcp` process, all against one embedded store. redb is an
embedded, single-process database (of the two sibling projects the design cited, one pins `redb = "2"` and the other `redb = "1.0"`;
F10's claim that both use redb 2 is half wrong, and the majors differ in lock/repair behavior).
Consequence: a hook firing while an agent session holds the server is the
normal case, and the second process cannot open the same store for writing —
the "one binary, every consumer" architecture fails at first concurrent use.
Fix: choose a single-owner design before code (one daemon owning the store;
or per-client store files with a merge path; or a ledger-less stdio mode with
a separate CLI for calibration). Verify redb 2's cross-process error in a
one-line P1 test.

**B5 — The §5 confidence-band pattern is not expressible in pii-graph as it exists.**
Evidence: the expr language is deliberately tiny — "Only object member access
is supported … No arrays, no functions" (`expr.rs:1-8`, `eval` at :17-32 is a
pure key-walk; `eval_or_literal` at :39-44 passes non-`$.` strings through as
literals). `transform` therefore emits either a member value or a constant
(`nodes.rs:43-62`); `route`'s `eq` is exact `serde_json::Value` equality
(`nodes.rs:113`) — no ranges, no comparisons, no arithmetic, no conditionals.
No other builtin kind thresholds anything (`node.rs:66-74`:
transform/route/emit/wait/timer; pii-server's `gate` kind is human-approval
suspension, `registry.rs:47`).
Consequence: "transform → band = low|mid|high (thresholds in CONFIG, one
line)" — the pattern §5 calls "forced by F8/F9" and the basis of the L4/L6/L7
gate-as-port story — **cannot be built**. The only routes a graph can take on
confidence today are exact-value matches, which are useless for bands.
Fix: pick one and write it into the design: (a) the laya node emits the band
itself (the option §11 Q4's "lean: transform" rejects — the lean is choosing
the impossible one); (b) a new threshold/band node kind; (c) extend the expr
language. All three are pii changes, contradicting §11 Q1's "no pii-graph
change" posture.

### MAJOR

**M1 — "We inherit the safety net for any diamond a decision feeds" is false for cyclic graphs, including the design's own showcase.**
Evidence: V6 is real and does catch two branches of one router feeding an
`All` join (`validate.rs:250-367`, message at :354-357). But the caller skips
it when V3 found a cycle (`validate.rs:101-108`), and `v6_unsatisfiable_join`
itself silently returns `None` whenever its Kahn topological sort fails to
cover all nodes (`validate.rs:331-333`) — i.e. in **any** graph with a
data-edge cycle, including a legal budgeted one. `failover.toml` — the §5
item 2 upgrade target — has the retry cycle (`failover.toml:28`, retry edge
:135-139). V6 also checks `JoinPolicy::All` only (`validate.rs:336`).
Consequence: a Laya-fed diamond added to the failover ladder deadlocks at run
time ("no progress: node … never became ready", `run_helpers.rs:181`) with no
static catch — the exact case §5 advertises as inherited safety.
Fix: state the acyclicity condition in §5; either make V6 cycle-tolerant
(propagate constraints through budgeted back-edges conservatively) or require
model-fed-graph graphs to use `any`/`allsettled` joins downstream of routers.

**M2 — The latency story mixes numbers from three different paths, and the hook path is the worst one.**
Evidence: "~0.3 s CPU" (§6, §9.3) is 039's **in-process PyTorch** measurement
(039 §6.2: 220 ms warm, 307.9 ms steady median). The path the design actually
builds (HTTP backbone) measured **608 ms** in 040 §1, which also says: "Do not
claim a speed win for the hybrid on CPU … measure before asserting speed."
Hooks-as-subprocess (§3 point 4) add process spawn + 106 MB fp32 head load per
call (039 §6.2 measured **23.0 s** cold load for the Python stack; Rust head
load time is unmeasured anywhere). §6's "Cheap enough for EVERY tool call"
contradicts §9.3's own risk 3 and 040 §3.5.4 ("batch questions, don't call it
per tool call").
Consequence: a PreToolUse hook pays 0.6 s+ per tool call optimistically, plus
cold start; the design's stated mitigation (`ask_files` batches) addresses a
different problem entirely.
Fix: a per-integration-point latency budget measured on the real path; a
persistent server/daemon for the hook lane; resolve the §6 vs §9.3
contradiction in the text.

**M3 — The head port spec (§8) and F2 are incomplete and partly wrong; "read the config, never hardcode" is impossible for the most load-bearing constant.**
Evidence (header read of `laya-head.safetensors`, 36 tensors):
- F2 omits all four `act_head` tensors (`act_head.0.weight [256,1028]`,
  `act_head.0.bias [256]`, `act_head.2.weight [2,256]`, `act_head.2.bias [2]`)
  while **F1's own 26,512,134-param count includes them** (263,938 params;
  26,248,196 without — arithmetic verified against the architecture).
- F2's "scorer.0 [1024,1024]" is wrong: scorer.0 is a **LayerNorm** of shape
  [1024] (`common.py:100`; [1024,1024] is scorer.1), and §8's scorer line
  ("Linear→activation→Linear") omits the leading LayerNorm.
- `nhead = max(1, d//64) = 16` is computed in code, **not in the config**
  (`common.py:96`) — so F5's rule cannot be followed for the single constant
  attention needs most.
- §8 lists `max_prefixes 6`, which is **unused by the inference path** (grep
  across the installed package: no reference).
- Logic outside the weights is unenumerated: `src_key_padding_mask`
  (`common.py:111-113`), marker gather + clamp (:114-115), `-1e4`
  `masked_fill` (:117), `type_emb` broadcast add (:109), `norm_first=True`
  pre-LN (:97), act-head features (softmax, top2, gap, entropy, `k/255`,
  pooled `h[:,0]`, :119-135), GELU variant and LayerNorm eps (unspecified),
  and `build_sequence`'s constants — per-option 48-token cap, 16/8-token
  floors, the `"<type> question: "` prefix, mask-token stripping, right-side
  state truncation (`common.py:49-86`), collate padding (:247-280).
Consequence: a port faithful to F2/§8 produces wrong logits and broken
confidence; `act_probability` silently vanishes from the tool contract
(breaking callers written to laya's documented shape, `agent.py:335`).
Fix: rewrite §8 as the actual forward pass with the 36-tensor inventory,
nhead, eps, GELU variant, masking constants and prompt-construction
constants; decide `act_head` explicitly (port it, or drop the field *and* the
params from F1).

**M4 — The cited parity oracle cannot certify the Rust port.**
Evidence: the harness keeps tokenizer + `build_sequence` + head in Python and
swaps only the encoder (`parity_test.py:18-38`), sends HF token IDs
(:34), and compares **one** probability on **one** state (:50).
Consequence: (i) Δp≤0.001 passes while labels flip on near-ties (B1);
(ii) the Rust port's own tokenizer and prompt construction — the part that
actually ships — is never exercised by the oracle; (iii) F7's number
quantifies Q8_0 backbone quantization, not Rust fidelity; (iv) with act_head
dropped, `act_probability` is incomparable.
Fix: test the Rust binary end-to-end from **text** (its own tokenizer, its
own `build_sequence`) against a pinned Python reference over a corpus,
reporting label agreement + margins + ECE, plus targeted near-tie and
11+-option cases.

**M5 — The rules-first ordering from 039/040 is silently dropped; the failover "upgrade" can put a probabilistic model in front of a deterministic classifier.**
Evidence: 039 §4 F5 — "a probabilistic model must not sit in front of
deterministic classification that is already correct and free" (FailoverReason
is derived in code from HTTP facts; `failover.toml:34-43` routes on it);
039 §3.1 J3 — "the static rule matches first … [the model] slots behind that
rule, not in front of it"; 040 §3.5.1 — "Rules first, model second. Laya
handles the ambiguous middle, never the hard rule." DESIGN §5 item 2 ("Laya
*refines* an ambiguous reason … turning a heuristic bucket into a calibrated
one") and the §5 route sketch (model-fed rules only, no static rules matching
first) state no ordering.
Consequence: a Laya verdict in front of `FailoverReason` replaces a free,
correct, deterministic classification with a probabilistic one — the exact
regression both prior docs rejected; the L4/L6 gates also lose the
deterministic-first defense 040 §3.3 requires against hostile state.
Fix: write the ordering invariant into §5/§7: static rules match first; the
model classifies only the unresolved remainder; every gate names its
deterministic companion control.

**M6 — `route` as an MCP tool contradicts the design's own thesis; the tool surface also inherits pii-mcp's error-as-success pattern and blanket capability grant.**
Evidence: DESIGN §4 L5 ("route returns a route") vs §5 ("Laya **never**
executes control flow") and 039 F4 / 040 §3.2 ("may only select among ports
that already exist"). As a tool, its rules come either from the caller at
call time (the caller owns control flow, unjournaled) or from config (a graph
query duplicated outside the journal). Meanwhile `pii-mcp` returns tool
errors as **successful** results with an `"error: {e}"` text prefix
(`main.rs:75-82`; no isError, no protocol error surface), and the dispatcher
grants workspace-wide `FS_READ|FS_WRITE|PROC|MEMORY` on every call
(`main.rs:62-70`).
Consequence: an agent calling `route` selects edges outside any graph — the
injection failure mode the whole design exists to prevent; a dead llama.cpp
makes `gate` return "error: …" as a success (fail-open by accident); `ask_file`'s
read scope is undefined (workspace? `/`? `/etc`?).
Fix: drop `route` from the MCP surface (or reduce it to a pure
`match(value, rules)` helper under a non-graph name); define per-tool
least-privilege scopes; use the protocol's error surface; specify fail-closed
for `gate` and a typed error envelope for `decide`.

**M7 — The language and state-quality caveats 039/040 recorded are dropped, and they break the design's own L8/L9/gate levels.**
Evidence: 039 §6.4 — Khmer at **0.000 accuracy / 0.952 confidence** (the
English checkpoint collapses on non-Latin scripts and reports high
confidence); thin vs rich state: the same German request scored 0.49 on a
bare sentence and 0.9995 with a full envelope; 040 §3.5.3 ("State quality,
not language, is the main degrader"). DESIGN §4 L8/L9 feed file content as
state; §7 mentions neither. The design's head is the English root checkpoint
(parity vs `convaiinnovations/laya`; multilingual siblings exist,
`agent.py:110-144`), and no Router/script detection is in scope.
Consequence: confidence-gated automation on non-Latin file content is
*confidently wrong* — the gate certifies garbage; thin file slices produce
low-confidence noise.
Fix: script detection (or a non-Latin guard) before any gating decision on
file content; state-shaping guidance per tool; hostile-input cases in the live
suite.

**M8 — ask_files' real failure modes are unexamined; the sharpest is silent truncation, and it collides with §7's no-secrets promise.**
Evidence: `build_sequence` truncates state to `room = max_len − len(ids) − 1`
tokens (`common.py:82-85`; max_len 512) — a multi-MB log becomes the first
~300 tokens with **no signal anywhere**; binary files tokenize to garbage;
options exceeding the marker budget raise (`agent.py:287-288`). §7's injection
mitigation ("files are marked as DATA") has no operational counterpart:
`build_sequence` has no data/system separation — the state is one serialized
blob (`common.py:15-18`).
Consequence: decisions are made on the first ~2 KB of each file — exactly
where a hostile file's prompt injection lives; secrets read from `.env`
travel to the backbone and, if journaled for replay (F15), into the redb store
that §7 promises holds no secrets.
Fix: the tool contract must state truncation policy and record tokens used
per file; refuse or flag binary; define a read scope; define what is
persisted (state hash, not state — or an opt-out); treat file content as
untrusted input to a model whose "state is not treated as hostile"
(039 §2, 040 §3.5.5).

**M9 — Provenance/versioning is absent, and it already drifted during the design's own lifetime.**
Evidence: DESIGN §12.4 cites `agent.py` as **v0.3.4** and F3/F4 cite 0.3.4
line numbers (:316/:295/:304/:309) — the venv now has **laya 0.3.5**
(`laya-0.3.5.dist-info`, installed 09-22) and the cited lines are
:341/:320/:329/:334 (I verified every behavioral claim against 0.3.5 — they
hold). 039 §4 F6 said "pin the version"; the design has no pinning, no head
hash, no backbone-quant provenance in the ledger.
Consequence: parity, calibration (`calibrate`/ECE) and replay are all
measured against a moving baseline; a `pip install -U` silently rewrites
what every threshold means.
Fix: pin laya version + head safetensors hash + backbone file/quant in
config; stamp every ledger row; fail loudly on mismatch.

**M10 — The loop between the lanes terminates as a run failure, not an escalation; the design never says what exhaustion looks like.**
Evidence: any Laya→LLM→Laya cycle needs `loop_budget` (V3,
`validate.rs:201-248`); the budgeted node's Nth execution is an
unconditionally terminal failure (`sched/state.rs:221-236`; the
`failover.toml:20-28` comment: budget=N buys N−1 meaningful executions). §6
patterns 3/4 route low confidence into the LLM lane with no exhaustion
semantics.
Consequence: the "uncertainty work queue" pattern, wired as a cycle, ends in
`loop_budget exceeded for node '…'` — a run failure, not an escalate edge;
there is no port on the budget-exhaustion path to hang `ask_human` semantics
on.
Fix: state the topology (acyclic per decision; one Laya verdict per branch)
or design the exhaustion path explicitly (e.g. a pre-route counter transform
that routes to escalate *before* the budget's terminal run).

### MINOR

**m1 — The proposed catch-all static check (§11 Q3) is not well-defined as specified.**
Evidence: `on` is a free-form expr string (`nodes.rs:66`; `expr.rs:17-32`);
"model-fed" is not derivable from the route node (payloads are untyped
`serde_json`; validate has no payload provenance). The prefix rule "on starts
with `$.payload.label`" over-matches `$.payload.label_v2`, and **misses the
design's own `$.payload.band` route** (§5), whose sketch shows no catch-all
either.
Consequence: the check under-catches the exact cases §5 draws; it cannot be
implemented faithfully.
Fix: require a no-`eq` fallback on **every** route — trivially implementable
by re-reading `cfg.rules` exactly as `RouteFactory::ports` does
(`node.rs:179-201`) — or add an explicit cfg flag (`model_fed = true`) that
triggers the requirement. Both are well-defined; the prefix version is not.

**m2 — Chaining route → transform(band) → route silently drops payload fields.**
Evidence: `route` passes `input.payload` unchanged (`nodes.rs:125-129`) —
fields survive route→route; but a transform's output **replaces** the payload
(`nodes.rs:46-55`: `Next{ok, eval(expr)}`), so the band transform must re-emit
`label`/`confidence` or downstream routes cannot read them.
Consequence: the §5 sketch only works if the transform repackages; and §5's
payload already contains `band` while §11 Q4 leans "transform-computed band" —
the design is internally inconsistent about where band comes from (see also
B5: the transform cannot compute it at all).
Fix: one worked example of the payload contract at each hop; resolve the band
question.

**m3 — "No pii-graph change" is technically true and practically misleading.**
Evidence: the `agent` kind is registered by **pii-server**'s engine registry
(`registry.rs:36-61`), not a pii-graph builtin (`node.rs:66-74`:
transform/route/emit/wait/timer); a `laya` kind needs a new `NodeFactory`
registered in *both* `registry()` and `lint_registry()` (`registry.rs:80+`);
the `tool` node kind can only call `ToolSet::builtin` tools
(`registry.rs:44-46`) and the agent node's toolset is builtin-only
(`agent/mod.rs:168-173`) — nothing in pii today can call an external MCP
server from a graph node.
Consequence: §11 Q1's lean hides a pii-server change (or an agent-node bash
hack that puts an LLM inside a deterministic decision's path). "Both lanes
return `Outcome::Next{port, value}`" *is* true at the trait level
(`node.rs:15`; `pii-abi/src/node.rs:43-50`; the agent node returns Next on
ok/fail/budget/err, `agent/mod.rs:111-150`) — but the work is a factory,
`ports()`/`required_caps()`/`effects()` (a net-calling node is not PURE), and
two registry insertions.
Fix: name the real seam and its crate in §11 Q1.

**m4 — `noul`'s confidence is a different quantity than choice/score's.**
Evidence: noul confidence = `max(p, 1−p)` (`agent.py:360`); choice/score
confidence = `1 − H(p)/log(k)` (`common.py:210-216`).
Consequence: the same band threshold means different things per question
type; a global band policy (§11 Q2) mixes them.
Fix: per-type thresholds (or normalize) in `policy.rs`.

**m5 — F4's "can NEVER change the EDGE" is false wherever a route matches a score- or band-derived value.**
Evidence: expected score `Σ i·p_i` (`agent.py:347`) and confidence
(`common.py:210-216`) are both computed from temperature-scaled
probabilities; §5's own second route matches on `$.payload.band`.
Consequence: tuning temperature changes the score/band → changes the edge;
the separability claim holds only for label routes.
Fix: scope F4's consequence to label routes; document that band/score routes
are temperature-coupled by design.

**m6 — Tool-name collisions with existing pii node kinds.**
Evidence: pii-server already has a `gate` node kind (human-approval suspend,
`registry.rs:47`) and pii-graph a `route` kind (`node.rs:69`); the design's
MCP tools carry the same two words with different semantics.
Consequence: skills/hook docs will conflate a model verdict with an approval
suspend, and a tool with a graph node.
Fix: rename the tools (`laya_gate`, …) or drop `route` (M6).

**m7 — `calibrate` has no ground-truth source.**
Evidence: ECE needs (confidence, correct) pairs (`common.py:197-207`);
§8's `calibrate.rs` says "ECE from the ledger", but the ledger records
decisions, not outcomes. No tool or hook records whether a decision was
right.
Consequence: calibration is incomputable as designed; the thresholds it is
supposed to tune have no feedback signal.
Fix: define the outcome-recording path (who labels a decision, when, with
what tool) before P5.

### NIT

**n1** — F2's scorer.0 shape; the "36 tensors" count that 040 recorded but the design dropped (header verified: 36).
**n2** — §8 lists `max_prefixes 6`; it is unused by the inference path (grep exit=1).
**n3** — `hybrid_test.py:13` points at `localhost:8099` while the serving container is :8083 (040 §2) — harness drift the design never reconciles.
**n4** — The Python ground truth lives in another project's venv (`another project's virtualenv`) — the same venv that produced the tokenizers/transformers conflict F1 cites; the design's evidence base is a side effect of an unrelated project.
**n5** — F10's claim that two sibling projects use redb 2: one pins 1.0, the other 2.

---

## 3. Corrections to the design's own "verified facts" (F1–F15)

- **F1 — partly wrong.** The 26,512,134-param count **includes** the 263,938-param `act_head` that F2 omits (26,248,196 without; verified by arithmetic and by the file header). "106.0 MB fp32" ✓ (106,052,840 bytes). "2 layers" ✓. "Can run in Rust with no Python/torch" is true for the tensors, but the port also needs the **tokenizer** and `build_sequence`, which F1 doesn't mention.
- **F2 — wrong/incomplete.** Omits `act_head.0.weight [256,1028]`, `act_head.0.bias [256]`, `act_head.2.weight [2,256]`, `act_head.2.bias [2]`; "scorer.0 [1024,1024]" is wrong — scorer.0 is a LayerNorm of shape [1024] (`common.py:100`); [1024,1024] is scorer.1. Total is 36 tensors. "No exotic ops" glosses gather/topk/entropy/masked_fill and the in-code nhead=16.
- **F3 — correct in substance, stale in citation.** Argmax verified at `agent.py:341` (design says :316); no sampling anywhere (grep exit=1). "No seed pinning needed for replay" holds only within a fixed implementation+version — not across head ports or backbones (see F7's own Δp, and B1).
- **F4 — correct math, wrong consequence.** Temperature scales logits before softmax (`agent.py:330-332`); argmax is invariant for T>0 (clamped to [0.5, 5.0], `common.py:228-240`) ✓ **for label routes**. "Can NEVER change the EDGE" is false for band/score routes (m5) — including §5's own second route.
- **F5 — correct that the config travels in the file (verified in the safetensors metadata), dangerously incomplete otherwise.** It quotes raw temperatures including `choice:11+ = 0.101`, which laya **refuses to apply** (clamps to 0.5, warns "uncalibrated", `agent.py:205-220`); and the most load-bearing head constant (nhead=16) is not in the config at all (`common.py:96`) — "never hardcode" cannot be followed.
- **F6 — verified live today.** `curl http://127.0.0.1:8083/health` → `{"status":"ok"}` (2026-09-29). ✓
- **F7 — real number, wrong inference.** Δp=0.0006 is one probability, one state, same Python head, different backbone (`parity_test.py:50`). It quantifies Q8_0 quantization, not Rust-head fidelity. "The Rust head port is a fidelity question" is true — F7 is not evidence for it.
- **F8 — accurate for THIS checkpoint, overstated as a fact.** 039 §6.5 measured act_probability ≡ 1.0 including gibberish; but if the Rust port drops act_head (as F2's inventory implies), the field vanishes and the tool contract diverges from laya's documented response (`agent.py:335`). Decide explicitly.
- **F9 — fine.** Saturation measured; flag-in-ledger is the right mitigation. Note unclamped sharpening (B2) manufactures fake saturation.
- **F10 — half wrong.** rmcp 3.2 ✓ (`pii-mcp/Cargo.toml:19`); llama-cpp-2 in pii-model ✓; redb: one sibling pins 2 ✓, **the other pins 1.0** ✗.
- **F11 — accurate.** Verified `nodes.rs:64-131` (parsing :75-101; hard error :118-123; payload travels :125-129). `eq` is exact `serde_json::Value` equality (:113) — string labels and bands work; no coercion (float eq is fragile, but the design uses strings).
- **F12 — accurate.** `find_map`, first match wins, exactly one port (`nodes.rs:112-117`). ✓
- **F13 — overstated.** V1 exists (`validate.rs:67-75`) and checks edges against rule-declared ports (`node.rs:179-201`) — but it says nothing about labels, and the failure it cannot see is a rule port with **no outgoing edge**, which is only a V5 *warning* (`validate.rs:169-192`): the catch-all nobody wired routes into a silent dead end, not an error. The null/missing case IS caught by a no-`eq` fallback (verified: eval returns `Ok(Null)` on unresolvable paths, `expr.rs:26-28`; Null ≠ any string eq) — but only because the fallback exists, which is exactly what nothing enforces (m1).
- **F14 — overstated.** V6 exists and catches two branches of one router feeding an `All` join — **in acyclic graphs only**. Skipped when V3 found a cycle (`validate.rs:101-108`) and silently returns None whenever its own topological sort fails (`validate.rs:331-333`), i.e. in ANY graph with a data cycle — including `failover.toml`, the design's named upgrade target. All-joins only (`validate.rs:336`).
- **F15 — overstated.** The journal is real (append-only, seq, `journal.rs:1-45`) and `NodeFinished` records the emitted value (`pii-abi/src/journal.rs:86-100`) — runs are **auditable**. But `Started` records `input_hash`, not the input (:69-70), so a journal alone cannot re-execute a run, and any graph containing the agent lane is not re-executable at all. "Replayable/auditable" ≠ "re-executable"; the design uses the strong word.

**Meta-correction**: §2 says "All measured in this workspace, 2026-09-24" and §12.4 cites v0.3.4 with 0.3.4 line numbers, while the venv has laya **0.3.5** (installed 09-22). The behavioral claims all hold in 0.3.5 (I verified each), but several "measurements" were inherited from 039/040 rather than re-taken, and the version moved under the design.

---

## 4. What the design must add before implementation

1. Tie margin in the payload; margin-floor rule routing near-ties to escalate; decision-level parity oracle (label agreement + margin histogram, from **text**, not token IDs). (B1/M4)
2. `clamp_temperature` in the port spec; clamped-bucket flag in answers and ledger. (B2)
3. P1's head runtime named (Python sidecar pinned to a laya version, or absorb P4). (B3)
4. redb ownership model for N stdio consumers + the one-line cross-process test. (B4)
5. An implementable band mechanism — pick (a) laya emits band, (b) new node kind, (c) expr extension — and correct §11 Q4's lean. (B5)
6. Rules-first ordering invariant; each gate names its deterministic companion. (M5)
7. Latency budget per integration point, measured on the HTTP path; persistent-server decision for hooks; fix §6 vs §9.3. (M2)
8. The full 36-tensor forward pass in §8 (act_head decision, nhead, eps, GELU variant, −1e4 mask, gather, padding mask, type_emb add, act-head features, `build_sequence` constants, collate padding); drop or mark `max_prefixes` unused. (M3)
9. Tool contracts: per-tool input schemas, typed error envelope, isError usage, fail-closed for `gate`, least-privilege read scope for `ask_file(s)`, truncation/binary/secrets policy. (M6/M8)
10. Provenance and pinning: laya version, head hash, backbone quant+hash in every ledger row; mismatch = loud failure. (M9)
11. Language/script guard + state-shaping guidance for file-content states; hostile-state cases in the live suite. (M7)
12. Ledger schema reconciled with §7's no-secrets promise (state hash vs state; retention). (M8)
13. The DAG seam named precisely: new `NodeFactory` in pii-server `registry()` + `lint_registry()`, ports/caps/effects for the `laya` kind; cycle-exhaustion semantics for the two-lane loop. (m3/M10/M1)
14. Catch-all enforcement: uniform fallback rule required on every route (well-defined), replacing the prefix heuristic. (m1)
15. Outcome-recording path for `calibrate`. (m7)
16. memory-consolidation (040 L3) and the pre-dispatch tier gate (040 L1): in scope or explicitly descoped — the design claims to land on the prior trail but reorders it silently.
17. Which checkpoint the head is (English root?) and what happens with the multilingual siblings (`agent.py:110-144`).
18. Discovery/wiring: where the binary lives, how Hermes/pi/Claude/hook configs register it, version pinning.

---

## 5. What is genuinely good

- The seam is right, and it is the seam our own prior research chose: decision as `$.payload`, `route` branches, graph owns every edge (039 J1 / 040 §3.2) — verified against `nodes.rs`, and proven in production by `failover.toml`, whose catch-all `{port="secondary"}` is exactly the pattern the design generalizes.
- F3 is a real and useful observation: the decision path is argmax with no sampling (verified), and within a fixed build it is genuinely deterministic.
- F4's separability insight (temperature tunes confidence, not the label) is correct where it applies and worth keeping once properly scoped (m5).
- The MCP-stdio choice and the spec-22 bounded-lifetime pattern are correctly identified and copyable (`main.rs:144-158`: `stdio()` → `serve_server` → `running.waiting()`); rmcp 3.2 + transport-io is already in-tree.
- Phasing the Rust head (P4) after the levels that pay is the right sequencing instinct; the offline-mock/live-suite test split (§8) is the right test shape.
- Two-lanes-never-one is the correct posture, matches both the reference design's framing and 039/040's conclusion that the model handles the ambiguous middle.
- F6 was true when written and is still true today (verified live).

---

## 6. Open questions I could not resolve from the files given

- **All reference-design-specific content** — the ten levels' details, the $3,500/$525/$16.80 cost ladder, the 0.35-confidence "reversible" case, the 201-test offline suite, the L6 bash-heredoc bypass: the design itself records that the transcript could not be fetched (HTTP 429) and relies on the source's published description. I could not verify any of it from the files given.
- **llama.cpp determinism across thread counts/builds** for the `/embedding` path (relevant to B1): not determinable from the files; worth a two-build, two-thread-count measurement before relying on replay.
- **redb 2.x exact cross-process behavior** (lock error vs repair): strongly believed to be exclusive-lock (B4), but I did not run the test; it is a one-liner in P1.
- **Which checkpoint `_tmp/laya-gguf/laya-head.safetensors` came from**: implied English root by the parity comparison, but not recorded anywhere I was given; the config's `model_name: rl-agent` does not disambiguate.
- **Rust head load time** — the cold-start number that decides the hook lane (M2): unmeasured anywhere in the workspace.
- **Whether pi/Hermes hooks can practically drive an MCP stdio server** or need the `laya-cli` one-shot surface: pi's hook configuration surface was not part of the files given.
- **act_head behavior on future/multilingual checkpoints** (F8's permanence): unknowable from the files.
