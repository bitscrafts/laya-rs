# TOKEN ECONOMY — what laya saves, what it costs, and how each was measured

**Measured 2026-10-02.** Unit throughout: **bytes ÷ 4** as a token estimate. The session store's
`token_count` column is **unpopulated** on this host, so every number here is derived from message
content and says so. Nothing in this document is an API-reported token count.

---

## The answer in three lines

| | measured |
|---|---|
| **Saved** by asking instead of reading | **101,840 tokens per triage round** (43×) |
| **Cost** to the model's context | **0 tokens** on allow/ask; **178 tokens total, ever**, on two blocks |
| **Cost** in provider spend | **0**, always — the head is a local Rust binary behind a Unix socket |

---

## 1. What it SAVES — `ask_files` instead of reading

The corpus is six real files in this repo, the kind an agent would otherwise read to answer a
question about the ledger or the gate:

```
crates/laya-core/src/agentic.rs         53,246 B
crates/laya-mcp/src/lib.rs             195,076 B
_docs/05-operations/OBSERVABILITY.md     28,215 B
HANDOFF.md                              46,851 B
crates/laya-store/src/lib.rs              ~61,000 B
_docs/05-operations/DECISION-LEDGER.md    ~32,000 B
                                       ───────────
                                        417,039 B   ≈ 104,259 tokens if read
```

Three questions through `ask_files`, answered per file with a `noul` score and a rank:

```
returned to context                     9,678 B   ≈ 2,419 tokens
saved per round                       ~101,840 tokens
reduction                                    43x
```

**Where the saving actually comes from:** the agent gets *which file* and *how sure*, and reads only
the winner. The saving is real when the alternative is reading a directory to find one answer.

**Honest limits, all measured:**

- **~9 s per question** over six files. Each file pays a ~460 ms forward, so six files ≈ six forwards.
- **Ranking is weak on large files.** On `lib.rs` (195 KB) a question it only partly answers scored
  `0.7389`.
- **A score near 0.5 means the head does not know.** A `bucket_clamped` question scored `0.5121`.
  Acting on that is acting on a coin flip.
- **It is triage, not retrieval.** The answer is a label and a confidence — never the file's text. If
  you need the code, you still read the code.

---

## 2. What it COSTS — measured against the wiring, not guessed

The gate runs on **every state-changing tool call**. If its verdict entered the model's context, that
would be a per-call tax and the whole economy would be suspect. It does not. From
`agent/shell_hooks.py`:

```
hook stderr          -> logger.log(INFO)  -> agent.log   (a FILE, never the model)
exit 0  (allow/ask)  -> returns None                      (nothing enters context)
exit 2  (block)      -> returns {"action":"block","message":...}   <- the ONLY path that costs
```

Measured in the session store:

```
allow/ask calls recorded          73       0 bytes in context
seeing-log lines in context        0       (they go to ~/.hermes/logs/laya.log)
gate block messages                2       712 B total ≈ 178 tokens, ever
```

**So the gate's total context cost to date is 178 tokens, and it bought two refusals that were
correct** — the layer was unreachable during a deploy window, and refusing was the right answer.

**A counterfactual worth knowing:** had the gate returned its verdict line as context instead of
exiting 0, those 73 calls would have added ~1,750 tokens — and that text would be **re-sent every
subsequent turn**. Exit-0-plus-log-to-file is why a per-call gate is affordable at all.

---

## 3. The measurement trap this document exists to prevent

The first version of this measurement reported **46,797 tokens of gate cost** and was wrong by a
factor of 263. A `content LIKE '%the decision layer must be running%'` query matched every message
that *quotes* that phrase, including:

- a **176,907-byte context-compaction summary** — it quotes the refusal because it summarises the
  session, and it costs tokens on **every turn**, but it is a compaction cost, not a gate cost;
- my own investigation commands (`grep`, `tail`, `sed`) printing the log.

**A pattern match on a shared phrase is not attribution.** The correct query matched the block
message by its **shape** (`content LIKE '{"error": "laya could not be reached%'`), which only a real
block directive has, and then the answer was 712 bytes.

This is the same defect the project has paid for five times: **a check whose input was not taken from
the artefact it claims to measure.** Two checks reported success while checking nothing; one reported
a 263× overstated cost. When a number looks large, the first question is what it actually matched.

---

## 4. What was NOT measured (do not assume these)

- **The resume saving.** A resumed session re-sends its history rather than rebuilding it. The
  rebuild would have cost the reads and commands that established this state — this conversation
  alone read 40+ files and ran 100+ commands — but that counterfactual has no measurement here, so
  no number is claimed. What *is* established is that resuming works with the gate (see
  `02-guides/USING-LAYA-DAILY.md` §6) and that `pi --session <FILE>` is the way to do it for pi.
- **Per-file latency** inside one `ask_files` call. The row carries the call's latency.
- **Provider tokens** are 0 by construction — the head is local — so there is nothing to measure
  there. That is the strongest claim in this document and the least interesting: a local binary
  cannot spend API tokens.
