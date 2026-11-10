# LAYA-RS — ANALYSIS BRIEF (adversarial design review)

**Job type: ANALYSIS ONLY. Do NOT implement. Do NOT edit any `.rs` file.
Do NOT create a `Cargo.toml` or scaffold the project.**

## Your task

Adversarially review the design document at `/projects/laya-rs/docs/DESIGN.md`.
It proposes `laya-rs`: a Rust MCP-stdio server exposing Laya (a small
System-1 decision model, self-hosted on llama.cpp) as a decision source that
supplies the *value* an existing deterministic DAG `route` node matches on.

Your job is **not** to agree with it. It is to find where it is **wrong,
incomplete, or will fail in practice**. The document was written by an
enthusiastic author; treat every "verified" claim as a claim to be checked.

## Read these files EXACTLY (do not go exploring the repo)

Read the design first, in full — including **§12, which lists the sources**:
1. `/projects/laya-rs/docs/DESIGN.md`  ← the subject of your review

Then the ground truth it cites. Read ONLY these:
2. `/projects/pi-improved/crates/pii-graph/src/nodes.rs`        (the `route` node)
3. `/projects/pi-improved/crates/pii-graph/src/spec.rs`          (NodeSpec, JoinPolicy, EdgeKind)
4. `/projects/pi-improved/crates/pii-graph/src/validate.rs`      (the static checks V0..V9)
5. `/projects/pi-improved/graphs/starter/failover.toml`          (a real DAG in production)
6. `/projects/pi-improved/crates/pii-mcp/src/main.rs`            (the rmcp stdio pattern to copy)
7. `/projects/_tmp/laya-gguf/laya_head.py`                       (how the head is loaded)
8. `/projects/_tmp/laya-gguf/rl_agent_config.json`               (head config)
9. `/projects/digital-marketing/.venv/lib/python3.12/site-packages/laya/agent.py`
   (v0.3.4 — the decision path; ~320 lines)

**Two prior documents this design claims to be consistent with.** Read them and
check that claim — the design says it "lands" on conclusions we already reached, and
it cites a ranked placement "J1" it says is *exactly* this design:
10. `/projects/pi-improved/_docs/06-research/039-jev-system-one-decision-layer-analysis.md`
11. `/projects/pi-improved/_docs/06-research/040-laya-on-llamacpp-and-graph-driving.md`

**Question: does the new design CONTRADICT or SILENTLY DROP anything in 039/040?**
Those earlier docs made ranked placements and recorded caveats. If the new design
asserts something 039/040 measured differently, or quietly omits a caveat they
raised (e.g. anything about vendor-reported benchmarks, latency, or the graph
"select among existing ports" boundary), that is a finding — name it.

The Laya head weights are at `/projects/_tmp/laya-gguf/laya-head.safetensors`
(106 MB binary — do NOT try to read it; its config is in file 8 and its tensor
shapes are recorded in the design's F2).

## Questions to answer — be specific and cite file:line

**A. The determinism claim (the load-bearing one).**
The design claims (F3/F4) that the answer is `argmax`, so same state → same port,
and that temperature scales probabilities *before* argmax, so tuning temperature
"can change the confidence band but can NEVER change the edge".
- Is that reasoning correct? Check `agent.py` yourself. Which exact lines?
- Is there ANY other source of nondeterminism in the path — floating-point
  non-associativity between the PyTorch head and a proposed *Rust* head, thread
  scheduling, the llama.cpp `/embedding` call, prompt construction, or the
  tokenizer? A Rust implementation will not produce bit-identical floats to
  PyTorch. **Does the argmax conclusion survive that, or can it flip an edge on a
  near-tie between two options?** This is the most important question in the
  review — answer it concretely.

**B. The DAG claims.**
- F11 says `route` is "already a deterministic port selector" fed by an expr.
  Verify against `nodes.rs`. Does its `eq` comparison really support the payload
  shapes the design proposes (string labels, bands)?
- F12 says `route` returns exactly one port (`find_map`, first match wins). Verify.
  Then: the design also wants a `band` transform feeding a *second* route. Does
  chaining two routes create any problem the design has not considered?
- F13 claims the "invented label" failure is impossible *twice over* (V1 at author
  time, hard error at run time). Verify V1 exists and does what is claimed. Is
  "impossible" too strong? What if the payload is missing the field entirely, or
  the value is `null` — does a no-`eq` catch-all rule really catch those?
- F14 claims V6 statically catches a routed-diamond deadlock. Read V6 and state
  precisely what it does and does NOT catch. Can the design's own shape
  (laya → route → branches → join) trip it, or is it genuinely covered?
- The design says a mandatory catch-all should become a static check. Is that
  implementable in `validate.rs` given how `on` is expressed (a free-form expr
  string)? If `on` is a string expression, how would a validator know which
  routes are "model-fed"? Is the proposed check actually well-defined?

**C. The two-lane composition.**
- Is "both lanes return `Outcome::Next{port, value}`" true? Check the `Outcome`
  enum and the `GraphNode` trait in `nodes.rs` / the graph crate. If the LLM
  `agent` node and a hypothetical `laya` node return different shapes, what is
  the actual integration work?
- The design routes Laya's *low-confidence* branch into the LLM lane. Is there a
  loop hazard (Laya → LLM → Laya → …), and does `loop_budget` cover it? Check how
  `loop_budget` is enforced.
- `ask_file`/`ask_files` read files in Rust and send content to Laya. What are the
  real failure modes (file size, binary files, encoding, secrets in files, prompt
  injection from file content)? The design mentions injection — is its mitigation
  sufficient?

**D. The Rust head port (F1/F2/F7).**
- Is "the head is pure linear algebra, therefore it can run in Rust" actually
  sound? What is missing from that tensor inventory (F2)? Look at
  `laya_head.py` — what does `load_head` do, and is there any logic OUTSIDE the
  tensor weights that the inventory does not capture (e.g. masking, positional
  handling, pooling, a prefix/type-embedding scheme, a max-length truncation)?
- The design's stated verification is a parity test (Δp ≤ 0.001 vs PyTorch).
  Is that a sufficient oracle? What could pass that test and still be wrong?

**E. MCP stdio surface.**
- Read `pii-mcp/src/main.rs`. What does the design get right about copying it,
  and what does it omit? Specifically: how are tools registered, how are errors
  returned, and how does the server terminate cleanly? Does the design's tool list
  (decide/ask_file/ask_files/route/gate/should_compact/ask/ledger/calibrate) fit
  the MCP tool model, or do some of them not belong as tools?
- `route` and `gate` as MCP *tools* — is that coherent? They are graph concepts.
  Argue for or against.

**F. What is missing entirely.** Name things the design does not mention that will
bite: concurrency (the redb single-writer constraint), the 0.3 s/decision latency
in a hook that runs on EVERY tool call, versioning of the head config, error
propagation from Laya to the caller, testing strategy for the live suite, how a
skill or hook actually discovers and calls this binary, and anything else you find.

## Rules

- **Analysis only.** Do not modify any file under `/projects/pi-improved`,
  `/projects/_tmp`, or `/projects/laya-rs` **except** the one deliverable below.
- Do not run `cargo build`/`cargo test` on pi-improved (it is large and not
  needed for this review).
- You MAY run read-only commands (`grep`, `sed`, `rg`, `jq`, `python3 -c`) to
  verify claims. Cite what you actually ran.
- **Where you cannot verify something, say so explicitly** rather than guessing.
  "I could not determine X from the files given" is a valid and valuable answer.
- Do not restate the design back. Assume the reader wrote it.
- Rank your findings: **BLOCKER / MAJOR / MINOR / NIT**, most severe first.

## Deliverable

Write exactly one file: `/projects/laya-rs/docs/ANALYSIS-glm-5.3.md`

Structure:
1. **Verdict** — 3-5 sentences. Is the design sound? What must change before code?
2. **Findings** — ranked, each as: severity, one-line claim, the file:line evidence,
   the concrete consequence, and the suggested fix.
3. **Corrections to the design's own "verified" facts** — if any F1..F15 is wrong or
   overstated, say which and why. This section matters most.
4. **What the design must add before implementation** — a checklist.
5. **What is genuinely good** — briefly; do not pad.
6. **Open questions you could not resolve from the files given.**

Keep it under ~500 lines. Precision beats length.
