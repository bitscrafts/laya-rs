# GRAPH-DETERMINISM — comparison is a `check`, judgement is a `decide`

**Rule: comparison → `check`; judgement → `decide`.** A `decide` must never be used where a `check`
can answer.

A `decide` is a model call. The head is measurably good at *judgement* and measurably wrong at
*comparison* — at exactly the boundaries a spec decision needs. A `check` runs a command and the
**exit status** selects the port; no model, no network, no `laya-mcp`, so it runs with the sidecar
down.

## Why: three measured failures (2026-10-02)

| question put to the head | truth | head's answer | consequence |
|---|---|---|---|
| "is the number of unfinished criteria equal to 0?" | **1** unfinished | `yes` @ band low 0.1115 | releases with one spec open |
| "did the build succeed?" | build exited **7** | `yes` @ band mid 0.79 | proceeds on a failed build |
| "is the guess higher than the secret?" | guess **is** the secret | `higher` | the guessing game cannot be won |

The band is no escape hatch: `low` holds both a correct answer (0.0603) and the wrong one (0.1115),
so there is no honest way to escalate on doubt. Each question above is one line of shell:
`[ "$n" = 0 ]`, `cargo test`, `[ "$guess" = "$secret" ]`.

## The `check` node

```json
{"id": "closed", "kind": "check",
 "run": "[ \"$(cat /tmp/count)\" = \"0\" ]",
 "depends_on": ["count"],
 "ports": {"pass": "release", "fail": "work_left", "_": "escalate"}}
```

* exit **0 → `pass`**; any other exit → `fail`; a key absent from `ports` → `_`.
* **A non-zero exit is the answer, not an error:** the node succeeds, the run continues, and the
  exit status is recorded in the node's `exit_code`.
* `run` is required; `ports` must carry a `_` catch-all and may only use `pass`, `fail`, `_`;
  `allow_failure` is refused.
* What *does* fail the node (and stops the run): a timeout, death by signal, and exit 126/127 —
  what `sh` reports when the command could not be executed at all. "The check did not run" and
  "the check said no" are different answers. A script that deliberately exits 126 or 127 is
  therefore indistinguishable from a missing command; pick another status for `fail`.
* Only the selected port runs; the others are named in `skipped` (spec 17's rules).
* Targets may be a shell, a `decide`, or another `check`.

## What a `check` cannot do

* Judge. It answers only what a command can decide; "is this summary good?" stays a `decide`.
* Fall back to a model. If it cannot run, it fails by name.
* Distinguish non-zero exit codes: every non-zero exit is `fail`. To branch on one specific code,
  fold the test into the command: `cmd; [ $? -eq 7 ]`.
* Read an upstream node's output directly: it has no `{{node}}` substitution. Pass state through
  a file or environment, as the example flows do.

## Examples (run with the sidecar down)

* `_docs/02-guides/wc-classify-check.flow.json` — the 300-word boundary, exact at 300/301.
* `_docs/02-guides/guess-the-number.check.flow.json` — `guess == secret` reaches `win`.
* `_docs/02-guides/specs-closed.check.flow.json` — one open criterion takes `fail`.
