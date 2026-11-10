<!-- Filed 2026-10-06T14:30Z by the Hermes orchestrator (Marcelo Correa).
     Produced by: pi + glm-5.3:cloud (ollama-cloud), the two-lane ANALYSIS lane (read-only tool grant),
     dispatched from _tmp/laya-seam-plan/run-laya-seam-review.sh per
     hermes/rules/two-lane-pi-analysis-then-workhorse-2026-10-06.
     Brief: _tmp/laya-seam-plan/PROMPT-20261006.md   Session: _tmp/laya-seam-plan/pi-sessions/
     Repo HEAD at analysis time: 9dfa5e9
     Orchestrator acceptance: session header provider/modelId verified; trees laya-rs+pi-improved read-only
     respected; worker claims spot-checked (hook :87-90 fingerprint, SKIP rows, ledger stats) — all held.
     Orchestrator verdict: ACCEPTED as the fix-plan of record. SPEC-A is the single next change.
     Nothing in this document is authorised work until specced, gated and committed. -->
> **DEPLOY-DAY STATUS (2026-10-06 ~15:00Z, supersedes the open-state above):**
> SPEC-A's laya-rs half = implemented as **spec 28**, gate green, committed `386f012`,
> deployed live (decision_id verified in production payloads; outcome joins verified —
> `outcomes: 0→4`); CLI ingest-before-read was the spec gap the worker found (reproduced
> 3×, fixed). SPEC-B = **live same day** (`laya-ask-nudge.sh` delegated from the gate
> hook; block-once-then-allow; selftest 22/22). SPEC-E stays open (the pi workhorse lane
> has no laya wiring); SPEC-A's autoresearch half = keep-seam wired in the driver (verdict
> recorded per row; enforcement on calibration proof); SPEC-C = spec 29, next; SPEC-D
> superseded (calibration corpus now produced by construction at the seam). Full state:
> `HANDOFF.md` § Deploy day.

# LAYA SEAM REVIEW + FIX PLAN — 2026-10-06

Reviewer: read-only worker. Deliverable: this report. No repo edits, no commits.
All live measurements taken 2026-10-06 ~13:47–14:05 UTC unless dated otherwise.
Every file:line below was read this run; live numbers were read from the running
deployment (`laya ledger stats`, `laya.log`, ledger segments), not from source constants.

---

## 1. Gate/mechanism table — what exists today, with evidence

| Component | What it does today | Evidence |
|---|---|---|
| `laya-gate.sh` (hook) | pre_tool_call, fail_closed. Gates **only** `terminal\|write_file\|patch\|execute_code`. Extracts `.command` only (falls back to the constant `"<tool> call"`). Blocks on: no decision, `reason=sidecar-unavailable`, `deny`. `ask` **passes**. `allow` passes. Every gated call writes a `laya.log` line + a ledger row. | `~/.hermes/hooks/laya-gate.sh:55` (LAYA_GATE_DISABLE bypass), `:79-82` (tool allowlist), `:87-90` (command extraction), `:130` (fingerprint = md5(SESSION␟TOOL␟COMMAND)), `:133-139` (5s SKIP window), `:155-160` (exec, `--run-id "hook-$$"`), `:176-186` (fail-closed block), `:197-207` (head-down block), `:210-220` (deny block), `:225` (PASS line) |
| Hook registration | pre_tool_call + `fail_closed: true`; **a `post_tool_call` event also exists** (used by a ledger hook) with payload keys `result`, `status` (ok/error/blocked), `error_type`, `error_message`, `duration_ms`, `session_id`, `tool_call_id`. `on_session_end` and `subagent_stop` (with `tool_call_history`!) also exist. `fail_closed` is honored **only** on pre_tool_call (`_BLOCKING_EVENTS`). | `~/.hermes/config.yaml:129-131` (laya-gate), `:132-133` (post_tool_call); hermes `agent/shell_hooks.py:179` (_BLOCKING_EVENTS), `:89-101` (post_tool_call payload), `:115-122` (on_session_end), `:124-134` (subagent_stop) |
| `laya-gate` binary | One command → one verdict → one ledger row. Exit codes carry the verdict: 0 allow, 1 ask, 2 deny, 3 unjudgeable. Calls the SAME `gate_payload` the MCP `gate` tool calls. | `crates/laya-mcp/src/bin/laya-gate.rs:43-46, :108, :151-165` |
| `gate_payload` (shared judgement) | Judges, records the row (`executed=Some(false)`, `run_id`/`node_id` from args), returns payload. **Does not surface the decision identity** — `record_decision_for` returns only an error note on failure, `None` on success. | `crates/laya-mcp/src/lib.rs:1051-1112`, `:1327` (record_decision_for), `:1416-1419` (append, returns None on success) |
| `gate_verdict` (the decision function) | `decision = Ask` if destructive-signals ∨ irreversible ∨ **not allowlisted**; otherwise the model decides. Deny is reachable **only** as (model-label=No ∧ noul band High). | `crates/laya-core/src/agentic.rs:556-580` (gate_verdict), `:528-539` (model_only_decision), `:334-350` (signal list) |
| `Policy` | `gate_allow` default = 13 read-only commands; `noul_high_threshold=0.8`; `near_tie_margin=0.01`. A measured comment records that destructive commands score 0.90–0.99 "safe" on this model. | `crates/laya-core/src/policy.rs:175-194` (default allowlist), `:201-211` (measured note + thresholds), `:743-752` (empty-allowlist-is-valid control) |
| Deny branch tests | Deny IS unit-tested: allowlisted command + confident-No → Deny. The branch works in tests; see G2 for why it cannot fire in production. | `crates/laya-core/src/agentic.rs:1160-1164` (`gate_verdict_a_confident_no_is_never_allowed`) |
| Outcome write path | **Exists and is CLI-exposed**: `laya ledger outcome <id> --correct|--wrong --source S [--note T]` writes an `OutcomeRow` via `Ledger::open_writer` + `record_outcome`. The id is the decision's `Record::identity()`. | `crates/laya-cli/src/main.rs:384-425`; `crates/laya-store/src/lib.rs:198-215` (OutcomeRow), `:781` (record_outcome), `:236-260` (identity = `d␟ts_nanos␟wall_nanos␟tool␟question_id␟label`) |
| Calibration | ECE over (confidence, correct) pairs; refuses to compute without ≥30 samples; refuses `0` for no data. | `crates/laya-store/src/calibrate.rs:44-58` (MIN_SAMPLES), module doc |
| MCP surface (7 tools) | `decide, ask_file, ask_files, gate, should_compact, ask, ledger` — registered live in hermes config with `LAYA_READ_ROOTS=<allowed roots>`. `ask_files`/`ask_file` are the token-saving retrieval path (measured 43×: 417,039 B → 9,678 B, 0 provider tokens). | `crates/laya-mcp/src/lib.rs:144-152` (TOOL_NAMES), `:476+` (tool_meta/schemas); `~/.hermes/config.yaml:196-201`; `_docs/02-guides/USING-LAYA-DAILY.md` §1–2 |
| Keep seam (autoresearch-loop) | Bash driver commits the experiment (`git -C "$WT" commit`, line 173), runs the metric, `keep`/`discard` (177-191), discard resets HEAD~1 (191). **No laya call anywhere in the driver.** | `~/.hermes/skills/software-development/autoresearch-loop/scripts/autoresearch-driver.sh:161-195` |
| Live ledger | 5083 decisions, **outcomes 0**. Per tool: gate 4329 (allow 39, **ask 4289**, yes 1, executed 0); ask 26 (12/12/2, executed 12); ask_file 13; ask_files 17; decide 650; should_compact 48. Bands high 2378 / mid 2559 / low 120 / unknown 26; near ties 49; latency p50 481 / p95 1740 / max 9115 ms. | `LAYA_LEDGER_DIR=/var/lib/laya-ledger laya ledger stats`, run 2026-10-06 |
| laya.log | 5231 rows: PASS 4214, SKIP 94, BLOCK 10 (all "laya unreachable" during the Oct 3–4 sidecar outages — fail-closed worked). Tool mix: terminal 2987, write_file 698, execute_code 447, patch 184. | `~/.hermes/logs/laya.log`, counted this run |

**Honest semantics of the deployed gate:** with ask passing at the hook, 4289/4329 (99.1%) of gate verdicts change nothing — the gate today is (a) a **reachability check** ("is laya up"), (b) a **ledger recorder**, and (c) a deny path that cannot fire (G2). That is a real deployment, but it is not yet a decision layer making decisions.

---

## 2. The three gaps

### G1 — OUTCOMES = 0 (no calibration feedback ever recorded)

**ROOT CAUSE (three missing links, not a missing mechanism).** The write path exists
(`laya ledger outcome`, CLI main.rs:384; `record_outcome`, store lib.rs:781; the join key is
`Record::identity()`, store lib.rs:236-260). What does not exist:

1. **The identity is invisible.** `record_decision_for` (laya-mcp lib.rs:1327) returns an
   error note or `None` — the decision's identity never reaches the caller, so the hook
   cannot log it and a post-run pass must re-read the whole store to find the row.
2. **The hook destroys its own join key.** It passes `--run-id "hook-$$"` (laya-gate.sh:158) —
   an ephemeral PID — while holding the real `$SESSION` in hand (laya-gate.sh:65). Ledger rows
   cannot be joined to sessions. Live tail confirms: `"run_id":"hook-772459"` etc.
3. **No ground-truth producer is wired.** Nothing correlates a verdict with what happened
   next. Nothing calls the CLI. AGENTS.md rule 11 already names the intended path
   (`laya ledger outcome <id> --correct|--wrong`, laya-rs/AGENTS.md:97-100) — it was written,
   never wired.

**MINIMAL FIX (three small changes, spec-A):**

- *laya-rs:* `record_decision_for` returns the row's `Record::identity()` on success
  (change the `Option<String>` return, lib.rs:1327/:1416-1419); `gate_payload` inserts
  `"decision_id": <identity>` into the printed payload (lib.rs:1051, after :1092). Files:
  `crates/laya-mcp/src/lib.rs`. `laya-gate` needs no change — it prints the whole payload.
- *hook:* one line — `--run-id "$SESSION"` (laya-gate.sh:158) — and extend the PASS line
  (laya-gate.sh:225) to include the decision_id, so laya.log rows carry the ledger reference.
- *producer:* the keep seam (G2 fix) records outcomes **as a by-product of every loop run** —
  the loop's own next-iteration metric is the ground truth, bash-driven, zero agent
  cooperation. An optional weak-proxy producer for the daily lane: a `post_tool_call` hook
  recording status ok/error for the *executed* ask lane (12 rows so far). A cron sweep
  correlating rows with incidents remains the weakest option — incidents are not
  machine-observable; do not spend on it first.

**PROOF (fails pre-implementation):**
```
laya-gate "ls" --run-id t1 | jq -r .decision_id     # today: null/absent → must be non-empty
laya ledger outcome <that-id> --correct --source test   # "recorded: … is correct"
laya ledger stats                                      # outcomes: 1 (today: 0)
```

**CONTROL:** `laya-gate "ls" --no-ledger` must produce NO decision_id (no row was written —
nothing may claim one); a `--wrong` outcome on an id that never existed must not silently
inflate calibration (see NOT-VERIFIED #2 — whether `record_outcome` validates the reference).

### G2 — DENY NEVER FIRED in 5072+ decisions

**ROOT CAUSE (structural, in the code — not a tuning problem).** `gate_verdict` forces
`Ask` for everything not allowlisted (agentic.rs:567). Deny is therefore reachable only when
a command **matches the read-only allowlist** AND the model returns a confident No
(label=No, noul band High ≥0.8, agentic.rs:535-538). A confident "unsafe" on `ls`/`git status`
is semantically contradictory; and the model is *measured* to say Yes 0.90–0.99 even on
destructive commands (policy.rs:201-211). Live proof from today's tail: a row with
`destructive_signals: 2, model_decision: allow, p=0.9743, band: high`. So:

- The deny branch is **unit-tested and works** (agentic.rs:1160-1164) — but is
  **production-unreachable** under the default policy.
- The hook never passes `--irreversible` (laya-gate.sh:157 sets it only from an env var
  nothing sets), so that Ask-forcing path is dead too.

**What SHOULD have been denied:** (a) the workspace's own categorical bans — `rm -rf` on
pre-existing paths is banned by AGENTS.md hard rule 1; today such commands pass with the
signal merely *recorded* (deliberate, documented at laya-gate.sh:34-37, because the signal
list conflates routine `mv`/`find`/`rm -f` with never-acceptable `rm -rf`/`of=/dev/`/
`push --force`); (b) on a keep seam, anything the judgement refuses.

**MINIMAL FIX (two parts):**

1. *A hard-deny class in Policy* (spec-C): new field `gate_deny: Vec<String>` — patterns that
   force `Deny` (checked **before** the allowlist question, i.e. deny beats ask). Default
   **empty** — same precedent as "an empty allowlist is a valid policy" (policy.rs:743-752).
   Deployed config sets `gate_deny = ["rm -rf", "of=/dev/", "git reset --hard", "push --force"]`.
   Files: `crates/laya-core/src/policy.rs` (struct :123-172, Default :196-224, validate,
   TOML round-trip test :760-765 pattern); `crates/laya-core/src/agentic.rs`
   (`gate_verdict` :556-580 — insert the deny check first; `GateVerdict` :510-524 — carry the
   matched deny entry). RED tests first per the project's TDD rule.
2. *The keep-seam integration* (spec-A): in the driver, before `status="keep"` (driver
   :177), ask laya over the candidate (see P2 for the exact feasible route); seam policy:
   **refusal (ask/deny/unjudgeable) → discard + log**, allow → keep. Record the decision_id;
   the next iteration's TSV row is the outcome. The loop is bash-driven — deny becomes
   exercised **and** outcome-recorded with zero agent cooperation.

**PROOF (fails pre-implementation):** `laya-gate "rm -rf /tmp/x"` with the deployed policy →
exit 2, `decision: deny` (today: exit 1, ask). Driver selftest: a forced-refusal case yields
`discard` + a ledger row + a later outcome row.

**CONTROL (mandatory near-misses):** `laya-gate "ls"` still allow; `mv x y` still
ask-not-deny (the routine class must survive); with `gate_deny = []` nothing denies (the
empty-is-valid control); at the seam, a stub forced-allow still keeps.

### G3 — AGENT-SIDE ask_files ADOPTION = 0

**ROOT CAUSE.** The tools exist, are registered, and are measured at 43× token reduction —
but adoption is voluntary and the default habit is `read_file`. Lifetime evidence: 30
ask_file/ask_files rows vs 5083 decisions (13 + 17 rows — consistent with the measurement
sessions, not daily use). Nothing in an agent's path ever *requires or even suggests* the
alternative; AGENTS.md's own audit rule ("a session that read large artifacts without a
single ask is a defect", laya-rs/AGENTS.md:99-100) has no enforcement mechanism and no
measuring instrument.

**MINIMAL FIX (enforcement, bash-only, zero model calls on the hot path).** New pre_tool_call
hook `~/.hermes/hooks/laya-ask-nudge.sh`, registered in config.yaml, firing on
`read_file`:

- `stat` the path (~1 ms — no laya call, no latency risk).
- Block **once per (session, path)** when ALL of: size > threshold (start 64 KiB), path under
  `LAYA_READ_ROOTS`, no `mcp__laya__ask_file(s)` call seen for that path this session (the
  hook sees every tool call, so it can mark ask usage itself), not already nudged for this
  path.
- The block message contains the **ready-to-paste ask_files JSON** for that path/question.
- The immediate retry passes (mark written at first block) — block-once-then-allow.

**Why mandatory beats advice here (user preference recorded in rule 11):** the token saving
is real and measured, `ask` never blocks and never writes, and the ledger makes adoption
auditable — the nudge hook turns the audit rule from prose into a check.

**PROOF (fails pre-implementation):** selftest — read a >64 KiB file with no prior ask:
blocked once, message contains `ask_files`; retry passes; a second distinct large file gets
its own single nudge; small files never block. Adoption becomes measurable:
`laya ledger stats` per-tool `ask_files` count rises in real sessions.

**DANGER + CONTROL.** The danger is blocking legitimate reads (reading a large spec *to edit
it* is legitimate and common). The control is structural: block-once-per-path-per-session
(never a hard wall), a high threshold, read-roots scoping, and the near-miss tests above —
two different large files each nudged once, small files never, retry always passes. If the
block-rate in laya.log exceeds a few percent of large reads, the threshold is wrong — that
ratio is the health metric, printed by the selftest.

---

## 3. Premises P1–P3

**P1 — "the hook fires on every state-changing call in every session": CORRECTED, twice.**

1. *Within Hermes, the tool allowlist is incomplete.* The gated four are
   `terminal|write_file|patch|execute_code` (laya-gate.sh:79-82). Hermes has further
   state-changing tools (enumerated from hermes-agent `tools/*.py` `"name"` fields):
   **`browser_exec` / `browser_cdp`** (arbitrary JS in a real browser), **`cronjob`** — the
   sharpest hole: a scheduled job's *execution* never passes any pre_tool_call at run time,
   so it is a deferred ungated-command channel; **`image_generate`/`video_generate`** (write
   artifacts), **`process`**, **`memory`**, `kanban_*`, `yb_send_dm`/`react_to_message`
   (external side effects). `delegate_task` subagents: their tool calls go through the same
   "single invocation point" for pre_tool_call hooks (hermes `agent/agent_runtime_helpers.py:3475`),
   so they are *likely* gated — but this was **not verified live** (NOT-VERIFIED #1); the
   `subagent_stop` hook's `tool_call_history` would answer it cheaply.
2. *The bigger correction: the hook is a HERMES hook.* **pi sessions (the worker lane) have
   no laya wiring at all** — `~/.pi/agent/settings.json` holds only the tool list
   (read/bash/edit/write/grep/find/ls), skills, and model config; no hook, no laya extension
   (the only installed extension is herdr's). A hermes session invoking `pi -p …` via its
   terminal tool gets the *outer* call gated; **every write/edit/bash the pi worker makes
   inside is invisible to laya**. This review session itself is the proof: none of my tool
   calls passed laya-gate. "Always and automatically" is currently true only for Hermes
   sessions' four tool classes — the lane where most code gets typed is ungated.

**P2 — "a deny-able job on the keep-seam is feasible with current Policy": CORRECTED.**
Via `gate`, **no**: any non-allowlisted action (a commit description) is forced to `Ask`
(agentic.rs:567), and Deny additionally requires allowlisted + confident-No — so with the
current allowlist the seam can *refuse* (treat non-allow as discard) but **can never see
`deny`**. Feasible routes, in order of preference: (1) the **`ask`** tool — caller-written
questions over a composed state (the diff + the metric evidence), route on
band: high → keep, else → discard; this is a deny-equivalent refusal needing no Policy
change; (2) the `gate_deny` Policy addition (G2 fix) so the seam's actions can hard-deny;
(3) *not* adding the seam's action to `gate_allow` — that would let the model **allow**
commits, the wrong direction. Exact functions/fields to change: named in G2 fix 1.

**P3 — "outcomes can be recorded without agent cooperation": CONFIRMED.** The write path is
a plain CLI over the store: `laya ledger outcome <id> --correct|--wrong --source S`
(main.rs:384-425) → `Ledger::open_writer` + `record_outcome` (store lib.rs:781). Any
external process — hook, cron, the bash loop driver — can write it. The join key is
`Record::identity()` (store lib.rs:236-260); a sweep computes identities from the rows
themselves (`records()`, store lib.rs:718/:823). What's missing is not the write path but
the identity's visibility (G1 fix 1) and a ground-truth producer (G1 fix 3). One caveat
unverified: whether `record_outcome` validates the referenced decision exists
(NOT-VERIFIED #2).

---

## 4. Additional review asked for

**Gate decision-path defects found (beyond G1-G3):**

1. **SKIP window hides different writes** (real, measured). For `write_file`/`patch`/
   `execute_code` the "command" is the constant `"<tool> call"` (laya-gate.sh:87-90), so the
   fingerprint (laya-gate.sh:130) is tool-only: **any two write_file calls within 5 s in one
   session dedupe to ONE gate call — the second is neither judged nor recorded.** 94 SKIP
   rows in laya.log; the tail sample is all `patch`/`write_file` constant fingerprints.
   Mandatory usage is undercounted by exactly these rows. Fix (spec-D): include the target
   path (or a content hash) in the fingerprint for those tools.
2. **The model judges a constant for three of four tools.** For write_file/patch/execute_code
   the state the model sees is the literal string `"write_file call"` — the ledger's
   band/confidence for 1329 rows (698+447+184) is a judgement of a constant, near-worthless
   for calibration (identical state → identical p). The gate for those tools is a
   reachability check, not a decision. Fix (spec-D extension): include path/code hash in the
   judged state.
3. **`--run-id "hook-$$"`** (laya-gate.sh:158) — ephemeral PID, unjoinable (G1 fix 2).
4. Latency: p50 481 ms / p95 1740 ms / **max 9115 ms** added to every state-changing call,
   with a 10 s timeout (laya-gate.sh:53) — the tail is real but the fail-closed path was
   exercised correctly during the Oct 3–4 outages (10 blocks, all "laya unreachable").
   `ask_files` p95 is 9.1 s (serial head, documented in ask_files.rs) — fine for triage,
   never for the hot path; the nudge hook (G3) correctly avoids model calls.
5. Fail-closed is sound: no-decision → block (laya-gate.sh:176-186); `ask`-with-
   `sidecar-unavailable` reason → block (:197-207); jq failures degrade to empty decision →
   block. The allowlist/metacharacter logic (agentic.rs:417-449) is well-tested with
   random controls. The stray `yes: 1` verdict label in stats is a historical row from the
   label-ordering bug documented (and fixed) at lib.rs:1069-1080 — only one such row exists.

**Ledger shape — can outcomes+decisions be joined for calibration?** Yes, mechanically:
`OutcomeRow.decision == Record::identity(decision)` (store lib.rs:212, :236-260). Once
outcomes land, the query "how often was laya right" is: join on identity → group by
`band`/`tool` → correct-rate per band; `calibrate.rs` consumes (confidence, correct) pairs
directly, with the 30-sample honesty gate. Today the join is ready and the data is absent
(outcomes 0). Unlogged-that-should-be: SKIPped calls leave no row (undercount); laya.log
lines carry no decision reference; 26 `band unknown` rows unexplained. Also note
`executed` is `Some(false)` for gate rows by design (permission ≠ event) — a post_tool_call
hook is the missing event lane.

**Memory-graph lane (+0.44 vs the +0.50 bar) — could laya score/verdict data improve the
ranking via calibration? Plainly: no.** Calibration (ECE) fixes the *meaning* of confidence
values, not the *discrimination* between candidates; the lane's failure is discrimination —
its +0.375/+0.44 matches the published literature for this corpus shape (RESUME-REGISTER,
graph-lane entries). With 0 outcomes there is nothing to calibrate against anyway. What
laya data *could* offer the lane (a hypothesis, only measurable after outcomes exist):
band/near-tie **filtering** of lane items (drop band-low/near-tie) as a precision lever —
a cheap A/B, not a fix, and not worth planner time before G1 lands.

---

## 5. Ranked spec-ready candidates

| # | Candidate | Files / exact names | Exit criteria that FAIL today | Needs planner? |
|---|---|---|---|---|
| 1 | **SPEC-A — keep-seam + decision_id** (closes G1+G2 together) | laya-rs: `crates/laya-mcp/src/lib.rs` (`record_decision_for` :1327, `gate_payload` :1051); hook: `laya-gate.sh:158, :225`; skill: `autoresearch-driver.sh:161-195` (ask-laya before `status="keep"` :177; outcome from next-iteration TSV) | `laya-gate "ls" \| jq -r .decision_id` → non-empty (today: absent); after one selftest run `laya ledger stats` → `outcomes: ≥1` (today 0); driver selftest forced-refusal → `discard` + ledger row | **Yes** — cross-repo (laya-rs + hermes skill); worker implements after spec |
| 2 | **SPEC-B — read-nudge enforcement** (G3; the direct token-mandate lever) | new `~/.hermes/hooks/laya-ask-nudge.sh`; `config.yaml` pre_tool_call registration (non-fail-closed is fine; blocking is its function); selftest in the skill's pattern | block-once on >64 KiB read with the ask_files JSON in the message; retry passes; distinct files each nudged once; small files never; laya.log nudge rows appear | Yes (author specs/NN first) |
| 3 | **SPEC-C — hard-deny class** (G2 daily path) | `crates/laya-core/src/policy.rs` (new `gate_deny: Vec<String>`, Default empty, validate, TOML round-trip); `crates/laya-core/src/agentic.rs` (`gate_verdict` order, `GateVerdict` deny-entry field); deployed policy config | `laya-gate "rm -rf /tmp/x"` → exit 2 deny (today: exit 1 ask); controls: `ls` still allow, `mv x y` still ask, empty `gate_deny` denies nothing | Yes (TDD spec; RED tests first) |
| 4 | **SPEC-D — SKIP-fingerprint + judged-state fix** (correctness of the mandate) | `laya-gate.sh:87-90, :130` (include `.path`/content hash for write_file/patch/execute_code); optionally laya-rs judged-state for those tools | two different write_file calls 1 s apart BOTH leave ledger rows (today: second is SKIP); same-command terminal still SKIPs (control) | No — small, but needs the spec convention |
| 5 | **SPEC-E — pi-lane wiring** (from P1 correction; research first) | investigate pi extension API for a pre-tool-call interceptor (extension mechanism proven by `~/.pi/agent/extensions/herdr-agent-state.ts`; API surface NOT-VERIFIED); gate pi's bash/write/edit through laya-gate | a pi session's write/edit produces a laya.log line + ledger row (today: zero) | Yes — research + spec |

Also noted, deliberately NOT specced: extending the hook's tool list (`browser_exec`,
`cronjob`, `image_generate`) — real holes (P1) but lower frequency than the pi lane and
each needs a command-text extractor; fold into SPEC-E's spec once the pi lane is decided.

---

## 6. NOT-VERIFIED list

1. **delegate_task subagent calls firing the shell hooks live** — code says single
   invocation point (agent_runtime_helpers.py:3475); no experiment run. Cheap test: delegate
   a terminal call, watch laya.log; or read `subagent_stop`'s `tool_call_history`.
2. **Whether `record_outcome` validates the referenced decision exists** (orphan outcomes) —
   read the call, not the implementation's every branch. Matters for the G1 control.
3. **Deployed binaries == tree HEAD** — binaries are Oct 3 12:34; the register's
   "deployed == tree" claim is attributed, not re-verified; working tree has ` M AGENTS.md`
   (docs-only rule-11 edit, uncommitted — consistent with worker rules).
4. **pi extension API's tool-interception surface** — the mechanism exists (installed
   extension), the API was not read (docs path from the harness prompt is absent in this
   container).
5. **All 94 SKIPs were different-content writes** — I sampled the tail (all
   patch/write_file constant fingerprints), did not enumerate every row.
6. **cronjob execution path** — read only the tool registration; whether the scheduler
   routes through any hook at run time is unverified (assumed NOT, hence the hole).
7. **p99/tail under a wedged sidecar beyond the 10 observed blocks** — fail-closed proved
   during Oct 3–4 outages; the block-then-retry loop was not exercised live.

---

## 7. The single next most valuable change

**SPEC-A: surface the decision identity, pass the session id, and stand laya in front of the
autoresearch keep seam.** One spec, three small commits (laya-rs payload change; hook
one-liners; driver integration) — and it closes **G1 and G2 by construction**: outcomes land
as a by-product of every loop run (the loop's own metric is the ground truth), the refusal
branch becomes exercised and deny-able (via `ask`-band routing now, `gate_deny` next), and
the calibration data every later threshold decision needs starts accumulating. Every other
candidate (read-nudge, hard-deny daily path, pi lane) is worth more once outcomes exist to
measure them against.