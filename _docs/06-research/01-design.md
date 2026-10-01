# laya-rs — design (single source of truth)

**Status**: DESIGN UNDER DISCUSSION. No code yet.
**Created**: 2026-09-24. **Consolidates**: the four amem entries listed in §12.
**Trigger** (user, verbatim): *"i want you to create a project using laya, rust and
hermes exercising all the concepts in the video! gather the information in a
centralized place! laya is free ro run local, doenst need a gpu, a must in! those
json can be kept into a redb database, the skills, hooks could call a rust
solution! lets think about it!"* · *"could be a stdio json-rpc (mcp stdio
compatible)"* · *"also it can drive dags! our graphs became fully deterministic"* ·
*"it should be used together with a llm solution!"*

---

## 1. Thesis

> IndyDevDan's ten levels assume a **paid cloud System-1 model** and an OpenRouter
> key. We substitute **Laya on our own llama.cpp** — same three question types,
> same calibrated-probability contract, **R$0 per call, no egress, no GPU** — and
> expose it as **one Rust binary speaking MCP over stdio**, so Hermes, pi, Claude
> Code, our skills and our hooks all consume the *same* tool surface. It supplies
> the **value** an existing `route` node matches on, so **the DAG owns every edge
> and the whole execution becomes deterministic and replayable**. It is used
> **together with** the LLM lane, never instead of it.

---

## 2. Verified facts that shape the design

All measured in this workspace, 2026-09-24. These are load-bearing; each one
closes an option or opens one.

| # | fact | measurement | consequence |
|---|---|---|---|
| F1 | **The head is pure linear algebra** | `laya-head.safetensors`: **26,512,134 params, 106.0 MB fp32, 2 layers** | the head can run **in Rust with no Python/torch** — also sidesteps the `tokenizers==0.22.2` vs `transformers>=0.23.1` conflict that already bit us |
| F2 | Head tensor inventory | `in_proj_weight [3072,1024]`, `out_proj [1024,1024]`, `linear1 [4096,1024]`, `linear2 [1024,4096]`, `norm1/norm2 [1024]`, `scorer.0 [1024,1024]`, `scorer.1 [1024,1024]`, `scorer.3 [1,1024]`, `type_emb [3,1024]`, `temperature [3]` | only attention, LayerNorm, GELU, matmul, softmax — **no exotic ops** |
| F3 | **The answer is `argmax`, not a sample** | `laya/agent.py:316` `"choice": keys[int(p.argmax())]`; no `multinomial`/`torch.rand` in the decision path | **the label is deterministic**: same state → same argmax → same port. No seed pinning needed for replay |
| F4 | **Temperature scales probabilities, and argmax is invariant under monotone scaling** | `agent.py:295` `softmax(act,-1)`; `:304` `temperature_by_options.get(temp_bucket(qt,k), temperature[qt])`; `:309` `confidence_from_probs` | **tuning temperature can change the CONFIDENCE BAND but can NEVER change the EDGE.** Route stability and calibration are separable — we can tune thresholds without silently rerouting the graph |
| F5 | Head config travels inside the file | `laya.config` metadata: `head_max_len 192`, `max_prefixes 6`, `temperature [1.637, 1.251, 1.983]`, `temperature_by_options {choice:2:1.906, choice:3-5:1.760, choice:6-10:1.000, choice:11+:0.101, noul:2:1.983, score:3-5:1.251}`, `encoder answerdotai/ModernBERT-large` | **read the config, never hardcode** |
| F6 | Laya is serving **now** | container `llamacpp-laya` :8083, `/models/laya-Q8_0.gguf`, `/health` ok, backbone `--embeddings --pooling none` | Rust talks **HTTP `/embedding`** — no FFI needed |
| F7 | Parity already proven | llama.cpp+head vs pure PyTorch: `0.9670` vs `0.9664` → **Δp = 0.0006** | the Rust head port is a *fidelity* question, not a feasibility question |
| F8 | `act_probability` is **inert** | exactly `1.0` on everything, including gibberish | **gate on `confidence`, never on `act_probability`** |
| F9 | Saturation occurs | one `choice` returned exactly `1.000/0.000/0.000` | flag saturated distributions as **suspect**, never as high confidence |
| F10 | Every dependency is already in our stack | `rmcp 3.2` (`transport-io`) — `pi-improved/crates/pii-mcp`; `redb 2` — `amem`, `pidag`; `llama-cpp-2 0.1.156` — `pii-model/src/llamacpp.rs` | **this project adds glue, not infrastructure** |
| F11 | `route` is already a deterministic port selector | `crates/pii-graph/src/nodes.rs:64` — `on` (expr) + `rules` (`{eq, port}`); `:79/:84/:91/:97` validation; `:121` hard error on no match; `:108` payload travels on the chosen port | a System-1 decision is **just another source for that value** |
| F12 | `route` returns **exactly one** port | `rules.iter().find_map(...)`, first match wins | **no fan-out**; diamonds need `JoinPolicy` |
| F13 | The "invented label" failure is impossible, twice | author time **V1** (`node has no port`) + run time hard error `"route: no rule matched value … and no fallback"` | with a **mandatory catch-all**, a model-fed route cannot produce an undefined edge — **stronger than the video claims for Jev** |
| F14 | The routed-diamond deadlock is already caught statically | **V6** in `validate.rs:250` — "join can never be satisfied: X and Y are on mutually exclusive ports of Z" | we inherit the safety net for any diamond a decision feeds |
| F15 | The journal makes runs replayable | `crates/pii-graph/src/journal.rs` — append-only `NodeStarted/Finished/Failed/Dead` + `seq` | decide → payload → route → **journal** = a re-executable run |

---

## 3. Architecture

```
   Hermes ─┐
   pi ─────┤  laya-rs  (ONE Rust binary · MCP stdio · JSON-RPC 2.0 on stdin/stdout)
   Claude ─┤    tools: decide | ask_file | ask_files | route | gate
   skills ─┤           | should_compact | ask | ledger | calibrate
   hooks ──┘                        │
                ┌───────────────────┴────────────────────┐
                │                                        │
        redb (embedded)                     HTTP :8083 /embedding
        ├── decisions log (append-only)      (llama.cpp, modern-bert)
        ├── calibration stats                        │
        ├── per-tool budgets             Laya head IN RUST
        └── policy/threshold overrides   (26.5M params, 106 MB, in-process)
```

**Why MCP stdio** (user's call, and it is correct):
1. **One implementation, every consumer.** Hermes, pi, Claude Code and any
   skill-reading agent already speak MCP — we do not write four integrations.
2. **No network surface.** No port, no auth, no TLS, nothing to expose; works in a
   container with no inbound access (our standing constraint).
3. **`rmcp 3.2` + `transport-io` already runs in `pii-mcp`** — copy the pattern,
   including the spec-22 bounded-lifetime fix (`stdio()` → `serve_server(server,
   (r,w))` → `running.waiting()`), so the server exits on EOF instead of parking
   forever on `pending()`.
4. **Hooks call it as a subprocess; agents call it as a tool server — same binary.**

**Why redb** (user's call): embedded, single-file, ACID, pure Rust, no server;
already used in `amem` and `pidag`; and it is exactly the user's instinct — every
decision is a JSON document, keyed and queryable.

---

## 4. The ten levels → tools

| L | video concept | surface | invariant |
|---|---|---|---|
| 1 | Single decisions | `decide` (a `noul`) | **threshold lives in the caller** |
| 2 | Multiple choice | `decide` (N questions, ONE call) | **fan-out**: ask everything, then choose which answers to use |
| 3 | Composite scoring | `decide` (several `score`s) | **weights stay in the caller** — Laya never sees them |
| 4 | Confidence gating | `gate`; 0.5 / 0.9 bands | **gate on `confidence`, never `act_probability`** (F8) |
| 5 | Intent/model routing | `route` returns a *route* | **not control flow** — the graph stays the authority |
| 6 | Guardrail hooks | `gate` shaped for `tool_call`/`tool_result` | **a model gate is ONE SIGNAL** (§7) |
| 7 | Should I compact | `should_compact`, 4 questions/turn | **token thresholds stay in code** |
| 8 | Cheap reads | **`ask_file`** | Rust reads the file; **it never enters agent context** |
| 9 | Files at scale | **`ask_files`** | glob + prune + 255 cap + **parallel** + `pick_first` |
| 10 | Agentic | **`ask`** | **the agent writes the questions**; its command passes the L6 gate first |

**Deliberate omission:** the video's *presentation* layer (Vue lab, animated hero
SVGs, agent-window UI) is **out of scope** — it exists to teach, not to decide. Our
equivalent is `ledger` + the redb log: observability and cost-per-decision without
a web app.

---

## 5. DAG integration — "our graphs became fully deterministic"

Laya **never** executes control flow. It supplies the **value** an existing
`route` node matches on. The DAG remains the sole authority over edges.

```
laya node → payload { label, probabilities, confidence, band }
    ↓
route  on = "$.payload.label"
       rules = [{eq:"bug_in_code", port:"fix_code"},
                {eq:"wrong_test",  port:"fix_test"},
                {eq:"environment", port:"check_env"},
                {port:"escalate"}]            ← CATCH-ALL MANDATORY
    ↓
ordinary deterministic nodes per port
```

**Why this is fully deterministic:** every branch taken is now an explicit, typed,
inspectable edge chosen by a value we can log, replay and test — instead of an
LLM's prose judgement that nothing can constrain. **decide → payload → route →
journal** (F15) is a run you can re-execute and get the same answer.

**Confidence as a PORT, not a hidden multiplier** (forced by F8/F9):
```
transform → band = low|mid|high      (thresholds in CONFIG, one line)
route on = "$.payload.band"   →  low ⇒ ask_human   ← a REAL dead-end that halts
```
The video's measured case is exactly this: `rm -rf node_modules && npm install` →
"reversible" at confidence **0.35** → **asks a human**, because *"the pick is
plausible, the confidence is not."* Escalation is a **drawn edge**, not a hope.

**Five DAGs this unlocks in our work:**
1. **Test-failure triage** (video L10) — `choice` over {bug_in_code, wrong_test,
   environment, other} → four named edges instead of buried prose.
2. **Failover ladder, upgraded** — `graphs/starter/failover.toml` already routes on
   `$.payload.reason` with a catch-all `{port="secondary"}`. Laya *refines* an
   ambiguous reason (is that 500 a blip or a hard billing failure?), turning a
   heuristic bucket into a calibrated one. Its `loop_budget=3` and deliberate
   `compress`/`exhausted` dead ends stay untouched.
3. **Model/harness routing** (L5) — one cheap call → one edge per harness.
4. **Guardrail gate as a port** (L4/L6) — blocked branch **terminal**, and never
   the only control (§7).
5. **Compaction point** (L7) — `route` on the chosen turn; thresholds stay on the
   transform.

---

## 6. Two lanes — Laya **and** the LLM

The video's own framing: *"This is NOT Jev versus LLMs. It's Jev AND your agents."*

```
LANE A — Laya : classify / gate / score / route.
                R$0, ~0.3 s CPU, no GPU, no egress. Narrow, typed, calibrated.
                Cheap enough for EVERY tool call. Never writes, never reasons
                open-ended, never invents an edge.
LANE B — LLM  : our `agent` node + the pii-abi FailoverReason chain
                (RateLimit / UpstreamRateLimit / Overloaded / ServerError /
                Timeout / ContextOverflow). Open work — code, prose, reasoning.
BOTH → `route` on a payload value. The graph owns every edge in both cases.
```

**The four composition patterns:**
1. **Narrow then act** (L5, highest value) — Laya picks *which* harness handles the
   work (+ ambiguity `score` in the same call) → one edge per lane. We stop paying
   a reasoning model to decide what a 300 ms classifier decides.
2. **Gate then act** (L4/L6) — Laya gates a tool call *before* the LLM sees the
   result; low confidence → human or second opinion.
3. **Triage the LLM's output** (L10) — the LLM runs the tests → Laya classifies →
   the LLM lane gets a **narrowed instruction** instead of "tests are red".
4. **Escalation is the contract** — Laya's low-confidence branch routes *into* the
   LLM lane. **Its uncertainty is the expensive lane's work queue.** That is what
   makes them "together" rather than one replacing the other.

**Cost context** (video's figures, per million yes/no calls): frontier **$3,500** →
workhorse **$525** → Jev **$16.80**. **Ours: R$0.** The combination is what makes
"a call on every tool call, every agent turn" affordable.

**Both lanes return `Outcome::Next{port, value}`** — the scheduler does not care
which lane produced the port. **The work is a node kind and a payload contract, not
a new engine.**

---

## 7. Guardrails — the video's own failures, inherited as requirements

- **A single-path gate gets routed around.** Level 6 option B blocked `.env` writes
  and the agent wrote the same file via a **bash heredoc**. His fix (a "this block
  is final" notice) he himself calls *"an instruction, not a control."*
  ⇒ `gate` returns a **reason**, is **one signal among several**, and the blocked
  branch is **terminal**.
- **The agent can read its own environment** — its bash printed every variable.
  ⇒ laya-rs never puts credentials in a tool result, and the redb store holds no
  secrets.
- **Level 10 gates its own command, not the agent's bash tool.** ⇒ documented;
  pair `ask` with a `tool_call` hook.
- **Saturation is a smell** (F9) ⇒ `answer.rs` **flags** saturated distributions
  in the ledger instead of reporting them as high confidence.
- **Unchanged across both lanes:** neither lane emits an edge; every port is
  declared by the graph author; every model-fed route has a catch-all; all
  thresholds live in config.

---

## 8. Crate layout

```
/projects/laya-rs/
├── crates/
│   ├── laya-core/       PURE — no MCP, no HTTP types
│   │   ├── head.rs      the 26.5M head in Rust (safetensors load, attention,
│   │   │                LayerNorm, GELU, scorer, per-type temperatures)
│   │   ├── questions.rs noul / choice / score prompt construction
│   │   ├── answer.rs    typed answers: value + probabilities + confidence + band
│   │   ├── policy.rs    thresholds, bands, weights — ALL caller-side, testable
│   │   └── calibrate.rs ECE from the ledger
│   ├── laya-backbone/   HTTP client to llama.cpp /embedding
│   ├── laya-store/      redb: decisions, calibration, budgets, overrides
│   ├── laya-mcp/        rmcp stdio server + tool router   ← THE BINARY
│   └── laya-cli/        terminal runner
├── specs/               exit criteria per feature (our SDD norm)
└── docs/                this design + the ten-levels mapping
```

**Testability rule** (from the video's own lab): `laya-core` runs its tests
**offline against a deterministic mock backbone**, so contracts are verified with
**zero model calls**; a separate live suite checks **meaning**. The video calls this
`just test` vs `just test-live`, and the split is right.

**What the Rust head must reproduce:**
```
backbone  POST :8083/embedding → hidden [seq, 1024]
head      type_emb[type] + head_max_len 192 + max_prefixes 6
          2 × (LayerNorm → self_attn 1024→3072→1024
               → LayerNorm → GELU MLP 1024→4096→1024)
          scorer: Linear(1024→1024) → activation → Linear(1024→1)
          softmax with temperature_by_options[<type>:<n>]
verified  parity vs PyTorch on the SAME questions → must be ≤0.001
          (we already have the harness: `_tmp/laya-gguf/parity_test.py`, Δp 0.0006)
```

---

## 9. Risks, named

1. **The head's exact forward order and which hidden state it consumes must be read
   from `laya/agent.py`, not guessed.** *Mitigation: the parity test is the oracle —
   if Rust disagrees with PyTorch by >0.001, Rust is wrong.*
2. **`ask_file` sends file content to Laya**, and a hostile file could try to
   manipulate the decision (the video warns *"state is not treated as hostile"*).
   *Mitigation: files are marked as DATA; a gate is never the only control.*
3. **~0.3 s/decision is fine per task, poor per tool call.** *Mitigation:
   `ask_files` batches in parallel; `ledger` makes the cost visible.*
4. **Scope — ten levels is a lot.** *Mitigation: the phasing in §10.*

---

## 10. Sequencing

| phase | deliverable | why | blocked on |
|---|---|---|---|
| **P1** | `laya-mcp`: `decide` (noul/choice/score) + `ledger` in redb, **proxying :8083** | usable *today*; no Rust head needed; proves the MCP surface | nothing |
| **P2** | `ask_file` / `ask_files` (L8/L9) | highest value per unit of work; batching fixes latency | P1 |
| **P3** | `gate` + `should_compact` (L4/L6/L7) | the levels we lack entirely | P1 |
| **P4** | **Rust head** replacing the Python path | removes torch/venv; self-contained binary | parity harness |
| **P5** | `route` + `ask` (L5/L10) + `calibrate` | agentic layer on proven ground | P1–P3 |

**P1–P3 need no head port and no new model work** — buildable against the running
container immediately. **P4 is the interesting research step**, deliberately after
the levels that pay.

---

## 11. Open questions (for the user)

1. **`laya` as a first-class node kind in pii-graph, or a thin wrapper over the
   existing model node?** *Lean: wrapper first (ships against :8083 today, no
   pii-graph change), first-class later.*
2. **Where do confidence bands live** — per-graph config, or one global policy the
   ledger can tune? *Lean: one global policy with per-graph overrides.*
3. **Make the catch-all mandatory at load time** for any route whose `on` starts
   `$.payload.label`? That converts a runtime error into a **static** guarantee.
   *Lean: yes, as a new static check.*
4. **`band` computed by Laya, or by a transform node?** *Lean: transform — keeps
   thresholds reviewable in one line of the graph file.*
5. **Project name/location** — `/projects/laya-rs`?
6. **Ledger location** — inside the project, or alongside amem's store so
   decisions are queryable with existing tooling?

---

## 12. Sources

### 12.0 The upstream layer — read this before citing the line below

The JEV lineage has an **upstream** source, reviewed 2026-10-01:
[`04-jev-typesafe-ai-video-review.md`](04-jev-typesafe-ai-video-review.md) — Jeremy Chone
building his own Rust client against the vendor "type safe AI" / "System One" API, live,
2h44m. Two findings there that bind anything written below:

- **The upstream never claims determinism.** `deterministic`, `constrain`, `grammar` and
  `logit` occur zero times; the API returns a **probability** per question. So this repo's
  determinism is a property of **its own Rust head** (spec 03, measured to 4.05e-6 against
  PyTorch) and must never be justified by citing the JEV line.
- **The three question types converge independently**: the vendor's `choice` / `score` /
  `null` are exactly our `QTYPES = {choice:0, score:1, noul:2}`. That is corroboration of the
  shape, not a source for it.

### 12.1 The video and its repository

**Video** — "Ten Levels of Jev" (IndyDevDan), `https://youtu.be/_U-O5lYhJ7Q`
(2026-09-28, 35 min, 66,731 views at capture). *Transcript NOT fetched — YouTube
returned HTTP 429 on the subtitle endpoint. All content below was taken from the
**full video description** plus the **repository README**, which together carry
the chapters, all ten levels, the failures section and the ten rules.*

**Repository** — `https://github.com/disler/ten-levels-of-jev` (MIT, 107 stars,
default branch `main`, last push 2026-09-27). Tagline: *"Ten levels of Jev, from
one smart if statement to a coding agent that reaches for Jev on its own."*

Verified tree (GitHub Contents API, 2026-09-24) — links to the pieces worth reading:

| what | link |
|---|---|
| README (the ten levels, the failures section) | https://github.com/disler/ten-levels-of-jev/blob/main/README.md |
| `justfile` — `just jev1..jev10`, `demo`, `test`, `test-live` | https://github.com/disler/ten-levels-of-jev/blob/main/justfile |
| **the portable skill** (client + 30 examples + cookbook) | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/SKILL.md |
| cookbook — structure & setup | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/cookbook/01-structure-and-setup.md |
| cookbook — input/output contract | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/cookbook/02-input-output.md |
| cookbook — **question design** | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/cookbook/03-question-design.md |
| cookbook — client operations | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/cookbook/04-client-operations.md |
| cookbook — production | https://github.com/disler/ten-levels-of-jev/blob/main/.claude/skills/hyper-jev/cookbook/05-production.md |
| cookbook — use cases | https://github.com/disler/ten-levels-of-jev/tree/main/.claude/skills/hyper-jev/cookbook/use-cases |
| skill templates (starter) | https://github.com/disler/ten-levels-of-jev/tree/main/.claude/skills/hyper-jev/templates/starter |
| lab app — README | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/README.md |
| **the client** | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/src/core/client.ts |
| **the deterministic mock** (offline tests) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/src/core/mock.ts |
| the types (question/answer contract) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/src/core/types.ts |
| helpers | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/src/core/helpers.ts |
| **the 30 scenarios** (one file, all ten levels) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/server/scenarios.mjs |
| the API server | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/server/server.mjs |
| **agent sessions** (one pi process per session, RPC) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/server/agent-session.mjs |
| extensions — **L6 guard** (the bash gate) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/jev-guard.ts |
| extensions — **L7 compaction** | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/jev-compact.ts |
| extensions — **L8 cheap reads** | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/ask-jev-file.ts |
| extensions — **L9 files at scale** | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/ask-jev-files.ts |
| extensions — **L10 agentic** | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/ask-jev.ts |
| extensions — event reporting (`JEV_EVENT` on stderr) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/extensions/report.ts |
| levels 01–10 (one dir each) | https://github.com/disler/ten-levels-of-jev/tree/main/apps/ten-levels/src/levels |
| tests — per level (01–10) | https://github.com/disler/ten-levels-of-jev/tree/main/apps/ten-levels/tests |
| tests — **the live suite** (vs the offline mock) | https://github.com/disler/ten-levels-of-jev/blob/main/apps/ten-levels/tests/live.test.ts |

**The five files we should read before designing our own equivalents:**
1. `apps/ten-levels/src/core/mock.ts` — the deterministic mock that makes offline
   contract tests possible (`just test` = 201 tests, zero model calls).
2. `apps/ten-levels/extensions/jev-guard.ts` — the L6 gate whose *single-path*
   failure the README documents (the agent routed around it via a bash heredoc).
3. `apps/ten-levels/extensions/ask-jev-file.ts` — the L8 pattern we want to port.
4. `.claude/skills/hyper-jev/SKILL.md` — the portable-skill packaging we should copy.
5. `apps/ten-levels/server/agent-session.mjs` — one pi process per session in RPC
   mode with an explicit tool allowlist (confirms our own `--rpc` finding).

### 12.2 Our own prior Jev/Laya research (the long trail this builds on)

This design is **not** a new direction — it is where a line of work we have been
running since 2026-09-21 lands. Read these before reviewing it; several are the
source of the design's invariants.

**Deliverables on disk:**
- `/projects/pi-improved/_docs/06-research/039-jev-system-one-decision-layer-analysis.md`
  (commit `c0647f1`/`06ce90a`) — Jev vs Needle 3 vs Laya; where it fits pii; the
  "select among existing ports, never create an edge" conclusion.
- `/projects/pi-improved/_docs/06-research/040-laya-on-llamacpp-and-graph-driving.md`
  (commit `bd8b0dc`) — Laya on llama.cpp, the hybrid split, graph-driving.
- Register rows: `/projects/docs/01-overview/OPEN-REGISTER.md` **A16**, **A17**.

**amem — topic `pii` (the substantive analyses):**
| key | what it holds |
|---|---|
| `pii/research/jev-laya-system1-decision-layer-part1` | what Jev **is**: 3 primitives (Choice ≤255 / Score 2-10 / Noul P(0..1)), the endpoint, the SDKs, RLCD (calibration by construction) |
| `pii/research/jev-laya-system1-decision-layer-part2` | **where it fits pii** — the ranked placements J1–J3. J1 is *exactly* this design: "a System-1 router node behind `route`; the answer becomes `$.payload` and the EXISTING route branches on it unchanged… the model can only SELECT AMONG PORTS THAT ALREADY EXIST, never create an edge" |
| `pii/research/laya-llamacpp-and-graph-driving-2026-09-21` | **can Laya run on llama.cpp — YES**, as a hybrid; the head "must run outside llama.cpp"; 36/36 tensors mapped; Δp 0.0006 |
| `pii/research/laya-measured-2026-09-21` | the **measured** numbers: no GPU, CPU path, 1 question 140 ms, 4 questions 328 ms, steady median 307.9 ms; install clean (laya 0.3.4) |

**amem — topic `workspace` (prior insight entries):**
| key | what it holds |
|---|---|
| `insights/laya-system1-alternative-to-jev` | Laya as the LOCAL-FIRST ZERO-COST replacement; API-compatible with Jev; the caveat that the benchmark table is **vendor-self-reported** |
| `insights/jev-system-one-models-primary` | Jev can slot at every branch point so code/infra knows the decision |
| `insights/jev-herdr-model-router` | the **model-router CLI** pattern — "dry run before you spend tokens"; and that the glue, not the vendor, is the portable part |
| `insights/typeface-ai-and-jev` | separating **decision-making from text generation** in agent loops; validates strict schemas + deterministic gates |
| `pii/research/jev-system-one-decision-layer-2026-09-21` | the grounding facts: latency 70–500 ms, input $0.042/M tok, **output free**, 0 % structured-output errors by construction |
| `research/jev-console-url` | TypeSafe console URL for replay/usage inspection (user holds a dev key) |

**amem — topic `insights` (later entries):**
- `x-jev-githubs-roundup` — the Jev tooling roundup.
- `youtube-jev-vs-deepseek-fraude-pix` — Rust+JEV vs DeepSeek+Python for
  traceable, low-latency transaction workflows.

**amem — adjacent (the *other* specialized-dispatch model):**
- `hermes-lnx-optimization/finding/needle-26m-specialized-dispatch` — Needle
  (Cactus Compute, 26M params, Simple Attention Network, no FFN) for single-shot
  function calling. Same 26M order of magnitude as the Laya head (F1), a different
  bet: Needle maximizes *dispatch* speed, Laya maximizes *calibrated judgement*.
  Worth citing whenever "small specialized model" is argued.

**amem — the design decisions from this session (topic `laya-rs`):**
- `laya-rs/architecture/design-and-ten-levels-mapping-2026-09-24`
- `laya-rs/architecture/driving-dags-deterministic-port-selection-2026-09-24`
- `laya-rs/architecture/two-lane-laya-and-llm-together-2026-09-24`

**amem — the video digest (topic `indydevdan-agentic-engineering`):**
- `.../video/v6-ten-levels-of-jev` — the full level-by-level digest
- `.../finding/jev-10-levels-vs-our-laya-on-llamacpp-position` — our position

### 12.3 External references

- Laya model card — `https://huggingface.co/convaiinnovations/laya` (Apache-2.0)
- The independent GGUF conversion — `fr0stbit3/laya-gguf` (Apache-2.0),
  `convert_hf_to_gguf.py`; README states the GGUF holds the ModernBERT
  **backbone only**.
- TypeSafe Jev console — `https://console.typesafe.ai/home`
- OpenAI `tokenizers` vs `transformers` version conflict (why the Python path is
  fragile and the Rust path appealing) — observed locally in the
  digital-marketing venv.

### 12.4 Local evidence (ground truth used in the facts table)

- `/projects/_tmp/laya-gguf/{laya_head.py, parity_test.py, hybrid_test.py,
  rl_agent_config.json, tokenizer.json}`; `laya-head.safetensors` (106 MB binary)
- `/projects/digital-marketing/.venv/.../laya/agent.py` (v0.3.4 — the decision path)
- `/projects/digital-marketing/.venv/.../laya/` (the `head.py`/`rl_agent_config` sources)
- `/projects/pi-improved/crates/pii-graph/src/{nodes.rs, validate.rs, spec.rs,
  journal.rs, expr.rs}`, `graphs/starter/failover.toml`
- `/projects/pi-improved/crates/pii-mcp/src/main.rs` (the rmcp stdio pattern)
- `/projects/pi-improved/crates/pii-abi/src/failover.rs` (`FailoverReason`)
- `/projects/pi-improved/crates/pii-model/src/llamacpp.rs` (`llama-cpp-2` usage)
- `/projects/amem/crates/core/` and `/projects/pidag/` (both use `redb`)
