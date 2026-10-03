//! Spec 26: the committed examples are RUN by the suite, not only validated.
//!
//! `validate.rs` proves every example file parses and validates; nothing proved one *does* the
//! thing its document says. This file expands every committed `*.flow.json` with the loader the
//! CLI uses and runs it with a scripted caller — no head, no sidecar, no `laya-mcp`, no network —
//! then asserts WHICH branch it took. A test that only checked "no node failed" would pass a flow
//! that routed everything to its catch-all, so every row below names the nodes that must run and
//! the nodes that must be skipped.
//!
//! The caller is an oracle over the decide call's own arguments: it reads the number out of the
//! prompt the graph built and answers the question about it. That is what lets the count-driven
//! flows be proven to follow the value (R3): a prompt with the number baked in answers the same
//! for every document.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use laya_graph::{
    CallContext, Library, McpCaller, RunOptions, RunReport, expand, lower, run_flow, validate,
};
use serde_json::{Value, json};

/// Every integer in `text`, in order.
fn numbers(text: &str) -> Vec<i64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Answers a `decide` the way an ideal head would, from the arguments alone, with a fixed band.
struct Oracle {
    band: &'static str,
}

impl Oracle {
    fn answer(arguments: &Value) -> Result<&'static str, String> {
        let text = |k: &str| arguments.get(k).and_then(Value::as_str).unwrap_or_default();
        let (prompt, question) = (text("prompt"), text("question").to_lowercase());
        let options: Vec<&str> = arguments
            .get("options")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let nums = numbers(prompt);
        let yes_no = |b: bool| Ok(if b { "yes" } else { "no" });
        if let Some(at) = question.find("greater than ") {
            let threshold = numbers(&question[at..]).first().copied();
            return match (nums.first(), threshold) {
                (Some(n), Some(t)) => yes_no(*n > t),
                _ => Err(format!("no count in prompt {prompt:?}")),
            };
        }
        if question.contains("guess higher than the secret") {
            return match nums.as_slice() {
                [secret, guess] => yes_no(guess > secret),
                _ => Err(format!("need secret and guess in prompt {prompt:?}")),
            };
        }
        if options == ["SHORT", "MEDIUM", "LONG"] {
            return match nums.first() {
                Some(n) if *n <= 100 => Ok("SHORT"),
                Some(n) if *n <= 300 => Ok("MEDIUM"),
                Some(_) => Ok("LONG"),
                None => Err(format!("no count in prompt {prompt:?}")),
            };
        }
        if options == ["passed", "failed"] {
            return Ok(if prompt.contains(" 0 failed") {
                "passed"
            } else {
                "failed"
            });
        }
        Err(format!("the oracle has no rule for question {question:?}"))
    }
}

impl McpCaller for Oracle {
    fn call(&self, tool: &str, arguments: &Value, _ctx: &CallContext) -> Result<String, String> {
        if tool != "decide" {
            return Err(format!("unexpected tool {tool}"));
        }
        let label = Self::answer(arguments)?;
        Ok(json!({"payload": {"label": label, "band": self.band}}).to_string())
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Shell nodes resolve `_docs/...` against the working directory, as they do from the CLI.
fn enter_repo_root() {
    std::env::set_current_dir(repo_root()).unwrap();
}

/// Expand, validate, lower and run `text` the way the CLI does, with the oracle as the head.
fn run_text(text: &str, band: &'static str) -> Result<RunReport, String> {
    enter_repo_root();
    let raw: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let flow = expand(&raw, &Library::new()).map_err(|e| e.to_string())?;
    validate(&flow).map_err(|e| e.to_string())?;
    let graph = lower(&flow).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(run_flow(
        &graph,
        RunOptions {
            run_id: "examples".to_string(),
            limit: 4,
            mcp: Arc::new(Oracle { band }),
        },
    ))
    .map_err(|e| e.to_string())
}

/// Authored node ids only: a decide lowers to `id` + `id/route`, and the route half is not
/// something an example's author names.
fn authored<'a>(ids: impl Iterator<Item = &'a String>) -> BTreeSet<String> {
    ids.filter(|id| !id.ends_with("/route")).cloned().collect()
}

fn ran(report: &RunReport) -> BTreeSet<String> {
    authored(report.records.iter().map(|r| &r.node_id))
}

fn skipped(report: &RunReport) -> BTreeSet<String> {
    authored(report.skipped.iter())
}

/// R1: the run completed.
fn completed(report: &RunReport) -> Result<(), String> {
    let failed: Vec<&str> = report
        .records
        .iter()
        .filter(|r| !r.success)
        .map(|r| r.node_id.as_str())
        .collect();
    if !failed.is_empty() {
        return Err(format!("nodes failed: {failed:?}"));
    }
    if report.stopped_early {
        return Err(format!("stopped early at {:?}", report.failed_node));
    }
    if !report.not_started.is_empty() {
        return Err(format!("not started: {:?}", report.not_started));
    }
    Ok(())
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| (*s).to_string()).collect()
}

/// One expected outcome (R2): a flow, how it is run, and exactly what it must do.
struct Case {
    /// Row name, for the failure message.
    name: &'static str,
    /// Flow file, relative to the repository root.
    file: &'static str,
    /// Exact text replacements applied before the run, to point an environment-driven flow at a
    /// document or a spec directory (each must match, or the row is vacuous).
    subst: &'static [(&'static str, &'static str)],
    /// The band the scripted head reports.
    band: &'static str,
    ran: &'static [&'static str],
    skipped: &'static [&'static str],
}

const GUIDES: &str = "_docs/02-guides";
const FIXTURES: &str = "crates/laya-graph/tests/fixtures";

const SHORT: &str = "_docs/02-guides/wc-example/short-note.txt";
const LONG: &str = "_docs/02-guides/wc-example/long-note.txt";
const WC_FILE_ENV: &str = "${WC_FILE:-_docs/02-guides/wc-example/short-note.txt}";
const GUESS_ENV: &str = "guess=\\\"${GUESS:-41}\\\"";
const GUESS_40: &str = "guess=\\\"40\\\"";
const SPECS_ENV: &str = "${SPECS_DIR:-specs}";
const SPECS_OPEN: &str = "crates/laya-graph/tests/fixtures/specs-open";
const SPECS_CLOSED: &str = "crates/laya-graph/tests/fixtures/specs-closed";
/// The criteria command in `sdd-tdd-gate` runs `cargo test` from an absolute path — neither
/// hermetic nor portable — so the row replaces it with the result line it would print.
const SDD_CMD: &str = "cd /projects/laya-rs && cargo test -p laya-graph --lib a_failure_stops_queued_siblings_from_starting 2>&1 | grep -E '^test result:' | head -1";

/// The expected outcomes. Sources: the runs recorded in `GRAPH-EXAMPLE-GUESSING.md` (guess pair:
/// `advise_lower` / `advise_higher`) and `GRAPH-EXAMPLE-WORDCOUNT.md` (13 words -> short branch,
/// 412 words -> long branch), `routing_order.rs` for the p6/p7/p13 fixtures, and a run of each
/// remaining flow read against its own description.
fn cases() -> Vec<Case> {
    let chained = |name, file, ran, skipped| Case {
        name,
        file,
        subst: &[],
        band: "high",
        ran,
        skipped,
    };
    vec![
        Case {
            name: "guess-the-number: 57 > 41 -> lower",
            file: "_docs/02-guides/guess-the-number.flow.json",
            subst: &[],
            band: "high",
            ran: &["setup", "judge", "advise_lower", "emit_move"],
            skipped: &["advise_higher", "escalate"],
        },
        Case {
            name: "guess-the-number control: 3 < 41 -> higher",
            file: "_docs/02-guides/guess-the-number.control.flow.json",
            subst: &[],
            band: "high",
            ran: &["setup", "judge", "advise_higher", "emit_move"],
            skipped: &["advise_lower", "escalate"],
        },
        Case {
            name: "guess check: equal -> win",
            file: "_docs/02-guides/guess-the-number.check.flow.json",
            subst: &[],
            band: "high",
            ran: &["equal", "win"],
            skipped: &["miss", "escalate"],
        },
        Case {
            name: "guess check: unequal -> miss",
            file: "_docs/02-guides/guess-the-number.check.flow.json",
            subst: &[(GUESS_ENV, GUESS_40)],
            band: "high",
            ran: &["equal", "miss"],
            skipped: &["win", "escalate"],
        },
        Case {
            name: "specs-closed check: work left -> work_left",
            file: "_docs/02-guides/specs-closed.check.flow.json",
            subst: &[(SPECS_ENV, SPECS_OPEN)],
            band: "high",
            ran: &["closed", "work_left"],
            skipped: &["release", "escalate"],
        },
        Case {
            name: "specs-closed check: nothing open -> release",
            file: "_docs/02-guides/specs-closed.check.flow.json",
            subst: &[(SPECS_ENV, SPECS_CLOSED)],
            band: "high",
            ran: &["closed", "release"],
            skipped: &["work_left", "escalate"],
        },
        Case {
            name: "sdd-tdd-gate: criteria passed -> merge_green",
            file: "_docs/02-guides/sdd-tdd-gate.flow.json",
            subst: &[(
                SDD_CMD,
                "echo 'test result: ok. 1 passed; 0 failed; 0 ignored'",
            )],
            band: "high",
            ran: &["run_criteria", "judge", "merge_green"],
            skipped: &["report_red", "escalate_unparsed"],
        },
        Case {
            name: "sdd-tdd-gate: criteria failed -> report_red",
            file: "_docs/02-guides/sdd-tdd-gate.flow.json",
            subst: &[
                (
                    SDD_CMD,
                    "echo 'test result: FAILED. 0 passed; 1 failed; 0 ignored'",
                ),
                (
                    "`test result: ok. 1 passed; 0 failed; 0 ignored`",
                    "`test result: FAILED. 0 passed; 1 failed; 0 ignored`",
                ),
            ],
            band: "high",
            ran: &["run_criteria", "judge", "report_red"],
            skipped: &["merge_green", "escalate_unparsed"],
        },
        Case {
            name: "wc-classify-measured: short note -> class_short",
            file: "_docs/02-guides/wc-classify-measured.flow.json",
            subst: &[],
            band: "high",
            ran: &["count_words", "gt300", "class_short"],
            skipped: &["class_long"],
        },
        Case {
            name: "wc-classify-measured: long note -> class_long",
            file: "_docs/02-guides/wc-classify-measured.flow.json",
            subst: &[(WC_FILE_ENV, LONG)],
            band: "high",
            ran: &["count_words", "gt300", "class_long"],
            skipped: &["class_short"],
        },
        Case {
            name: "wc-classify-check: short note -> class_short",
            file: "_docs/02-guides/wc-classify-check.flow.json",
            subst: &[],
            band: "high",
            ran: &["over300", "class_short"],
            skipped: &["class_long", "escalate"],
        },
        Case {
            name: "wc-classify-check: long note -> class_long",
            file: "_docs/02-guides/wc-classify-check.flow.json",
            subst: &[(WC_FILE_ENV, LONG)],
            band: "high",
            ran: &["over300", "class_long"],
            skipped: &["class_short", "escalate"],
        },
        Case {
            name: "wc-classify: short note -> report_short",
            file: "_docs/02-guides/wc-classify.flow.json",
            subst: &[],
            band: "high",
            ran: &["count_words", "classify", "report_short"],
            skipped: &["report_medium", "report_long", "escalate_fallback"],
        },
        Case {
            name: "wc-classify control: long note -> report_long",
            file: "_docs/02-guides/wc-classify.control.flow.json",
            subst: &[],
            band: "high",
            ran: &["count_words", "classify", "report_long"],
            skipped: &["report_short", "report_medium", "escalate_fallback"],
        },
        Case {
            name: "wc-classify-band: confident band -> report_confident",
            file: "_docs/02-guides/wc-classify-band.flow.json",
            subst: &[],
            band: "high",
            ran: &["count_words", "classify", "report_confident"],
            skipped: &["escalate_low_confidence", "escalate_unparsed"],
        },
        Case {
            name: "wc-classify-band: low band -> escalate_low_confidence",
            file: "_docs/02-guides/wc-classify-band.flow.json",
            subst: &[],
            band: "low",
            ran: &["count_words", "classify", "escalate_low_confidence"],
            skipped: &["report_confident", "escalate_unparsed"],
        },
        chained(
            "wc-classify-chained: 13 words -> class_short",
            "_docs/02-guides/wc-classify-chained.flow.json",
            &[
                "count_words",
                "gt300_gate",
                "gt300_ask",
                "gt100_gate",
                "gt100_ask",
                "class_short",
            ],
            &[
                "class_medium",
                "class_long",
                "escalate_gt300",
                "escalate_gt100",
            ],
        ),
        chained(
            "wc-classify-chained control: 412 words -> class_long",
            "_docs/02-guides/wc-classify-chained.control.flow.json",
            &["count_words", "gt300_gate", "gt300_ask", "class_long"],
            &[
                "gt100_gate",
                "gt100_ask",
                "class_short",
                "class_medium",
                "escalate_gt300",
                "escalate_gt100",
            ],
        ),
        Case {
            name: "p6: outer says no -> inner_B pruned",
            file: "crates/laya-graph/tests/fixtures/p6-file-order.flow.json",
            subst: &[],
            band: "high",
            ran: &["outer_A", "A_no"],
            skipped: &["inner_B", "B_yes", "B_no"],
        },
        Case {
            name: "p6c: same graph, other file order, same outcome",
            file: "crates/laya-graph/tests/fixtures/p6c-file-order-control.flow.json",
            subst: &[],
            band: "high",
            ran: &["outer_A", "A_no"],
            skipped: &["inner_B", "B_yes", "B_no"],
        },
        Case {
            name: "p7: a node behind a pruned dependency is skipped",
            file: "crates/laya-graph/tests/fixtures/p7-join.flow.json",
            subst: &[],
            band: "high",
            ran: &["prep", "d", "short_path"],
            skipped: &["long_path", "needs_long"],
        },
        Case {
            name: "p13: two routes sharing targets both reach them",
            file: "crates/laya-graph/tests/fixtures/p13-shared-target.flow.json",
            subst: &[],
            band: "high",
            ran: &["d1", "d2", "handle_yes", "handle_no"],
            skipped: &[],
        },
    ]
}

/// The flow's text, with the row's replacements applied. A replacement that matches nothing
/// would leave the row running the unmodified flow, so it is an error.
fn load(file: &str, subst: &[(&str, &str)]) -> Result<String, String> {
    let mut text =
        std::fs::read_to_string(repo_root().join(file)).map_err(|e| format!("{file}: {e}"))?;
    for (from, to) in subst {
        if !text.contains(from) {
            return Err(format!("{file}: substitution {from:?} matches nothing"));
        }
        text = text.replace(from, to);
    }
    Ok(text)
}

/// R2 for one row, on an already-loaded flow text: it completed AND ran/skipped exactly the
/// expected sets. The error names the row and the difference.
fn check_case(case: &Case, text: &str) -> Result<RunReport, String> {
    let report = run_text(text, case.band).map_err(|e| format!("{}: {e}", case.name))?;
    completed(&report).map_err(|e| format!("{}: {e}", case.name))?;
    let (want_ran, want_skipped) = (set(case.ran), set(case.skipped));
    let (got_ran, got_skipped) = (ran(&report), skipped(&report));
    if got_ran != want_ran || got_skipped != want_skipped {
        return Err(format!(
            "{}: ran {got_ran:?} (want {want_ran:?}), skipped {got_skipped:?} (want {want_skipped:?})",
            case.name
        ));
    }
    Ok(report)
}

/// Every `*.flow.json` directly under `dir`, as repo-relative paths.
fn flow_files(dir: &str) -> BTreeSet<String> {
    std::fs::read_dir(repo_root().join(dir))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".flow.json"))
        .map(|n| format!("{dir}/{n}"))
        .collect()
}

#[test]
fn every_committed_flow_runs_to_completion() {
    let on_disk: BTreeSet<String> = flow_files(GUIDES)
        .union(&flow_files(FIXTURES))
        .cloned()
        .collect();
    assert!(on_disk.len() >= 16, "found only {on_disk:?}");
    let covered: BTreeSet<String> = cases().iter().map(|c| c.file.to_string()).collect();
    assert_eq!(
        on_disk, covered,
        "a committed flow has no expected outcome, or a row names a flow that is gone"
    );
    let mut problems = Vec::new();
    for case in cases() {
        match load(case.file, case.subst).and_then(|t| run_text(&t, case.band)) {
            Ok(report) => {
                if let Err(e) = completed(&report) {
                    problems.push(format!("{}: {e}", case.name));
                }
                if report.records.is_empty() {
                    problems.push(format!("{}: nothing ran", case.name));
                }
            }
            Err(e) => problems.push(format!("{}: {e}", case.name)),
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn every_committed_flow_reaches_its_declared_branch() {
    let mut problems = Vec::new();
    for case in cases() {
        if let Err(e) = load(case.file, case.subst).and_then(|t| check_case(&case, &t)) {
            problems.push(e);
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A pair that must reach different branches: the same flow over two documents, or a flow and
/// its control. Each side names a file, how to point it, and the node that distinguishes it.
struct Pair {
    name: &'static str,
    a: (&'static str, &'static [(&'static str, &'static str)]),
    b: (&'static str, &'static [(&'static str, &'static str)]),
}

fn pairs() -> Vec<Pair> {
    let g = |f: &'static str| -> &'static str { f };
    vec![
        Pair {
            name: "wc-classify-measured, short vs long note",
            a: (g("_docs/02-guides/wc-classify-measured.flow.json"), &[]),
            b: (
                g("_docs/02-guides/wc-classify-measured.flow.json"),
                &[(WC_FILE_ENV, LONG)],
            ),
        },
        Pair {
            name: "wc-classify-check, short vs long note",
            a: (g("_docs/02-guides/wc-classify-check.flow.json"), &[]),
            b: (
                g("_docs/02-guides/wc-classify-check.flow.json"),
                &[(WC_FILE_ENV, LONG)],
            ),
        },
        Pair {
            name: "wc-classify vs its control",
            a: (g("_docs/02-guides/wc-classify.flow.json"), &[]),
            b: (g("_docs/02-guides/wc-classify.control.flow.json"), &[]),
        },
        Pair {
            name: "wc-classify-chained vs its control",
            a: (g("_docs/02-guides/wc-classify-chained.flow.json"), &[]),
            b: (
                g("_docs/02-guides/wc-classify-chained.control.flow.json"),
                &[],
            ),
        },
        Pair {
            name: "guess-the-number vs its control",
            a: (g("_docs/02-guides/guess-the-number.flow.json"), &[]),
            b: (g("_docs/02-guides/guess-the-number.control.flow.json"), &[]),
        },
    ]
}

/// R3: the two sides ran, completed, and reached different sets of nodes.
fn diverge(a_text: &str, b_text: &str) -> Result<(), String> {
    let a = run_text(a_text, "high")?;
    let b = run_text(b_text, "high")?;
    completed(&a)?;
    completed(&b)?;
    if ran(&a) == ran(&b) {
        return Err(format!("both reached {:?}", ran(&a)));
    }
    Ok(())
}

#[test]
fn the_count_driven_flows_diverge_on_the_two_documents() {
    let mut problems = Vec::new();
    for p in pairs() {
        let result = load(p.a.0, p.a.1)
            .and_then(|a| load(p.b.0, p.b.1).map(|b| (a, b)))
            .and_then(|(a, b)| diverge(&a, &b));
        if let Err(e) = result {
            problems.push(format!("{}: {e}", p.name));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// CONTROL for R2: a flow whose port points at the wrong node must be REFUSED by the table check.
#[test]
fn the_control_a_flow_with_a_wrong_port_is_detected() {
    let all = cases();
    let case = all
        .iter()
        .find(|c| c.name == "wc-classify-measured: short note -> class_short")
        .unwrap();
    let text = load(case.file, &[]).unwrap();
    assert!(
        check_case(case, &text).is_ok(),
        "the unbroken flow must pass"
    );
    let broken = load(
        case.file,
        &[(
            "\"ports\": { \"yes\": \"class_long\", \"no\": \"class_short\" }",
            "\"ports\": { \"yes\": \"class_long\", \"no\": \"class_long\" }",
        )],
    )
    .unwrap();
    let err = check_case(case, &broken).expect_err("a mis-pointed port must be caught");
    assert!(err.contains("class_long"), "{err}");
}

/// CONTROL for R3: the same flow with the count inlined into its prompt, run over both
/// documents, must be reported as NOT diverging — while the real flow does.
#[test]
fn the_control_a_hardcoded_prompt_is_detected() {
    let file = "_docs/02-guides/wc-classify-measured.flow.json";
    let short = load(file, &[]).unwrap();
    let long = load(file, &[(WC_FILE_ENV, LONG)]).unwrap();
    assert!(diverge(&short, &long).is_ok(), "the real flow must diverge");
    let inline = [("{{count_words}}", "13")];
    let hard_short = load(file, &inline).unwrap();
    let hard_long = load(file, &[(WC_FILE_ENV, LONG), inline[0]]).unwrap();
    let err = diverge(&hard_short, &hard_long).expect_err("a hardcoded prompt must be caught");
    assert!(err.contains("both reached"), "{err}");
    // the inputs really were the two documents
    assert!(Path::new(&repo_root().join(SHORT)).exists() && repo_root().join(LONG).exists());
}
