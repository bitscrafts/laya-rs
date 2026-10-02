# RESUMING THE OPUS ANALYSIS SESSION

**Why this document exists.** The operator's standing rule (2026-09-21, verbatim): *"we should never
use `claude -p` (one run) as this way we cant reattach to the session!!! so we could use the previous
context to further analysis!"* — and *"after finishing the spec implementation we should re run
claude-opus for a re analysis (using the previous session info)"*. Analysis accumulates. A review
round that starts fresh re-derives what the previous round already established, and pays for it twice.

This is the exact procedure, the session ids, and the two failure modes that cost us a round on
2026-10-02.

---

## The session

| | |
|---|---|
| **session id** | `d58b577f-fc19-48d5-b690-f6d7524ea4f8` |
| model | `opus` (claude CLI 2.1.287; Opus 5.5 needs ≥ 2.1.280) |
| dispatcher | `scripts/run-opus-graph-analysis.sh` (checked in, reviewable, re-runnable) |
| state file | `/projects/_tmp/opus-graph-analysis/session.id` — holds the id above |
| round 1 | `/projects/_tmp/opus-graph-analysis/REPORT.md` (20,786 B, 40 turns, $2.45) |
| round 2 | `/projects/_tmp/opus-graph-analysis/ROUND2.md` |

Round 1 found the C1–C5 contradictions in `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md`.

## Round 1 — a fresh round

```sh
cd /projects/laya-rs
BRIEF=/projects/_tmp/brief-opus-graph-analysis.md \
REPORT=/projects/_tmp/opus-graph-analysis/REPORT.md \
  MAX_TURNS=120 scripts/run-opus-graph-analysis.sh
```

## Round 2 — RESUMING (the normal case)

```sh
cd /projects/laya-rs
BRIEF=/projects/_tmp/brief-opus-round2.md \
REPORT=/projects/_tmp/opus-graph-analysis/ROUND2.md \
  MAX_TURNS=140 scripts/run-opus-graph-analysis.sh --resume
```

`--resume` reads the id from the state file; you never retype it. If you do retype it by hand:

```sh
claude --resume d58b577f-fc19-48d5-b690-f6d7524ea4f8 -p "<brief pointer>"
```

**The prompt is mandatory.** A bare `--resume` is read as "continue a deferred session" and dies.

## What the script prints before spending anything

```
model   : opus (claude CLI 2.1.287 (Claude Code))
repo    : /projects/laya-rs
brief   : /projects/_tmp/brief-opus-round2.md  (7190 bytes, delivered as a POINTER not inlined)
outdir  : /projects/_tmp/opus-graph-analysis
report  : /projects/_tmp/opus-graph-analysis/ROUND2.md  (checked after the run)
session : d58b577f-fc19-48d5-b690-f6d7524ea4f8
quota   : five_hour … seven_day …
```

**Read this block before it runs.** Especially `brief` and `session`: a wrong brief means a whole
round spent doing the previous round's job.

`BRIEF`, `REPORT`, `OUTDIR` and `MAX_TURNS` are all overridable environment variables. The brief is
passed as a **pointer** — claude reads the file itself. Never inline it: measured, an inline brief
arrived truncated from 3,577 bytes to 248.

## Exit codes

| code | meaning |
|---|---|
| 0 | the report was written by **this** run |
| 3 | **the dispatch FAILED** — the report is missing or older than 15 minutes, i.e. the run exited without producing output |

Exit 3 exists because of the failure below. A round that exits 0 having written nothing reads exactly
like a round that succeeded.

---

## The two failure modes that cost us a round (2026-10-02)

**F1 — the tools were blocked, so it answered from prior context.**
The script invoked claude with `--allowedTools "Read,Grep,Glob,Bash,Write"`. That is **not** a
permission grant. `claude -p` in print mode then **stuck on its first write**, asked for approval, and
— with its turn count still available — answered the question from the previous round's context
instead. It looked like a completed 5-turn run costing $2.67 and produced **no output at all**.

- **Fix:** `--permission-mode bypassPermissions` (and `export IS_SANDBOX=1`, required for that as
  root). `--allowedTools` alone is not enough. This same pitfall was already recorded in amem on
  2026-09-09 for the spec dispatcher — `run-spec-part.sh` has carried the flag all along, and this
  script was written without it. **Compare against the working dispatcher before trusting a new one.**
- **Detection:** the script now **fails (exit 3)** when the report is missing or stale, instead of
  printing `NOT WRITTEN` and exiting 0.

**F2 — the prompt named the round-1 brief.**
`BRIEF` was overridable in the preflight, but the `PROMPT` string **hardcoded** the round-1 brief path
and `REPORT.md`. So the round-2 dispatch printed `brief: …brief-opus-round2.md` and then instructed
claude to read `…brief-opus-graph-analysis.md`. Claude correctly observed *"Same request as before"*.

- **Fix:** `$BRIEF` and `$REPORT` are interpolated into the prompt.
- **Lesson (this project's oldest one):** the preflight printed what I *intended*, and the payload
  carried something else. **A preflight that reports a value the payload does not use is worse than
  no preflight** — it certifies the wrong thing. The check must read the live path.

## Verifying a resumed round actually did new work

Three checks, cheapest first:

1. **Turn count and wall time.** Round 2's broken attempt: **5 turns, 24 s**. Round 1: **40 turns**.
   A resumed round that answers a substantial brief in 5 turns did not do it.
2. **The report is newer than the run started**, and the script's exit code is 0 (not 3).
3. **The session file grew.** The transcript is
   `/root/.claude/projects/-projects-laya-rs/d58b577f-fc19-48d5-b690-f6d7524ea4f8.jsonl`.
   Byte-compare before and after: a resume that starts a *new* session leaves the old file frozen.
   (This is the same check as `pi --session <FILE>`: did the old file **grow**, or did a new file
   appear beside it?)

## Related

- `_docs/04-development/OPUS-REVIEW-GENERIC-GRAPHS.md` — round 1's findings, with the re-verification.
- `_docs/04-development/GENERIC-GRAPHS-ANALYSIS.md` — my analysis, which round 1 attacked.
- `HARD RULE 0e` in `/projects/AGENTS.md` — RESUME FIRST; resuming is compatible with every hook.
