# laya-rs — spec-13: test hardening — hermetic default suite, a mutation gate, offline property and fuzz tests

- **Project**: `/projects/laya-rs`
- **Priority**: P1 for R1–R2 (cheap, and each closes a measured hole); P2 for R3–R5.
- **Status**: SPECIFIED — not yet dispatched
- **Source**: architecture review 2026-10-01 (`REPORT.md` §4); mutation run `mutants/mutants.out/`
- **Depends-On**: 08 (R2's `agentic.rs` targets assume `gate_verdict` exists), 09 (R3's scheduler invariants include `not_started`).

---

## Verified Premises

- `crates/laya-graph/tests/acceptance.rs:72-80, 91-110`: the acceptance tests run on **plain `cargo test`**. They `assert!` that the backbone answers `/health`, and that a `laya-mcp` binary exists next to the test executable. **Measured:** in cargo-mutants' clean copy of the tree, the baseline `cargo test` failed on exactly these two asserts (`mutants.baseline-failed/mutants.out/log/baseline.log`: `panicked at crates/laya-graph/tests/acceptance.rs:107:5`). So `cargo test --workspace`, and therefore "414 tests green", **depends on the network and on build order.** AGENTS rule 9: *"Anything needing Laya to judge meaning is a live test and is kept separate."*
- **Mutation testing** (cargo-mutants 27.1.0 is installed at `/usr/local/cargo/bin/cargo-mutants`), run over `sched.rs`, `policy.rs` and `agentic.rs` with `--lib`: **128 mutants: 96 caught, 18 MISSED, 5 timeout, 9 unviable** (`mutants/mutants.out/missed.txt`). Among the missed:
  - `policy.rs:279:26 replace match guard irreversible with true` **and** `… with false`: no test can tell whether `irreversible` does anything (spec 08 R5).
  - `policy.rs:305:18 replace / with *` in `weighted_score`: the weighted mean is only tested with weights summing to 1, where `/1 == *1`.
  - `agentic.rs:349:33 replace && with ||` in `destructive_signals`: no control for "push without force" or "`-f` without push".
  - `agentic.rs:161-164` (7 mutants) in `clip_stream`: the truncation boundary that bounds `ask`'s state is unasserted.
  - `sched.rs:52:29 replace == with !=` in `topo_order`: no graph with in-degree ≥ 2 is order-checked (spec 09).
  - 5 `sched.rs` mutants were caught only by **cargo-mutants' own timeout**: the tests hang rather than fail.
- **Offline registry** (`/usr/local/cargo/registry/cache/index.crates.io-*/`): **no** `proptest`, `quickcheck`, `arbitrary`, `libfuzzer-sys`, `criterion` or `loom`; `cargo-fuzz` is not installed. The project's dependency comments treat "already in the local registry, no network fetch" as a criterion (`Cargo.toml:41`).
- `crates/laya-mcp/src/lib.rs:1584`: `serde_json::from_value(args_value).unwrap_or_default()`. A wrongly-typed argument (`"action": 123`) deserialises to `RawArgs::default()` and surfaces as *"requires `action`"*, which names the wrong problem. No test sends a wrongly-typed argument.
- `crates/laya-head/src/safetensors.rs:363-375, 295-313`: header length and offsets are bounds-checked, and unit tests cover truncation (`:924-952`). No randomized input has ever been run through `from_bytes`.

---

## Overview

The suite is strong where it is differential (the head) and weak where it is example-based and
asserts presence rather than value. Four changes, ordered by value per effort:

1. **Hermetic by default.** Live tests become `#[ignore]` and run in the gate's live step. A red
   `cargo test` then always means code, never network.
2. **A mutation gate on the safety-critical files.** The tool is already installed. Zero missed
   mutants on `policy.rs`, `agentic.rs` and `sched.rs`.
3. **Property tests without a new dependency.** A 30-line seeded generator in each crate's test
   module: deterministic, offline, reproducible from the printed seed. No shrinking. Adopt
   `proptest` only if shrinking is ever needed, and record the network fetch in the manifest the
   way `fancy-regex` was.
4. **Structured fuzzing as tests.** Byte mutation of real fixtures through every parser. The
   property is *no panic, a typed error or `Ok`*.

---

## Requirements

### Functional

- **R1: live tests are opt-in.** The three tests in `crates/laya-graph/tests/acceptance.rs` carry
  `#[ignore = "live: needs the backbone and the sidecar; run with --include-ignored"]`.
  `scripts/verify-all.sh` step 7c runs them with `--include-ignored`, and it **still fails** (not
  skips) when the backbone is down. That keeps N7/G23 of spec 07.
- **R2: a mutation gate.** `scripts/mutants-gate.sh` runs `cargo mutants --cargo-test-arg=--lib` on
  `crates/laya-core/src/{policy,agentic,route_payload}.rs` and `crates/laya-graph/src/sched.rs`, into
  an output dir under `/projects/_tmp/`, and exits with cargo-mutants' code. It is added to
  `verify-all.sh` as a **separate, opt-in step** (`--with-mutants`), because it takes minutes. The
  tests that kill today's missed mutants are part of this spec (TDD table).
  **The script must set `TMPDIR` and pass `--gitignore true`, and both are load-bearing.** By default
  cargo-mutants copies the whole tree into `$TMPDIR` **including gitignored files**, so a run copies
  this repo's `.venv-laya` — 5.4 GB — into `/tmp`. Measured 2026-10-01: that filled the container's
  root filesystem and the run died with `No space left on device` while copying
  `nvidia/cusparselt/lib/libcusparseLt.so.0`, leaving a 6.0 GB orphan behind and root `/` at 95%.
  Root `/` has ~22 GB free; `/projects` has ~188 GB. So the script uses
  `TMPDIR=/projects/_tmp/mutants-tmp`, passes `--gitignore true`, and removes the copy it creates.
  A gate that can fill the disk and then fail at random is worse than no gate: the failure lands on
  whichever run is unlucky rather than on the change being tested.
- **R3: offline property tests** (seeded LCG, ≥ 256 cases each, the seed printed on failure):
  - `route_payload`: for random probability vectors (k ∈ 2..13, summing to 1), `margin ≥ 0`,
    `margin == top1 − top2`, `label ∈ options`, `near_tie ⇔ margin ≤ policy.near_tie_margin`.
  - `policy`: `band_for(kind, c)` is monotone non-decreasing in `c`, for both kinds.
  - `agentic` (after spec 08): metamorphic. For every destructive seed command, inserting spaces,
    splitting or recombining short flags, and changing `-r`↔`-R` never removes the signal. For every
    allowlisted command, appending any shell metacharacter and any suffix never yields `Allow`.
  - `sched` (model-based): random DAGs (n ≤ 12, edges only i→j with i<j), random failing subset,
    random `allow_failure`, random `writes`, limit 1..4, with a recording `McpCaller` that sleeps
    0–2 ms. Invariants: (i) a node ran ⇒ every dependency ran and (succeeded ∨ allow_failure);
    (ii) peak in-flight ≤ limit; (iii) peak in-flight writers ≤ 1; (iv) no failures ⇒ all n ran;
    (v) `stopped_early` ⇒ `records + not_started == n` (spec 09 R2).
  - `tokenizer`: random strings (ASCII, mixed-script, random bytes via `from_utf8_lossy`): `encode`
    returns, every id < vocab size, and two encodes are equal.
- **R4: structured fuzz tests** (seeded byte mutation of real inputs, ≥ 2 000 cases, no panic):
  `laya_graph::validate` + `lower` over mutated `tests/fixtures/*.json`;
  `SafetensorsFile::from_bytes` over a mutated small valid file (built in the test);
  `laya_mcp::handle` (`lib.rs:445`) over mutated JSON argument objects.
- **R5: a wrongly-typed argument is named.** `dispatch_shim` reports
  *"argument `action` must be a string, got number"*. It does not default.
  **Measured 2026-10-02, after spec 08 landed — the current behaviour is worse than this
  requirement states, and in the exact way the surrounding code says it wants to avoid.**
  `ask_file` declares `question` as a string in its advertised schema, and the server type-checks it
  as a string. Sending an **object** (the natural guess for a typed question, and what a caller who
  has only read `decide`'s shape would send) produces:

  ```
  ask_file {"path": "<valid path>", "question": {…object…}}
    -> error -32602: "`ask_file` requires `path`"
  ```

  `path` **was** supplied. The refusal names the wrong argument, so a caller fixing what they are
  told to fix gets the same error again. The comment above that check already states the hazard —
  *"the error a caller sees would be about their PATH, sending them to fix the wrong argument"* —
  and the current code does precisely that, because `missing_required` asks
  `args.question.as_deref().is_none_or(str::is_empty)` and reports `path` first when the type is
  wrong. A wrong type must be reported as a wrong type, naming the field and the type found; that is
  what this requirement is for, and one criterion should be added:
  `ask_file`/`ask_files` with a non-string `question` must name `question` and the type received.


### Non-Functional

- **N1: no new dependency** for R1–R5.
- **N2: the default `cargo test --workspace` makes zero network calls.** It is checked by running it
  with the backbone URL unreachable (R1 criterion).
- **N3:** property and fuzz tests finish in < 10 s total in debug, so they stay in the default suite.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| R1 | (attribute) | `cargo test -p laya-graph --test acceptance` | `0 passed; 0 failed; 3 ignored` |
| R2 | `weighted_score_divides_by_a_non_unit_total` | `[(2.0, 1.0), (2.0, 0.0)]` | `Some(0.5)` (kills `/`→`*` and `/`→`%`) |
| R2 | `push_without_force_and_dash_f_without_push_are_clean` | `git push origin main`, `ls -f` | no signal (kills `&&`→`\|\|`) |
| R2 | `clip_stream_keeps_exactly_the_cap_and_names_the_rest` | `MAX_STREAM_BYTES + 5` ASCII bytes; then a 3-byte char straddling the cap | prefix length exact; `5 more bytes`; valid UTF-8 |
| R3 | `payload_invariants_hold_for_random_distributions` | 256 seeded cases | all invariants |
| R3 | `band_is_monotone_in_confidence` | 256 seeded pairs | monotone |
| R3 | `scheduler_invariants_hold_for_random_dags` | 200 seeded plans | (i)–(v) |
| R3 | `tokenizer_never_panics_and_ids_are_in_vocab` | 512 seeded strings | ok |
| R4 | `mutated_flows_are_refused_or_accepted_never_panic` | 2 000 mutations | no panic |
| R4 | `mutated_safetensors_are_refused_never_panic` | 2 000 mutations | no panic |
| R5 | `a_wrongly_typed_argument_is_named_not_swallowed` | `gate {"action": 123}` | error names `action` and `number` |

---

## Exit Criteria

**New, each watched to FAIL on `b0ee704`:**

- [x] `cd /projects/laya-rs && cargo test -p laya-graph --test acceptance 2>&1 | grep -qE '^test result: ok\. 1 passed; 0 failed; 2 ignored'`: the TWO live tests are opt-in; the third connects only to a closed loopback port and is hermetic, so it must RUN. The original criterion demanded `3 ignored`, and that count made a hermetic test get skipped to satisfy an arithmetic — the criterion was serving a number instead of a property
- [ ] `cd /projects/laya-rs && LAYA_ACCEPTANCE=1 cargo test -p laya-graph --test acceptance -- --include-ignored 2>&1 | grep -qE '^test result: ok\. 3 passed'`: the control — the live tests must RUN when opted in, not merely be marked: R1 (today: `3 passed` when live, `2 failed` when not)
- [ ] `cd /projects/laya-rs && bash scripts/mutants-gate.sh`: R2 (today: no such script; the equivalent command exits 2 with 18 missed)
- [x] `cd /projects/laya-rs && cargo test -p laya-core --lib payload_invariants_hold_for_random_distributions 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib scheduler_invariants_hold_for_random_dags 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R3
- [x] `cd /projects/laya-rs && cargo test -p laya-graph --lib mutated_flows_are_refused_or_accepted_never_panic 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R4
- [x] `cd /projects/laya-rs && cargo test -p laya-mcp --lib a_wrongly_typed_argument_is_named_not_swallowed 2>&1 | grep -qE '^test result: ok\. 1 passed'`: R5

  **Control for R1**: the live tests must still run, and still be able to fail, when asked for.

- [x] `cd /projects/laya-rs && grep -qE 'acceptance.*--include-ignored' scripts/verify-all.sh`: the gate's live step opts in (today: absent)

**Carried re-checks:**

- [x] `cd /projects/laya-rs && cargo test --workspace`
- [x] `cd /projects/laya-rs && cargo clippy --workspace --all-targets -- -D warnings`

---

## Guardrails

- **G1: do NOT turn a live test into a skip.** `#[ignore]` moves it to an explicit step. That step must still fail loudly (spec 07 N7/G23).
- **G2: do NOT add `proptest`/`arbitrary`/`libfuzzer-sys` in this spec.** They are not in the offline registry (N1). If a later spec adds one, record the fetch and the reason in the manifest.
- **G3: a property test that never fails has not been shown to work.** For each R3 test, inject the defect it targets once (e.g. swap `margin` operands) and paste the failing seed in the PR, as part 1 R5 did.
- **G4: fixed seeds only.** A failing case must reproduce from the printed seed.

---

## Files to Modify

| File | Change | Req |
|------|--------|-----|
| `crates/laya-graph/tests/acceptance.rs` | `#[ignore = …]` | R1 |
| `scripts/verify-all.sh` | live step `--include-ignored`; `--with-mutants` | R1, R2 |
| `scripts/mutants-gate.sh` | new | R2 |
| `crates/laya-core/src/{policy,agentic,route_payload}.rs` | mutant-killing tests; property tests | R2, R3 |
| `crates/laya-graph/src/{sched,validate}.rs` | model-based test; flow fuzz | R3, R4 |
| `crates/laya-head/src/{tokenizer,safetensors}.rs` | property + fuzz tests | R3, R4 |
| `crates/laya-mcp/src/lib.rs` | typed argument errors; dispatch fuzz | R4, R5 |
