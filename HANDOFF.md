# HANDOFF — laya-rs

**Date**: 2026-09-30 (2) · **Status**: GREEN — 6 of 7 tools real, committed (`4b471e4`).
`scripts/verify-all.sh` PASS · **227 tests** · clippy 0 · fmt clean · 54 doc links 0 broken ·
audit vs baseline `new=0`.

## SCOPE — THIS PROJECT ONLY (no association with any other project)

**No association with any other project** (user, 2026-09-30). This repository is ours alone:
never modify, depend on, or cite another project's files or line numbers in our docs. A property
we rely on from outside is stated as a **fact about an interface**, and its consequence is
enforced by a test in our own code.

**`laya-rs` has NO graph implementation.** It has `route_payload.rs`: the contract with a graph
`route` node — the two field names a graph matches on — restated in our own crate and pinned by
test, so nothing outside this repository can move our behaviour silently. A DAG runner belongs
HERE if we build one.

## Six tools are real now (was 3 of 7)

| tool | state |
|---|---|
| `decide` | real — typed questions -> route-shaped payloads (`label`, `band`, `margin`, `near_tie`) |
| `gate` | real — **TWO signals**; see the safety finding below |
| `ask_file` | real — a judgement about a file, never the file |
| `ask_files` | real — globs expanded, capped at 64, `hits` ranked by belief |
| `should_compact` | real — L7, four judgements in one call |
| `ask` | real — L10, the CALLER writes the questions |
| `ledger` | **stub** — needs `laya-store`, which is unwritten |

## THE SAFETY DEFECT THAT WAS FIXED (do not regress it)

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

`laya-store` declares **no dependencies** ("NO DEPS YET, ON PURPOSE"), is in neither binary's tree,
`redb` is **absent from `Cargo.lock`**, and **no `.redb` / `ledger-*.jsonl` exists anywhere** for
this project. All seven tools are stateless.

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

## Next (ranked, unchanged)

1. `ask_file` / `ask_files` — highest value; everything needed now exists.
2. The **near-tie margin** in the payload (top1−top2), before P4: the measured
   Δp = 0.0006 between backends can flip a near-tie, so it must escalate, not decide.
3. `should_compact`, then `ledger` (needs an outcome path first).
4. **P4** — the Rust head, which also wires `temperature_was_clamped` / `temp_tag` /
   `with_criteria` or deletes them.

## Next (ranked, superseded by the section above)


1. **`ask_file` / `ask_files`** — the highest-value tools (a file judged without ever
   entering the agent's context). Everything needed to build them now exists.
2. **`should_compact`**, then `ledger` (needs an outcome-recording path before
   `calibrate` can mean anything — ECE needs `(confidence, correct)` pairs).
3. **The near-tie guard** must land before P4: the payload should carry the top1−top2
   margin so a Δp=0.0006-scale flip escalates instead of deciding silently.
4. **P4 Rust head** — removes torch, and with it the sidecar. Not before (1)–(3).

## Known gaps (honest)

* Five of seven tools are unimplemented — they error rather than pretend.
* The sidecar is supervised; **the MCP client session is not** — a session holding a
  pre-rebuild binary keeps its old behaviour (MCP clients do not hot-reload).
* `laya doctor` reads the socket env var at call time while `laya-mcp` reads it at
  startup. Same default, so no drift today, but a relocated socket can diverge.
* `calibrate` has no ground truth: the ledger records decisions, not outcomes.

## Verified state (all measured, not asserted)

```
cargo xtask gate        -> PASS (fmt + clippy -D warnings + test)
tests, whole workspace  -> 148 passing, 0 failing
clippy --workspace      -> 0 warnings
cargo fmt --all --check -> clean
doc links               -> 45 relative links checked, 0 broken
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

