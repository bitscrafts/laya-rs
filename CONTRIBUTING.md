# Contributing

1. Open an issue with the **Task** form. Say what changes, give exit criteria as commands that exit 0
   when it is done, and list the files it may touch.
2. A maintainer marks it `ready`. Work starts from `ready` issues only.
3. Open a pull request that closes the issue. Fill in the template: paste the criteria output
   unsummarised, and name the test that fails without the change.

Before opening it:

```bash
cargo xtask gate      # fmt, clippy -D warnings, tests: what CI runs
```

Rules that CI and review hold to:

- A test is written first and must fail for the right reason before the change.
- A check that always passes does not count. A rule that skips something needs a near-miss case that
  still takes the normal path.
- A source file stays under 450 lines; split into a folder module instead of raising the limit.
- Commit messages describe the change. Identity is your own.
- Do not weaken a check to make your own change pass.

Larger work gets a spec in [`specs/`](specs/README.md) first; its exit criteria are run by
`scripts/spec-gate.sh`.
