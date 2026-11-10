# Handoff

Current state of laya-rs. The long history of how it was built is in
[`_docs/04-development/BUILD-LOG-2026-09-to-10.md`](_docs/04-development/BUILD-LOG-2026-09-to-10.md);
what each part is for is in [`README.md`](README.md) and the README in each crate.

## State

- Eight crates plus `xtask`. `cargo xtask gate` (fmt, clippy `-D warnings`, tests) passes.
- Workspace tests: 657 passed, 0 failed, 11 ignored (they need the live model server).
- Pure-Rust decision path: `laya decide` runs the head in-process (about 1.2 s end to end with a
  release build, including loading the head).
- Still on the Python sidecar: `laya-mcp` and `laya-gate`. Replacing it removes the sidecar from
  the install guide and the prefix limit on `ask_file`.
- `laya` has no `--version` flag yet; bug reports ask for the commit hash.

## Specs

34 numbered specs in [`specs/`](specs/). Each has a card on line 2 (`#: depends= status= block=`).
Run one with `scripts/spec-gate.sh specs/<n>-*.md`; `FAST=1` skips the slow criteria and says so.

Measured when this was written (criteria run, not the card trusted):

| spec | result |
|---|---|
| 10, 11, 12, 28, 29 | all criteria pass; cards set to `done` |
| 08 | 8 of 9: the live corpus check fails |
| 09, 13 | all fast criteria pass; slow ones not run |
| 14 | 12 of 13: the ten-levels live check fails |
| 07 | no runnable criteria found |
| 15 to 27 | card status `-` (written before cards existed) |

## Open

- [Issue #1](https://github.com/bitscrafts/laya-rs/issues/1): the install guide. Written; closes
  when the branch is pushed.
- Live checks in `scripts/live/` need the model server and the head; they report UNMEASURED
  rather than pass when it is down. Two specs (08, 14) still fail on them.
- Spec 24 needs a built `target/debug/laya`: its criteria say so only implicitly.
- Spec 30 has one criterion that names a script outside this repository.
- History: 35 early commits carry another author name and 118 an old address; rewriting them
  needs a force-push and is the owner's decision.
- Nothing is published to crates.io. The names `laya`, `laya-rs` and `laya-core` are taken there;
  the first release is a GitHub release with prebuilt binaries (`release.yml`, on a `v*` tag).

## How work is tracked

Issues and pull requests. A task is an issue using the Task form; it becomes `ready` when the
owner marks it (`scripts/mark-ready.sh <n>`). See [`CONTRIBUTING.md`](CONTRIBUTING.md).
