# USING LAYA DAILY — the operator's guide

**This document answers one question: what do I run, every day, and what should I expect?**

It is written for the person (or agent) who wants laya to **save tokens** rather than to know how it
works. For the internals see `03-reference/GRAPH-SEAM.md` and `05-operations/DECISION-LEDGER.md`.

---

## 1. What it is for, in one paragraph

Laya answers a yes/no or multiple-choice question about **text you would otherwise have to read into
your context**. The model that decides is a small Rust head running locally behind a Unix socket, so a
call costs **zero provider tokens** and does not touch an API. Measured on this host, 2026-10-02: three
questions over six real files (417 KB) returned **9,678 bytes — a 43× reduction** against reading the
same files.

The saving is real for **triage**: *does this file answer X, how sure are you, which file*. It is not a
"paste me the code" tool — the answer is a label and a confidence, not the text. And it is **not fast**:
nine seconds per question over six files, because the head runs 460-token forwards one at a time on CPU.
Use it to decide *what* to read; read the thing itself when the answer matters.

---

## 2. The commands you will actually use

The real surface, read from the binaries (not remembered — an earlier draft of this guide invented an
`ask-file` subcommand that does not exist):

```
laya <doctor|decide|ledger|graph> [--url URL]
laya decide --state STATE --question JSON [--url URL] [--head PATH]
laya ledger <compact|stats|tail|outcome> [args]
laya graph  <validate|render|run> <flow.json>
laya-gate   <command> [--irreversible] [--run-id ID] [--node-id ID] [--no-ledger]
laya-mcp                                        # the MCP stdio server (7 tools)
```

**Asking about files is MCP-only.** There is no `ask-file` subcommand; `ask_file` / `ask_files` are
tools on the MCP server, called by the agent. That is deliberate: the answer is built for a model to
consume, not a shell.

### Is it up? — `laya doctor`

```sh
laya doctor
```

Expect `laya: READY` with four `ok` lines (endpoint, `/health`, `/v1/models`, head). If it is not
READY, **nothing else here works** and every gate call fails closed to `ask`.

The `ledger:` line is a warning, not an error. `laya doctor` runs as its own process and cannot see
the server's environment, so if it says `ledger: off` while the server's startup event says on, the
server is right — see §5.

### Ask a question about files — the MCP tools `ask_file` / `ask_files`

The question is a **plain string**; the server builds the typed question itself. (Verified
2026-10-02: `question` as an object map is refused with *"argument `question` must be a string, got
object"*. `decide` takes a typed map; `ask_file`/`ask_files` take a string. They differ on purpose —
do not assume one convention across tools.)

```
ask_files  { "paths": ["<glob or path>", …], "question": "<a plain string>" }
ask_file   { "path":  "<one path>",            "question": "<a plain string>" }
```

The answer is a per-file `noul` score with a band and confidence, ranked. Read the top hit; ignore the
tail. That is where the tokens are saved.

**Watch the confidence.** Measured 2026-10-02: a question about `bucket_clamped` scored **0.5121**,
which is a coin flip. The `band` and `margin` fields say so (`low`/`mid`/`high`, and `near_tie`). A
score near 0.5 means *the head does not know* — do not act on it.

### The gate — `laya-gate`

This is what makes laya **mandatory** rather than optional: a Hermes `pre_tool_call` hook refuses a
tool call when the decision layer is unreachable. It is not called by hand.

```sh
laya-gate "git status"              # exit 0 = allow
laya-gate "mkfs.ext4 /dev/sda1"     # exit 1 = ask
```

| exit | verdict | meaning |
|---|---|---|
| 0 | `allow` | the action matched the allowlist and nothing downgraded it |
| 1 | `ask` | not allowlisted, or a destructive signal, or `irreversible` — a human should confirm |
| 2 | `deny` | refused |
| 3 | `unjudgeable` | **no judgement was possible** (the head is down). Distinct from `ask` on purpose: only the caller knows what an unjudged action should cost. |

`ask` is **not an error** — a caller that treats a judgement as a failure stops calling the gate. That
is why the exit codes are separate.

---

## 3. What it costs

| | cost |
|---|---|
| provider tokens, per call | **0** |
| context tokens added | only the answer (~2.4 KB for three questions over six files), never the files |
| wall clock, gate | ~180 ms |
| wall clock, `ask_files` over 6 files / 417 KB | ~9 s per question |
| ledger row | ~330 bytes |

**The gate is on the path of every state-changing tool call.** 180 ms is affordable; the 9 s
`ask_files` is not, per call. Use `ask_files` when the alternative is reading 100 KB of files, not on
a hot path.

Measured 2026-10-02 on this host, six real files (417,039 B ≈ 104,259 tokens if read):

```
3 questions, ask_files over all six
  bytes returned to context : 9,678   (~2,419 tokens)
  bytes if the agent read   : 417,039 (~104,259 tokens)
  reduction                 : 43x
  provider tokens           : 0
```

**The honest caveat.** A 460-token forward runs at ~460 ms after spec 12, and `ask_files` embeds each
file, so the six-file call pays for six forwards per question — hence ~9 s. And the ranking is not
magic: on the largest file the head returned `0.7389` for a question that file only partly answers.
Treat the score as *"worth a look"*, not *"this is the answer"*.

---

## 4. Daily routine

```sh
laya doctor                                     # 1. is it up?
export LAYA_LEDGER_DIR=/var/lib/laya-ledger     # 2. or you will read ./.laya and see zero
laya ledger stats                               # 3. what has it decided lately?
```

For the ledger commands, set the directory or you will read the wrong one: with no `LAYA_LEDGER_DIR`
the CLI defaults to `./.laya`, which is empty, and `stats` prints `decisions: 0` — which looks like
"nothing was recorded" rather than "you asked the wrong directory". Measured: `./.laya` gave
`decisions: 0` while `/var/lib/laya-ledger` gave `decisions: 90`.

`stats` opens the store **read-only** and ingests pending segments first, so an uncompacted ledger
cannot read as empty. `--no-compact` is the pure read.

The other ledger commands:

```sh
laya ledger tail 20                                          # newest 20 rows
laya ledger outcome <question_id> --correct --source review   # the only record that enables calibration
laya ledger compact                                          # merge segments into the store
```

---

## 5. The failure modes, and what each actually means

| you see | it means | do |
|---|---|---|
| `laya: READY` on all four lines | everything works | nothing |
| a tool call refused with *"laya could not be reached"* | the gate could not judge, so it refused. **This is correct behaviour** — an unavailable gate must never mean permission | `laya doctor`; if READY, retry. A deploy window looks identical from inside the hook |
| `ledger: off` from `laya doctor` while the server says on | `doctor` is a separate process and cannot see the server's env | trust the server's startup event; see the server's stderr |
| `unjudgeable` (exit 3) | the socket exists but the head did not answer | restart the sidecar; every judgement fails closed until it does |
| `ask_files` scores cluster near 0.5 | the head does not know — the files may not contain the answer | read the top hit yourself, or rephrase with the vocabulary the file uses |
| `stats` shows `decisions: 0` | you are reading an empty directory (usually `./.laya`, not `/var/lib/laya-ledger`) | set `LAYA_LEDGER_DIR` |

---

## 6. The gate and the session lifecycle — read this before concluding "it is not firing"

**The rule is about PROCESS start, not session start.** A hook is loaded when the process that
serves your turns starts. Two measured cases from 2026-10-02, and the second corrects the first:

- **Before a restart, and NOT after one:** a hook was registered at 14:33 in a process that had
  started at 03:29. Tool calls made in that session at 14:33–14:52 produced **no** seeing-log entry,
  while driving the same hook directly with a real payload wrote lines immediately. Hook and binary
  both fine — the process serving that session had loaded its hook set before the hook existed.
- **After the restart, with a RESUMED session:** the gateway restarted at 14:50:26, the hook
  re-registered at 14:50:26, the tool-hook workers started at 14:50:27, and tool calls in the
  **resumed** session at 14:52:17 and 14:52:20 were gated and logged normally.

**So a restart fixes it, and resuming an existing session afterwards is fine.** Resuming is the right
default: the new process loads the current hook set, then re-attaches to your session. Earlier wording
in this guide said a session "already in flight" would not be gated, which is true only of the process
that predates registration, and misleading afterwards — corrected here with both measurements.

An agent **cannot verify its own gating** at the moment it matters: from inside the process there is no
way to tell whether the hook pre-dates your session. Check these four things in order:

```sh
hermes hooks doctor | grep -A5 laya-gate      # registered, executable, allowlisted, ran clean
ls -l /run/laya/sidecar.sock                  # the head is up, or every judgement fails closed
tail -3 /root/.hermes/logs/laya.log           # did the seeing log grow when you made a call?
/root/.local/bin/laya-gate "git status"       # does the binary itself answer? (exit 0 = allow)
```

If the first two are good and the binary answers but the log does not grow, the process predates the
hook: **restart the gateway, then resume your session as usual.**

**What enforcement actually covers.** Measured on 73 gated calls: `allow 27 / ask 45`. `ask` does not
block by design — a caller that treated a judgement as a failure would stop calling the gate. So the
gate **records every state-changing call** and **blocks only** when the layer is unreachable, the head
is down, or the verdict is `deny`. Everyday agent work that is not on the allowlist (`bash …`,
`cargo build`, anything scripted) returns `ask` and proceeds. The allowlist is deliberately 13
read-only entries; widening it is a policy decision, not a bug.

**A second trap, hit twice in one day:** `hermes hooks doctor` reports *"script modified since
approval"* when a hook is edited after consent. That is a **warning, not a disablement** — the
allowlist check matches on event and command only. The hook still fires. Do not chase it; fix it with
`hermes hooks revoke` + re-approve if you want the warning gone.

---

## 7. What it CANNOT answer yet (measured, not guessed)

Recorded so nobody plans around them:

- **"Which head produced this decision?"** — `head_sha256`/`backbone_sha256` are always `None`.
- **"What did the gate allow in the last hour?"** — there is no `--since` window; `stats` is all-time.
- **Per-file latency in an `ask_files` call** — the row carries the call's latency, not each file's.
- **"Why was this row dropped?"** — the identity now includes `wall_nanos` (it used to collide across
  processes), so rows are no longer silently merged, but there is no rejection log.

---

## 7. Turning it off deliberately

Two escapes, both explicit human actions:

```sh
export LAYA_GATE_DISABLE=1     # bypass the gate for this shell only
hermes hooks revoke <hook>     # remove the hook entirely
```

If you can disable it without meaning to, it is not enforcement — which is the point. The gate exists
to be inconvenient exactly once: when the layer is down.
