# THE EXAMPLES NEED `laya-mcp` ON PATH — measured, and it is NOT intermittent

**Measured 2026-10-02.** Found by chasing what I first called an intermittent failure of the decide
node. **It was never intermittent.** It is deterministic and depends entirely on the caller's `PATH`.

## The measurement, three ways, same flow, same machine, same minute

```
WITHOUT ~/.local/bin on PATH   -> FAILED gt300 | failed to spawn `laya-mcp` (set LAYA_MCP_BIN if it is not on PATH): No such file or directory (os error 2)
WITH    ~/.local/bin on PATH   -> ALL OK
WITH LAYA_MCP_BIN=~/.local/bin/laya-mcp  -> ALL OK
```

`command -v laya-mcp` prints `NO` in the first case and a path in the second.

## Why it looked like a flake — and the lesson

A `decide` node lowers to an `mcp` node, and the CLI **spawns `laya-mcp` as a child process**. It
resolves that name through the **caller's** `PATH`. So:

- scripts of mine that began `export PATH=~/.local/bin:$PATH` ran clean;
- scripts that did not, failed at the **first decide**, every time.

I observed both and called the difference **intermittent**, which sent me hunting a race for several
checks. It was not a race: it was my own scripts disagreeing about `PATH`.

**The rule this is the ninth instance of: when a check disagrees with production, suspect the check
first — and before concluding "flake", compare the ENVIRONMENT the passes and the failures ran in.**
A nondeterministic cause and a differing environment look identical from a single result, and only one
of them is a real defect. Here the environment differed, and the "flake" was an artefact of my own
tooling.

## What this means for the documented examples

Every worked example in `_docs/02-guides/` runs a `decide`, so **every one of them needs `laya-mcp` on
`PATH`** (or `LAYA_MCP_BIN` set). A reader who follows the documented command in a shell without
`~/.local/bin` gets:

```
laya graph run: STOPPED EARLY at `gt300` … failed to spawn `laya-mcp`
```

The documents do not say this. Until the root fix lands, the requirement is stated where a reader will
hit it.

## The root fix (filed, not done here)

**`laya` should find `laya-mcp` beside itself.** A binary installed in a bin directory next to its
sibling should prefer that sibling, then `PATH`, then `LAYA_MCP_BIN` — rather than requiring the caller
to have arranged the environment. Filed as `specs/22-a-binary-finds-its-sibling.md`.

Reasoning for the order: the sibling is the build that shipped **with this binary**, so preferring it
removes the "deployed CLI spawns a different, older server" failure mode as well — the same class as the
stale-binary trap that cost three specs a criterion correction today.

## Not claimed

- Whether `cargo run` from the workspace behaves the same — it would put `target/debug` on `PATH` and
  so mask the problem, which is likely why it went unnoticed through several verification passes.
- Whether the installed `laya` and the installed `laya-mcp` are ever mismatched in practice — they are
  deployed together by `scripts/archive/deploy-binaries.sh`, but nothing in the binary enforces it.
