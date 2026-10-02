# RESUME PROMPT — laya-rs: the gate is deployed, and enforcement is unverified

**Session**: `20260915_120440_be256d` · **Repo HEAD**: `a8bf6bb` (clean) · **Written**: 2026-10-02 ~14:50Z

You are taking over this conversation. Read this file, then the two it points at. Nothing else is
required to continue.

```
1. this file                      — the conversation's state
2. laya-rs/_docs/05-operations/RESTART-2026-10-02.md   — what is deployed, what to verify
3. laya-rs/_docs/02-guides/USING-LAYA-DAILY.md         — how to use it daily
```

---

## TL;DR

**The work is done and the queue is empty.** Specs 01–14 all at 0 open criteria; 538 tests pass;
clippy 0; fmt clean; mutation gate exit 0 with 0 missed. Binaries deployed with hashes verified
against the tree.

**ONE THING IS OPEN, and it is the whole reason for the restart:** whether the laya gate actually
fires for a live session's tool calls. It was deployed, registered and correct, but the running
process could not pick it up — hooks load at process start, not session start. An agent **cannot
verify its own gating** from inside the long-lived process, which is why the check has to happen in a
session that begins after the restart.

## THE ONE THING TO DO FIRST

Make any state-changing tool call (terminal / write_file / patch / execute_code), then:

```sh
wc -l < /root/.hermes/logs/laya.log      # note the count BEFORE and AFTER the call
```

The log must **grow**, with a line like:

```
2026-10-02T14:33:03Z laya[2424021] PASS [ask] tool=terminal signals=0 cmd=<your command>
```

If it does not grow, the gate is not reaching this session either. Then, in order:

```sh
hermes hooks doctor | grep -A5 laya-gate      # must say ✓ allowlisted and ✓ ran clean
/root/.local/bin/laya-gate "git status"; echo "exit=$?"   # must be exit 0
ls -l /run/laya/sidecar.sock                  # the head must be up
hermes hooks revoke <laya-gate entry> && re-approve, then restart
```

**Do not conclude "it works" because the binary answers.** The binary answering and the hook firing
are different facts, and conflating them is precisely the mistake made on 2026-10-02 (five separate
checks reported success while checking nothing).

## What was finished today — do not redo

Six specs implemented by a delegated session via `scripts/run-spec-part.sh` (never inline), each
verified by **running its criteria**, never by reading its report:

| spec | commit | what carried it |
|---|---|---|
| 09 scheduler fail-fast | `7e4267f` | the control reproduces the original defect verbatim |
| 13 test hardening | `8c69f1e` | the orphan test, not the flag test |
| 10 deadlines | `0ef1743` | a real run bounded by its deadline: exit 1 after 1s on a `sleep 3` |
| 12 head kernel | `9ba02ad` | PyTorch differential green, tolerance untouched; 10,407 ms → 471 ms |
| 11 observability | `451b337` | a data-loss bug the spec never knew about |
| criteria ticks | `50a4b30` `fe407fe` | specs 08 and 14 were done but never marked |

Then: `b7e8fe4` (handoff), `ede5d4c` (operator's guide + deploy), `a8bf6bb` (session-lifecycle finding).

## The token measurement (the reason to use it daily)

```
ask_files over 6 real files (417,039 B)
  returned to context :  9,678 B  (~2,419 tokens)
  if read instead     : 417,039 B (~104,259 tokens)
  reduction           : 43x
  provider tokens     : 0
```

Honest limits: **~9 s per question** over six files; ranking is weak on large files; a score near 0.5
means the head does not know. Triage tool — *which file should I read* — not a "paste me the code" tool.

## Known traps (all measured today)

```sh
export LAYA_LEDGER_DIR=/var/lib/laya-ledger   # else `ledger stats` reads ./.laya and prints 0
```

- `laya ledger stats` with no env var prints `decisions: 0` while the real ledger holds 90+ rows.
- `hermes hooks doctor`'s *"script modified since approval"* is a **warning, not a disablement**.
- The `terminal` tool blocks long inline one-liners and heredocs — write a script to `/projects/_tmp/`
  and run that.
- Commits must name no AI tool in message, author **or** email; the `commit-msg` hook scans the body.
  Write the message to `.git/COMMIT_*.txt` and use `git commit -F`.
- `pi` resume is `--session <FILE>`, **not** `--session-dir` + `FRESH=0` (that silently starts a new
  session and bills it).

## Deliberately unfinished

1. **Gate enforcement in a live session** — §"THE ONE THING" above.
2. **`ask_files` is slow** (~9 s / six files, a 460 ms forward each). If daily use depends on it, the
   next step is caching embeddings per file.
3. **Three questions the ledger cannot answer**: provenance (`head_sha256` still always `None`),
   time windows (no `--since`), per-file latency in `ask_files`.

## Evidence

`/projects/_tmp/brief-spec{09,10,11,12,13}.md` · `/projects/_tmp/spec{09..13}.log` ·
`/projects/_tmp/spec-dispatch/` (session ids + pre-dispatch HEADs) · the control scripts
`/projects/_tmp/{r5-control3,spec10-control,spec11-identity-control,spec12-baseline}.sh` ·
`/projects/_tmp/mutants-out-laya-rs/mutants.out` · `/projects/_tmp/gate-tests.log` ·
`/projects/_tmp/measure-ask2.py` · `/projects/_tmp/real-surface.sh`.
