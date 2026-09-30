# Installing Laya in Hermes

How to register the `laya` MCP server so Hermes (and any other MCP client) can call the
decision layer as tools.

**Prerequisite**: the backbone and the head are up — see
[RUNNING-LAYA.md](RUNNING-LAYA.md). Registration is pointless if
`laya doctor` does not say READY: `laya-mcp` **refuses to start** when the layer is
unavailable, so Hermes would report a server that never connects.

---

## 1. Install the binaries

Everything lives in `~/.local/bin` (this container's convention — no root paths, no
`/usr/local`):

```bash
cd /projects/laya-rs
export PATH="/usr/local/cargo/bin:$PATH" CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
cargo build --release -p laya-mcp -p laya-cli
install -m 0755 target/release/laya-mcp /root/.local/bin/laya-mcp   # the MCP server
install -m 0755 target/release/laya    /root/.local/bin/laya        # the CLI
install -m 0755 scripts/laya-sidecar.py /root/.local/bin/laya-sidecar.py
```

`laya-sidecar` (the head launcher) comes from `scripts/setup-runtime.sh` instead — it
needs the pinned venv. See RUNNING-LAYA.md §3.

Two binaries, two roles — do not confuse them:

| Binary | Role | Consumer |
|---|---|---|
| **`laya-mcp`** | MCP JSON-RPC over stdio, long-lived, **refuses to start if Laya is down** | Hermes / pi / Claude Code |
| **`laya`** | one-shot CLI; **the exit code is the result** | you, cron, hooks, scripts |

---

## 2. Register with Hermes

```bash
printf 'Y\n' | hermes mcp add laya --command /root/.local/bin/laya-mcp
```

* The name `laya` (not `laya-mcp`) makes the tools read `mcp__laya__decide`,
  `mcp__laya__gate`. Pick the tool-call prefix you want to live with.
* **The enable prompt is interactive** and cancels on EOF — pipe `Y` as above.
* **Do not hand-edit `config.yaml`.** `hermes mcp add` writes the entry *and* the
  enable state correctly.

Verify:

```bash
hermes mcp list        # laya  /root/.local/bin/laya-mcp  all  ✓ enabled
hermes mcp test laya   # Transport: stdio → ...  ✓ Connected  ✓ Tools discovered: 7
```

The seven tools: `decide`, `ask`, `ask_file`, `ask_files`, `gate`, `should_compact`,
`ledger`. There is deliberately **no `route` tool** — a model that could pick its own
edges would defeat the design; edges belong to the graph.

---

## 3. A new session is required — and this is not a formality

**MCP clients bind their servers at session start and do not hot-reload.** After
rebuilding `laya-mcp`, an existing session keeps talking to the old process.

This is not theoretical; it happened while building this, and the tell is worth
remembering:

```
$ (new session) mcp__laya__gate -> {"decision":"allow","confidence":0.9479}
$ (old session) mcp__laya__gate -> laya-rs: `gate` is not implemented yet
                                    ^^^^^^^ the OLD binary's diagnostic prefix
```

**The diagnostic prefix identifies which build answered.** If a tool returns stale
behaviour after a rebuild, check the prefix and the process start time before believing
the new code is broken:

```bash
ps -eo pid,lstart,args | grep -E 'laya-mcp' | grep -v grep
stat -c '%y' /root/.local/bin/laya-mcp     # newer than the process => stale session
```

If several sessions are open, the old processes are harmless (they exit when their
stdin closes) — they just cannot see the new binary.

---

## 4. Using the tools

```
mcp__laya__decide          typed question -> route-shaped payload
                           {label, probabilities, confidence, band, margin, saturated}
mcp__laya__gate            may this action proceed?  -> allow / deny / ask
mcp__laya__ask_file        ask a question ABOUT a file; the file never enters your context
mcp__laya__ask_files       the same question across many files
mcp__laya__should_compact  four bounded questions about whether to compact
mcp__laya__ask             the agentic form: the CALLER writes the questions
mcp__laya__ledger          decision ledger stats / tail / outcomes
```

Implemented today: **`decide` and `gate`**. The rest return an honest
`not implemented yet` error — which is deliberate, because a tool that "succeeds" while
doing nothing is how a caller comes to trust a fabricated answer.

### The design rule to respect when you call it

**Code for numbers and thresholds; Laya for bounded judgments; the LLM for open work.**
Laya supplies a *value*; it never executes control flow. Keep token budgets, thresholds
and loop counts in your code — only bounded, typed judgements go to Laya.

### Failure behaviour, by contract

| Situation | `gate` | `decide` |
|---|---|---|
| Laya up | real decision | real answer |
| Laya down | **`ask`** — never `allow` | **protocol error** — never a made-up answer |
| sidecar dead | **`ask`** | protocol error naming the socket |

**An unavailable gate must never mean permission.** Verified live: with the socket
pointing at a path that does not exist, `gate` returns
`{"decision":"ask","reason":"sidecar-unavailable"}` while `decide` errors.

---

## 5. Cost and latency, so you can decide when to call it

| Path | Warm latency | Cost |
|---|---|---|
| Laya decision (whole layer) | **~0.4–0.7 s** | R$0 — local, no egress |
| llama.cpp `/embedding` alone | **35 ms** median | R$0 |
| 8 concurrent `/embedding` | 121 ms wall (5.8× parallel) | R$0 |

Consequences worth acting on:

* **~0.5 s per decision is fine per TASK, poor per TOOL CALL.** Batch questions — send
  four questions in one call rather than four calls.
* The head is **persistent** because it loads in **~24 s**; a per-call spawn would pay
  that every time.
* Laya runs on CPU with **no GPU** (`torch.cuda.is_available()` is False here). That is
  a design target, not a limitation being worked around.

---

## 6. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `laya` missing from `hermes mcp list` | `mcp` package absent in the **Hermes venv** → discovery silently skipped | `/root/.hermes/hermes-agent/venv/bin/python -c "import mcp"`; install if missing |
| Connected, 0 tools | server started but the tool router failed | `hermes mcp test laya`, then check the server's stderr |
| Server will not connect at all | **preflight refused** — Laya or the head is down | `laya doctor`; note `laya-mcp` exiting is *correct*, not a bug |
| Tools return stale behaviour | session bound to an old process | new session; check the diagnostic prefix |
| `tool_call 'arguments' is not valid JSON` | very long argument through the MCP wrapper | write it to a file and use `laya` / the sidecar directly |
| Env vars not seen by the server | Hermes passes only baseline vars to stdio servers | add them explicitly under `env:` in the server's config block |

---

## 7. Verified state

```
hermes mcp list       laya  /root/.local/bin/laya-mcp  all  ✓ enabled
hermes mcp test laya  ✓ Connected (859 ms)   ✓ Tools discovered: 7
in-session call       mcp__laya__gate -> {"decision":"allow","band":"high",
                                          "confidence":0.9479,"saturated":false}
```

Last verified 2026-09-30. Re-run `hermes mcp test laya` before trusting this line.
