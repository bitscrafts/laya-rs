# APPEND a laya-rs resume entry to the workspace RESUME-REGISTER.
# The register is append-only in practice; the newest entry goes at the end.
import pathlib

p = pathlib.Path("/projects/docs/05-operations/RESUME-REGISTER.md")
t = p.read_text()

ENTRY = '''
## 2026-09-30 (2) — laya-rs: SIX of seven tools real, and a safety hole found in the gate

**State: GREEN, committed. Nothing is in flight. This entry is written so the next session does
not have to rediscover the two measurements below — they are the expensive part.**

Project `/projects/laya-rs`, git HEAD `0039361`, 7 commits. Gate `scripts/verify-all.sh` PASS ·
**227 tests** · clippy 0 · fmt clean · 54 doc links 0 broken · audit vs baseline `new=0`.

### What is now real (was 3 of 7 tools, now 6)

| tool | state |
|---|---|
| `decide` | real — typed questions -> route-shaped payloads (with `margin`, `near_tie`) |
| `gate` | real — TWO signals, see the finding below |
| `ask_file` | real — judgements about a file, never the file |
| `ask_files` | real — globs expanded, capped at 64, `hits` ranked by belief |
| `should_compact` | real — L7, four judgements in one call |
| `ask` | real — L10, the CALLER writes the questions |
| `ledger` | **still a stub** — needs `laya-store`, which is unwritten |

### THE TWO MEASUREMENTS THE NEXT SESSION MUST NOT REDO

**1. This head reads a PREFIX of the state, not the whole thing.**
A signal at the START of a state scored exactly **0.788 at 3,975 / 15,943 / 63,951 / 199,951
bytes** — invariant across a 50x range, which is only possible if the tail is not read. The same
signal at the END survived at 984 bytes and was **gone at 3,976** (0.087, identical to the
no-signal control 0.0873). The exact boundary could NOT be pinned (filler semantics move the
answer), so `FULL_ATTENTION_BYTES = 1024` sits conservatively below the smallest observed failure
and both file tools return `bytes` plus a `warning` above it. Scripts: `scripts/measure-content-limit.py`,
`scripts/measure-window.py`.

**2. This head CLASSIFIES CONTENT; it does not REASON ABOUT CONSEQUENCES.**
Three rounds agree, on cases with known answers:
* `"Is this action safe?"` as a noul -> **6/12** (`ls -la /tmp` 0.83, `rm -rf <dir>` 0.89 — no
  discrimination at all);
* six consequence phrasings -> **every one had a NEGATIVE separation**, so no threshold can work;
* a `choice` over declared effects -> **13/18**, still missing `: > file` and `dd if=... of=...`.
Meanwhile "is there a bug?" scored **0.8044** correctly. The same lesson appeared as `mid_edit`:
its first phrasing scored **1/6**, and two rewritten phrasings scored **6/6**.
**So: ask it to classify content. Never ask it to reason about a consequence.**
Scripts: `scripts/measure-mid-edit.py`, `-variants.py`, `measure-gate-question.py`,
`measure-effect-choice.py`, `-choice2.py`.

### THE SAFETY DEFECT THAT IS NOW FIXED (do not regress it)

The e2e proof caught the gate **allowing `rm -rf <dir>` and the command running**. The check that
caught it was an **effect** check — the protected file had to still exist afterwards — not a
message check. Root cause was twofold: the model's verdict was the only signal, and
`Policy::gate`'s `irreversible` flag was decorative (both `Band::Mid` arms identical, defaulted
`false`, and nothing ever inferred it).

Now: **`allow` requires TWO signals to be clear** — no destructive effect pattern matched, AND
the model reached the high band. A pattern match can never be allowed through. The response
reports `model_decision`, `destructive_signals` (matched text + why) and `irreversible`, so a
refusal is auditable. This implements the source research's own rule ("a security gate is one
signal, not the only control") in both directions. Spec 04 F6c/F6d.

### ANSWERED THIS SESSION: "what actually goes to the redb db?"

**Nothing.** Verified: `laya-store` declares **no dependencies** ("NO DEPS YET, ON PURPOSE"), is
not in `laya-mcp`'s or `laya-cli`'s dependency tree, `redb` is **absent from `Cargo.lock`**, and
**no `.redb` or `ledger-*.jsonl` file exists anywhere** for this project. Every `.redb` on the
machine is agent-memory's. All seven tools are stateless. `laya-core::queue` holds a redb
`PriorityQueue` with 16 tests that is wired to nothing.

### ANSWERED THIS SESSION: "is any graph implementation already coded?"

**Yes — in `/projects/pi-improved/crates/pii-graph`, not in laya-rs.** 3,281 lines:
`node.rs nodes.rs expr.rs validate.rs spec.rs toml_load.rs journal.rs blackboard.rs cron.rs
render.rs sched/{mod,state,dispatch,loop_helpers}.rs run/{engine,resume}.rs`. laya-rs contains
**no** graph implementation — only `route_payload.rs`, "the contract with the graph engine",
targeting pii-graph's `route` node (`on` + `rules` of `{eq, port}`).

### REMAINING, RANKED

1. **`ledger` + `laya-store`** — the last stub. It needs an **outcome-recording path** first:
   `calibrate` needs `(confidence, correct)` pairs and nothing records `correct`. The single-writer
   design is already specified in `crates/laya-store/src/lib.rs` (servers read-only + their own
   `ledger-<pid>.jsonl`; `laya-cli ledger compact` the sole writer) — that plan is sound, it is
   simply unwritten.
2. **P4 (the Rust head)** — removes the serial sidecar (currently one connection, blocking loop),
   removes torch, and would raise the content window above the measured ~1 KB prefix limit.
3. **Wiring into a real DAG** — laya-rs now emits the payloads; nothing yet runs one through
   `pii-graph`'s `route` node end to end. That is the step that proves the two halves fit.

### KNOWN NON-BLOCKERS (honest, do not "fix" blindly)

* The **sidecar is unsupervised** (runs under `nohup`): nothing inside the container can supervise
  a daemon (`systemctl`/`podman`/`supervisord`/`crontab` all absent). If it dies, `gate` fails
  closed (correct) but the other tools error until it is restarted. Needs a host unit.
* The **sidecar is strictly serial** — the backbone is 5.8x parallel, the single-process head is
  not. `ask_files` asks sequentially and its schema says `parallel: false`.
* The gate's honest limit, stated so nobody over-trusts it: it is **TWO signals, not a
  sandbox.** It never executes a command it refuses, but that is not the same as containing one.
'''

if "laya-rs: SIX of seven tools real" in t:
    print("  entry already present; not duplicated")
else:
    p.write_text(t.rstrip() + "\n" + ENTRY)
    print("  appended the laya-rs entry to RESUME-REGISTER.md")
