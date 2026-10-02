# Observability — what can be seen, and what could not

**Status**: MEASURED 2026-10-02. This document records the observability state of the
**deployed** system, measured against it rather than read from the design. The design for its
improvement is [`specs/11-observability-one-run-id-end-to-end.md`](../../specs/11-observability-one-run-id-end-to-end.md) — **specified, not yet implemented** when this was measured; **R1–R9 were implemented 2026-10-02** and the operator's view is in the final section, "Spec 11: what an operator can ask now". Everything above that section describes the system as it was measured, and is kept as the record.

**Why this document exists.** The same question was asked three times in different words — *"can you
check usage?", "token economy?", "is it usable on everyday tasks?"* — and each time the answer required
a measurement that had never been taken. It is written down so the next person starts from numbers.

---

## 1. The headline: nothing was recorded, and nothing said so

`LAYA_LEDGER_DIR` was **unset**, and absent from the MCP server entry in the host configuration. So
`record_decision`'s first line — `let dir = state.ledger_dir.as_ref()?;` — returned early on every
call, for the entire life of the deployment, **without a single signal**.

| what was asked | what it returned |
|---|---|
| `laya ledger stats` | `decisions: 0 · outcomes: 0 · latency p50/p95/max: unknown` |
| host session store (133 207 messages, ~65 000 tool calls) | **3** calls to any `mcp_laya_*` tool |
| `ledger-*.jsonl` on the machine | none, outside throwaway test directories |

**The mechanism was never broken — only unconfigured.** With the variable set on the same call, `decide`
writes a segment immediately:

```json
{"tool":"decide","question_id":"paid","label":"yes","band":"mid","confidence":0.7872,"latency_ms":478}
```

The lesson is worth more than the fix: **a subsystem that is silently off is indistinguishable from one
that is broken.** Both produce zero rows. Only one of them is a defect. Nothing in the system could tell
them apart, which is why spec 11 R4 requires the state to be *announced* — a startup event and a
`laya doctor` line — and why its criterion must assert the **warning** when the directory is unset, not
merely that a healthy server prints `enabled: true`.

---

## 2. What the operating configuration now is

Set in the host's MCP server entry for `laya` (not in this repo — the repo stays standalone):

| variable | value | why |
|---|---|---|
| `LAYA_LEDGER_DIR` | `/var/lib/laya-ledger` | without it the ledger is off, silently |
| `LAYA_READ_ROOTS` | `/projects:/root` | colon-separated; without it the scope defaults to the **cwd**, so `ask_file` refuses most real paths |

The read scope is a security boundary, and it behaves as one: with the roots above, a file under
`/projects` is read and `/etc/hostname` is refused with a message that names the path and says the scope
*"is a security boundary, not a hint"*. Both were verified.

---

## 3. Usage measurement, and its limit

The host's `state.db` is the only usage record that exists, because the ledger records nothing. It
answers with `tool_name`:

```sql
select tool_name, count(*) from messages where tool_name like '%laya%' group by tool_name;
```

**Caveat, stated because it bounds every number here:** counting calls is not the same as measuring
value. The ledger — once enabled — records *what was decided* (question id, label, band, confidence,
latency), which is what an operator actually needs. Tool-call counts say only that something was
invoked.

---

## 4. The token economy, measured

Each tool was called against the deployed server; the returned JSON was measured.

| tool | bytes | ≈tokens | seconds |
|---|---|---|---|
| `ask_file` (noul) | 189 | 47 | 0.63 |
| `ask_file` (choice) | 189 | 47 | 0.52 |
| `ask_files` | 376 | 94 | 0.50 |
| `decide` | 274 | 69 | 0.25 |
| `gate` | 489 | 122 | 0.18 |
| `should_compact` | 910 | 228 | 0.91 |

**median ≈ 325 bytes ≈ 81 tokens, 0.51 s.**

The decision itself costs **no model tokens**: the head runs locally on CPU. So the saving is structural
— a decision that replaces a model call replaces its entire cost with ~81 tokens of context.

**The honest caveat:** the saving is real *only if the call replaces a model turn*. Where an agent would
not have asked a model anyway, the cost is ~81 tokens for an answer it did not need. **The break-even is
one avoided model turn per used call.** No measurement exists yet of how often that happens in real
work — that is what spec 11's per-tool stats are for.

---

## 5. What cannot be answered today

Each of these needs spec 11; each is a question the measurements above provoked:

| question | why not |
|---|---|
| which decision sent run R down branch B? | `run_id` is `None` on every row; nothing carries it |
| what did `gate` allow in the last hour, and did `ask` run it? | `gate` and `ask` have **no** `record_decision` call site |
| is a run slow because nodes are slow, or because `limit` starves them? | `duration_ms` starts **after** the semaphore permit |
| did a node time out, or fail? | no node timeout exists (spec 10) |
| which head or backbone produced this decision? | `head_sha256`/`backbone_sha256` are always `None` |
| p99 latency per tool? | `Stats` is global; no per-tool split |
| is the ledger even on? | silently off, and nothing says so |

The two cheapest fixes with the largest return are **R1** (propagate `(run_id, node_id)` end to end) and
**R2** (record every judging tool). R1 is the single join that turns two logs into one story; R2 is what
makes the safety-critical decisions auditable at all.

---

## 6. What was fixed while measuring this

- `LAYA_LEDGER_DIR` and `LAYA_READ_ROOTS` set on the host's `laya` MCP entry, and both verified to
  change behaviour.
- All seven tools exercised end to end through the deployed server: **10 of 10 checks behaved as
  required**, including the read-scope refusal that must still fail.
- Two defects recorded against other specs, both the same class — *a refusal that sends the caller to
  fix the wrong thing*:
  - `ask_file`/`ask_files` with a non-string `question` reports **``requires `path` ``** although `path`
    was supplied (spec 13 R5);
  - `choice`/`score` without `criteria` reports a bare `AttributeError`/`TypeError` that names neither
    the field nor the question (recorded in
    [`../04-development/TEN-LEVELS-MAPPING.md`](../04-development/TEN-LEVELS-MAPPING.md)).

---

## 7. Reproducing any of this

The probes were written as scratch and are listed here because the numbers above should be repeatable:

| what | how |
|---|---|
| all seven tools, deployed env | `/projects/_tmp/usable-probe.py` |
| the ten levels | `/projects/_tmp/verify-levels.py` |
| cost per call | `/projects/_tmp/token-economy.py` |
| the ledger records at all | `/projects/_tmp/ledger-proof.py` |
| the question-shape matrix | `/projects/_tmp/shape-matrix.py` |

They live in `_tmp/` **deliberately, and they must not become load-bearing**: nothing in the repo may
depend on them existing. The durable checks are the exit criteria in the specs.

## The deployment records, but nothing had ever compacted (measured 2026-10-02)

The distinction that matters when reading `stats` on this box: **is the ledger off, or is it on with
nothing ingested?** They look identical from the outside, and the earlier note in this doc ("the
ledger was off for the life of the installation") is now the *other* state. Corrected by measurement:

```
deployed server pids 1068293 / 1068334 / 1070199
  LAYA_LEDGER_DIR=/var/lib/laya-ledger      <- present, so recording IS on
  LAYA_READ_ROOTS=/projects:/root

on disk:  7 segment files, 7 rows (ledger-<pid>.jsonl, one per probing process)

laya ledger stats --dir /var/lib/laya-ledger     (reads ledger.redb)
  decisions:      0      outcomes: 0      latency p50/p95/max: unknown ms

laya ledger compact --dir /var/lib/laya-ledger   (the step nobody had run)
  segments read: 7   records seen: 7   inserted: 7   already present: 0

laya ledger stats --dir /var/lib/laya-ledger     (same command, after)
  decisions:      7      band low 1 / mid 6      near ties: 1
  latency p50:    226 ms   p95: 353   max: 353
```

**So the recorded answer is 7 decisions — not 0.** The rows were on disk the whole time, in the write
path's append-only segments, and `stats` reads `ledger.redb`, which only `compact` fills. An operator
running `stats` on this box gets "unknown" and is told nothing can be calibrated, while seven real
decisions sit unread one directory away.

This is **by design, and the design is documented** — `DECISION-LEDGER.md:59` states compaction is an
explicit act, not a background task, "easy to reason about, easy to test". What follows from it was
not stated anywhere: **`stats` cannot see a single row until someone runs `compact`.** Two consequences
worth knowing before answering any question from this ledger:

1. **"unknown latency" does not mean no data.** Check the segment count first:
   `ls /var/lib/laya-ledger/*.jsonl | wc -l`. If it is non-zero while `stats` says 0, the store needs
   compacting, not investigating.
2. **No scheduled compaction exists on this host.** Verified: no systemd timer, no crontab entry, no
   Hermes job for `laya` (the only job naming it is the specs 09–13 implementation). Compaction is
   therefore manual, and until it runs, every question in the table at the top of this document is
   answered from an empty store.

**The honest one-line answer to "is observability implemented?"**: *the write path is on and provably
records; the read path is honest and reports `unknown` rather than zeros; and there is no ingestion
step, so nothing an operator asks is answered until someone compacts by hand.* The instrumentation
spec 11 adds — per-tool blocks, run/node ids, wall-comparable ordering — is what makes the read path
worth running; the missing compaction step is the operational half, and it is a one-line unit or a
documented ritual, not a code change.

## Working, measured on the live deployment (2026-10-02, commit b59b88d)

The two states that used to be indistinguishable — ledger OFF vs ledger nobody has INGESTED — are
now separable, because a read ingests first and a live check exercises the deployed binary.

```
uv run scripts/check-observability.py     -> exit 0, ten passes against the running install
```

```
=== deployed laya observability ===
  PASS  tools_exposed                7 tools
  PASS  gate_answers                 decision='allow'
  PASS  gate_records_a_row           segments 0->1, rows 0->1
  PASS  row_carries_run_id           run_id='obs-check'
  PASS  row_carries_wall_clock       wall_nanos=1790910327198556023
  PASS  row_carries_tool             tool='gate'
  PASS  command_text_absent          only a hash of the action, never the action
  PASS  stats_reads_what_is_on_disk  9 decision(s)
  PASS  stats_has_per_tool_block     the per-tool breakdown is printed
  PASS  stats_shows_gate_verdicts    the gate's verdict counts appear
VERDICT: the ledger is recording AND readable
```

### What an operator now sees, and what it cost

```
$ laya ledger stats --dir /var/lib/laya-ledger
decisions:      9
outcomes:       0
saturated:      0
near ties:      1
band low        1
band mid        8
latency p50:    226 ms
latency p95:    353 ms
latency max:    353 ms

per tool:
  decide           7     p50 226 ms  p95 353 ms  max 353 ms
  gate             2     p50 225 ms  p95 229 ms  max 229 ms
    allow        1
    yes          1
    executed     0

laya: ingested 1 row(s) from 1 segment(s) before reading
```

`gate p50 225 ms` is the real cost of a gate call, measured; `decide p50 226 ms` likewise. Zero
model tokens are spent producing those numbers.

**The `yes: 1` is deliberate and is not a bug.** It is a row written *before* the fix, when the
gate recorded the model's own yes/no label instead of the gate's decision. Historical rows are not
rewritten — the ledger is append-only evidence, and a metric that silently rewrote its own past
would be worth less than one that shows the change. The new row reads `allow`.

### The four defects this closed, all found by looking rather than trusting

1. **Only `decide` wrote a row.** `gate`, `ask`, `ask_file`, `ask_files` and `should_compact`
   recorded nothing, so nothing about the tools an agent actually calls was answerable.
2. **A refusal was invisible.** The refusal paths return early, so what the gate *denied* — the
   row an incident review wants first — left no trace at all.
3. **Ordering was not comparable across processes.** See the `wall_nanos` note in
   `DECISION-LEDGER.md`; rows now sort by epoch time, not by a per-process counter.
4. **A missing confidence read as `0.0`** — indistinguishable from maximally unsure, and fatal to
   any calibration built on the column.

### Two checks that passed while the thing they named was absent

Both were caught by running the live check and **reading its output** rather than its exit code,
and both are worth stating because they are the failure mode this project keeps paying for:

- A check named `stats_has_per_tool_rows` asserted `"p50" in stats` — a substring the *blended*
  latency lines already contain. It passed while the CLI printed no per-tool block at all. It now
  asserts `"per tool:"`, the block itself.
- The gate's row recorded an **empty label** on the fail-closed path, because the verdict there
  arrives as `decision` and the recorder read only `label`. A row existed and said nothing.

### What is still not implemented

The seven questions at the top of this document are now answerable *except* two: **"what did the
gate allow in the last hour"** has no time-windowed query (`tail` is newest-first, `stats` is
all-time), and **"which head produced this decision"** is unanswerable because `head_sha256` and
`backbone_sha256` are always `None` and no requirement fills them. Both are recorded in
`specs/11` as open, with criteria. `run_id` threading from the graph runner is also still open —
the field exists and the tools accept it, but nothing in `laya-graph` supplies it yet.

## Is it USED? A different question from "does it work" (measured 2026-10-02)

`laya ledger stats` answers *did the tool run*. It cannot answer *is the tool used*, because a
tool that is never called writes no rows — and a tool that is called and a tool that is ignored
both show a healthy, empty ledger. Only the caller's own history can answer it.

Measured against the agent session store on this host — every tool call ever made:

```
tool calls, all tools      63,886
tool calls, laya                3   (0.0047%)

by tool:   mcp__laya__gate   3
by day :   2026-09-29        2
           2026-09-30        1

every call:
  2026-09-29T23:48:13Z  mcp__laya__gate   (action: 'rm -rf node_modules && npm i')
  2026-09-29T23:48:13Z  mcp__laya__gate   (the paired result)
  2026-09-30T00:03:07Z  mcp__laya__gate
```

**So: three calls, ever, and none in the last day.** Both of the first two were made *before* the
spec-08 fix, i.e. against the gate whose ordering was inverted — the one that returned `allow` for
`mkfs.ext4 /dev/sda1`. Nothing has called it since the fix landed.

### Why, and it is not a bug in laya

Driving the deployed gate with the commands an agent actually issues:

```
command                        verdict  model
git status                     allow    allow
ls -la                         allow    allow
cargo test                     allow    allow
cargo build --release          ask      allow     <- the build an agent needs
mkdir -p /tmp/x                ask      allow
rm -rf target/                 ask      allow
git push --force               ask      allow
curl -s https://example.com    ask      allow
sed -i 's/a/b/' file.txt       ask      allow
chmod 777 /etc/passwd          ask      ask
```

Every verdict is **correct** — nothing destructive is allowed. But `allow` requires a positive
match against a **13-entry read-only list** (`Policy::gate_allow`: `ls`, `pwd`, `cat`, `head`,
`tail`, `wc`, `git status|diff|log|show`, `cargo check|test|clippy`). Anything outside it gets
`ask`, and an agent that must stop and ask before `cargo build` will simply not call the gate.

**So the honest answer to "can it be used on everyday tasks" is: it answers correctly, and it is
too conservative to be worth calling.** Widening the allowlist is a policy decision, not a code
change — the list is the thing that decides what an agent may do unattended — so it is left to
the operator rather than widened here.

### The review commands

Two scripts, one question each, and they live on opposite sides of the boundary on purpose:

| question | command | where it lives |
|---|---|---|
| is it recording, and what did it cost? | `laya ledger stats` | in the project — laya's own ledger |
| is it being USED, by whom, when? | `~/.hermes/scripts/laya-usage.py` | Hermes-side |

The usage script is deliberately **outside** this repository: it reads the agent session store,
and laya must not know the agent exists. It reads that store **read-only** and reports both ledger
locations, because reporting only one is how the "7 rows on disk, `stats` says 0" state went
unnoticed — a review that counted only segments or only the store would call either case healthy.

## Mandatory usage, and the seeing log (2026-10-02)

The measured state this answers: **3 calls to `mcp__laya__gate` out of 63,886 tool calls ever.**
A decision layer registered, enabled, and never called is indistinguishable from one that is
broken — so usage was made mandatory where it can be, at the operator's explicit direction.

### The mechanism is a `pre_tool_call` hook

`~/.hermes/hooks/laya-gate.sh`, registered with **`fail_closed: true`** — the only combination
Hermes enforces. (Verified: `_BLOCKING_EVENTS = frozenset({'pre_tool_call'})`; `fail_closed` is
silently ignored on every other event. Same finding as the agent-memory gate.)

It fires on the four tools that can change state — `terminal`, `write_file`, `patch`,
`execute_code` — and asks the deployed `laya-mcp` one question first.

**What it enforces:**

| condition | result |
|---|---|
| laya unreachable (no binary, no socket) | **BLOCK** — an unavailable gate must never mean permission |
| the head is down (`reason: sidecar-unavailable`) | **BLOCK** — laya is up but cannot judge, which is the "stop if not running" case |
| verdict `deny` | **BLOCK** |
| verdict `allow` / `ask` | pass, and the call was recorded |

**What it deliberately does NOT enforce, and this matters more than the table above:**

- **`ask` does not block.** `ask` is the gate doing its job, not a refusal. Blocking on it would
  refuse routine work — and an agent made to beg permission for `cargo build` stops calling the
  gate entirely, which is precisely how usage died the first time. This hook exists to make the
  gate *used*, not to make it a wall.
- **Destructive signals do not block.** That list contains `"mv "`, `"find "` and `"rm -f"` —
  routine here, and this project's own hard rule is to `mv` rather than delete. Blocking on
  signals would block the safe path while the unsafe one passed. Signals are *recorded*, not
  enforced.

### The seeing log

`~/.hermes/logs/laya.log` — one line per gated call, written by the hook, and mirrored to stderr
so it also reaches the agent's own log:

```
2026-10-02T03:31:44Z laya[1110417] PASS [allow] tool=terminal signals=0 cmd=pwd
2026-10-02T03:31:49Z laya[1110548] PASS [allow] tool=terminal signals=0 cmd=ls -1 crates
2026-10-02T03:28:58Z laya[1109534] SKIP (same command within 5s) tool=terminal cmd=git log -1
2026-10-02T03:28:58Z laya[1109563] BLOCK (laya unreachable) tool=terminal cmd=cargo build
```

and, in `agent.log`, through the stderr channel:

```
INFO agent.shell_hooks: shell hook stderr (command=/root/.hermes/hooks/laya-gate.sh):
     laya-gate: PASS [allow] tool=terminal signals=0 cmd=pwd
```

**Two deliberate design points.** The hook rate-limits the *same* command in the *same* session
within 5 seconds — that is the same decision, and without it a build loop would spend a gate call
and a ledger row per repetition. And it writes the log file **directly** rather than relying on
the logger, because a hook's stderr is emitted by Hermes at `debug` while the gateway runs at
`WARNING` (measured: `agent.log` held **0 DEBUG lines**), so a line printed by a hook would not
have appeared anywhere. That level was also inconsistent with blocking hooks, which already log
at `INFO`; it is now `INFO`, with `HERMES_HOOK_STDERR_DEBUG=1` to demote it for a chatty hook.

### Cost, measured

```
gate rows in the ledger      30
gate latency  min/p50/p95/max   0 / 174.5 / 231 / 268 ms
verdicts answered           {allow: 16, ask: 13, yes: 1}

provider tokens per gated call     0
model context tokens added         0
```

**Zero tokens, and that is structural rather than lucky:** the gate runs a local Rust head over a
Unix socket. No provider API is called and nothing is added to the model's context window, so the
marginal token cost of gating every state-changing tool call is zero. What is actually spent is
~175 ms of wall clock (median) and one ledger row of ~330 bytes. The `yes: 1` is a pre-fix row,
written before the gate recorded its own verdict instead of the model's label.

### Reviewing it

Three commands, each answering a different question:

| question | command |
|---|---|
| is the ledger recording, and what did it cost? | `laya ledger stats` |
| is laya being **used**, by whom, when? | `~/.hermes/scripts/laya-usage.py` |
| what does the mandatory gate **cost**, in tokens and time? | `~/.hermes/scripts/laya-token-economy.py` |

All three are read-only. The two review scripts live on the agent side of the boundary, never in
this repository: laya must not know the agent exists.

**One caution the scripts now handle explicitly.** The MCP tool counter in the session store only
sees calls the *model* makes through the tool surface. The hook calls the gate as a *subprocess*,
which never appears there — measured: **3 MCP calls against 30 gate rows**. A review reporting
only the MCP number would say "laya is unused" while the hook was gating every state-changing call
on the box. Both counters are therefore reported.

## The gate blocked a live agent, and the cause was the deploy (2026-10-02)

An agent was asked to run three commands. Its FIRST call at **03:41:24** was refused:

```
{"error": "laya could not be reached, so this state-changing tool call is refused. ..."}
```

The retry at **03:44:43** succeeded. Nothing was wrong with laya: the refusal fell inside the
two-minute window in which the `laya-gate` binary was being replaced on disk (`cp` to a temp name,
`mv` into place) while a live agent was calling it. Measured afterwards: the built and deployed
binaries hash identically (`cd8c2665…`), and no block has been logged since.

**The gate behaved exactly as designed.** It refused rather than letting an unjudged action
through, which is the whole point of `fail_closed` — and the agent was told how to check
(`laya doctor` said READY, the retry worked). A transient deploy window is indistinguishable from a
real outage *from inside the hook*, and refusing is the correct choice for both.

**What this cost, and the lesson.** A deploy takes seconds; an agent blocked by one loses its turn.
Worth knowing before replacing a binary that sits on the path of every state-changing tool call:

```
# before replacing, check for live callers
pgrep -x laya-gate    # or: lsof ~/.local/bin/laya-gate
```

`mv` into place is still the right mechanic — it leaves running processes on the old inode rather
than failing them mid-call, which is why the block was a clean refusal and not a crash. But the
*new* invocations between `cp` and `mv` see a missing file, and that window is what blocked the
agent.

### The log is append-only evidence, and this session truncated it

Mid-run I cleared `laya.log` by hand to get a clean reading of the next exercise. That destroyed
the record of the block above — the one block the log had ever held, and exactly the entry an
incident review would ask for. The hook now **rolls the file aside** at a size cap
(`LAYA_GATE_LOG_MAX`, default 10 MB) into `laya.log.old-<timestamp>` instead of ever truncating it,
so every past verdict stays readable. Verified with a roll at a 1-byte cap: the old content
survived in the rolled file and the new line went to a fresh log.

**A note on concurrency, measured:** the sidecar is strictly serial, so eight concurrent gate calls
queue linearly — 0.17 s, 0.31, 0.48, 0.64, 0.81, 0.98, 1.15, 1.32 s — and all eight were answered
`allow`. Load does not make the gate fail closed; the queue simply lengthens, and the hook's 10 s
timeout is far above what this work generates.

---

## Spec 11: what an operator can ask now (2026-10-02)

**Is the ledger on?** The server writes one line to stderr at startup, whichever way it is configured:

```json
{"ts":…,"event":"ledger","level":"info","enabled":true,"dir":"/var/lib/laya-ledger","dir_exists":true}
{"ts":…,"event":"ledger","level":"warn","enabled":false,"dir":null,"message":"ledger is OFF: LAYA_LEDGER_DIR is unset, …"}
```

The OFF line is a `warn` and names the variable. `laya doctor` adds `ledger: …` as a *warning* line — never a
failure — but it can only see **its own** shell's `LAYA_LEDGER_DIR`, not the server's; the server's own
view is the startup event. The two are complementary. `scripts/check-observability.py` reads the variable
from the host's MCP entry, requires the startup event to say `enabled: true`, and exits **1** when it does not
(control: `LAYA_OBS_SIMULATE_OFF=1`).

**What did call N do, and for which run?** One line per tool call, same stream:

```json
{"ts":…,"event":"tool_call","seq":7,"tool":"gate","run_id":"run-42","node_id":"triage","latency_ms":252,"outcome":"ok","decision":"ask"}
```

`seq` is a per-process counter (the request id). `ts` is epoch **milliseconds**. `run_id`/`node_id` are `null`
for a call that supplied none. `decision` appears for `gate`. The ids are the point: `laya graph run` now
passes the run and node id as tool arguments, so a ledger row joins its `RunReport` on `(run_id, node_id)`.

**What did the gate let through, and what did it stop?** `laya ledger stats` prints per tool: count, p50 / p95 /
p99 / max latency, and for `gate` and `ask` the `allow` / `ask` / `deny` counts (from `gate_decision`), the
`refused` count, and how many actually `executed`. Refusals and errors are rows (`label` `refused` / `error`),
so "what did the gate deny" is a count, not an absence.

**Where did the time go in a run?** `NodeRecord` carries `queued_ms` (spawn → permit: waiting) and
`started_at_ms` (offset from run start); `RunReport` carries `wall_ms`. A run starved by `limit` now reads
differently from a run of slow nodes.

### Two things measured while building this, both contrary to the spec

* **`stats` cannot answer *during* a compact by being read-only.** redb's read-only handle also takes the
  file lock: opening one while a writer holds the store fails `AlreadyOpen`, exactly as a second writer does
  (readers coexist with readers, not with a writer). So `stats` now opens read-only **and waits** up to 5 s for
  the writer to finish, and reports `AlreadyOpen` after that. `stats`/`tail` still ingest pending segments
  first, which does take the write lock briefly; `--no-compact` is the pure read.
* **The identity of a row ignored the clock that distinguishes processes.** See `DECISION-LEDGER.md`,
  "Row identity".

### Still not answerable

* **Which head produced a decision.** `head_sha256` / `backbone_sha256` are still always `None`; no requirement
  in spec 11 fills them, and nothing here does either.
* **"What did the gate allow in the last hour?"** `stats` is all-time and `tail` is newest-first over the whole
  store; there is no `--since`.
* **Per-file latency for `ask_files`.** Its rows carry the whole call's latency.
