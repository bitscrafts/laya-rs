# Observability — what can be seen, and what could not

**Status**: MEASURED 2026-10-02. This document records the observability state of the
**deployed** system, measured against it rather than read from the design. The design for its
improvement is [`specs/11-observability-one-run-id-end-to-end.md`](../../specs/11-observability-one-run-id-end-to-end.md) — **specified, not yet implemented**.

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
