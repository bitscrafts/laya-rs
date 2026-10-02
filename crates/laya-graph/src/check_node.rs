//! Spec 25: a `check` node — the exit status of a command selects the port, with no model in the
//! loop. Every test drives expand -> validate -> lower -> `run_flow` with a caller that PANICS if
//! it is ever asked anything, so "no model was called" is asserted by the run completing.

#![cfg(test)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use serde_json::{Value, json};

use crate::node::{CallContext, McpCaller};
use crate::run::RunReport;
use crate::sched::RunOptions;
use crate::{FlowError, expand, lower, run_flow, validate};

/// A model that must never be consulted: any call is a test failure.
struct NoModel;

impl McpCaller for NoModel {
    fn call(&self, tool: &str, _args: &Value, _ctx: &CallContext) -> Result<String, String> {
        panic!("a check flow called the model: `{tool}`");
    }
}

fn check_flow(run: &str, extra: &Value) -> String {
    let mut check = json!({"id":"c","kind":"check","run":run,
        "ports":{"pass":"yes_node","fail":"no_node","_":"odd_node"}});
    if let (Some(obj), Some(more)) = (check.as_object_mut(), extra.as_object()) {
        obj.extend(more.clone());
    }
    json!({"flow":"t","version":1,"nodes":[
        check,
        {"id":"yes_node","kind":"shell","run":"echo yes"},
        {"id":"no_node","kind":"shell","run":"echo no"},
        {"id":"odd_node","kind":"shell","run":"echo odd"}
    ]})
    .to_string()
}

fn try_run(text: &str) -> Result<RunReport, FlowError> {
    let raw: Value = serde_json::from_str(text).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    validate(&flow)?;
    let graph = lower(&flow).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    Ok(rt
        .block_on(run_flow(
            &graph,
            RunOptions {
                run_id: "ck".to_string(),
                limit: 4,
                mcp: Arc::new(NoModel),
            },
        ))
        .unwrap())
}

fn run_text(text: &str) -> RunReport {
    try_run(text).unwrap()
}

fn ran(report: &RunReport, id: &str) -> bool {
    report.records.iter().any(|r| r.node_id == id)
}

fn skipped(report: &RunReport, id: &str) -> bool {
    report.skipped.iter().any(|s| s == id)
}

#[test]
fn a_check_takes_the_pass_port_on_exit_zero() {
    let report = run_text(&check_flow("exit 0", &json!({})));
    assert!(ran(&report, "yes_node"), "{report:?}");
    assert!(skipped(&report, "no_node") && skipped(&report, "odd_node"));
    assert!(!ran(&report, "no_node") && !ran(&report, "odd_node"));
    assert!(!report.stopped_early);
}

#[test]
fn a_check_takes_the_fail_port_on_nonzero_exit() {
    let report = run_text(&check_flow("exit 1", &json!({})));
    assert!(ran(&report, "no_node"), "{report:?}");
    assert!(skipped(&report, "yes_node") && skipped(&report, "odd_node"));
    assert!(!report.stopped_early, "a non-zero exit is the answer");
    assert!(report.not_started.is_empty(), "{report:?}");
}

/// The control: a non-zero exit is data. If the check failed the node, `stopped_early` would be
/// true and the `fail` target would never start.
#[test]
fn the_control_a_failing_check_does_not_fail_the_run() {
    let report = run_text(&check_flow("exit 7", &json!({})));
    assert!(!report.stopped_early, "{report:?}");
    assert_eq!(report.failed_node, None);
    let record = report.records.iter().find(|r| r.node_id == "c").unwrap();
    assert!(
        record.success,
        "the check node itself succeeded: {record:?}"
    );
    assert_eq!(record.exit_code, Some(7), "the exit status is kept as data");
    assert!(ran(&report, "no_node"), "the fail target ran");
}

#[test]
fn an_absent_pass_or_fail_key_takes_the_catch_all() {
    let text = json!({"flow":"t","version":1,"nodes":[
        {"id":"c","kind":"check","run":"exit 3","ports":{"pass":"p","_":"other"}},
        {"id":"p","kind":"shell","run":"echo p"},
        {"id":"other","kind":"shell","run":"echo o"}
    ]})
    .to_string();
    let report = run_text(&text);
    assert!(ran(&report, "other") && skipped(&report, "p"), "{report:?}");
    assert!(!report.stopped_early);
}

#[test]
fn a_check_with_no_run_is_refused_by_name() {
    let text = json!({"flow":"t","version":1,"nodes":[
        {"id":"c","kind":"check","ports":{"_":"x"}},
        {"id":"x","kind":"shell","run":"true"}
    ]})
    .to_string();
    let err = try_run(&text).unwrap_err().to_string();
    assert!(err.contains("`c`") && err.contains("run"), "{err}");
}

#[test]
fn a_check_whose_command_cannot_be_executed_fails_the_node() {
    let report = run_text(&check_flow("/nonexistent/bin/x", &json!({})));
    let record = report.records.iter().find(|r| r.node_id == "c").unwrap();
    assert!(!record.success, "{record:?}");
    assert!(report.stopped_early, "{report:?}");
    assert_eq!(report.failed_node.as_deref(), Some("c"));
    assert!(!ran(&report, "no_node"), "must not silently take `fail`");
}

#[test]
fn a_check_needs_a_fallback_port() {
    let text = json!({"flow":"t","version":1,"nodes":[
        {"id":"c","kind":"check","run":"true","ports":{"pass":"x","fail":"x"}},
        {"id":"x","kind":"shell","run":"true"}
    ]})
    .to_string();
    let err = try_run(&text).unwrap_err().to_string();
    assert!(err.contains("`c`") && err.contains('_'), "{err}");
}

#[test]
fn a_check_port_must_be_pass_fail_or_catch_all_and_name_a_node() {
    let bad_key = json!({"flow":"t","version":1,"nodes":[
        {"id":"c","kind":"check","run":"true","ports":{"yes":"x","_":"x"}},
        {"id":"x","kind":"shell","run":"true"}
    ]})
    .to_string();
    let err = try_run(&bad_key).unwrap_err().to_string();
    assert!(err.contains("yes"), "{err}");
    let bad_target = json!({"flow":"t","version":1,"nodes":[
        {"id":"c","kind":"check","run":"true","ports":{"_":"ghost"}}
    ]})
    .to_string();
    let err = try_run(&bad_target).unwrap_err().to_string();
    assert!(err.contains("ghost"), "{err}");
}

#[test]
fn allow_failure_on_a_check_is_refused() {
    let err = try_run(&check_flow("true", &json!({"allow_failure": true})))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`c`") && err.contains("allow_failure"),
        "{err}"
    );
}

#[test]
fn a_timed_out_check_fails_the_node_not_the_fail_port() {
    let report = run_text(&check_flow("sleep 5", &json!({"timeout": 0})));
    let record = report.records.iter().find(|r| r.node_id == "c").unwrap();
    assert!(!record.success && record.timed_out, "{record:?}");
    assert!(report.stopped_early);
    assert!(
        !ran(&report, "no_node"),
        "a timeout is not the `fail` answer"
    );
}

/// The panicking caller is the proof: a completed run took no model call.
#[test]
fn no_model_is_called_by_a_check() {
    for (run, expected) in [("exit 0", "yes_node"), ("exit 9", "no_node")] {
        let report = run_text(&check_flow(run, &json!({})));
        assert!(ran(&report, expected), "{run}: {report:?}");
        assert!(!report.stopped_early);
    }
}

#[test]
fn a_check_composes_with_depends_on_and_another_check() {
    let text = json!({"flow":"t","version":1,"nodes":[
        {"id":"count","kind":"shell","run":"echo 0"},
        {"id":"first","kind":"check","run":"exit 1","depends_on":["count"],
         "ports":{"pass":"done","fail":"second","_":"done"}},
        {"id":"second","kind":"check","run":"exit 0",
         "ports":{"pass":"release","fail":"work_left","_":"work_left"}},
        {"id":"done","kind":"shell","run":"echo done"},
        {"id":"release","kind":"shell","run":"echo release"},
        {"id":"work_left","kind":"shell","run":"echo left"}
    ]})
    .to_string();
    let report = run_text(&text);
    assert!(
        ran(&report, "second") && ran(&report, "release"),
        "{report:?}"
    );
    for id in ["done", "work_left"] {
        assert!(skipped(&report, id) && !ran(&report, id), "{id}");
    }
    assert!(report.not_started.is_empty() && !report.stopped_early);
}

/// The boundary a model gets wrong: exactly 300 vs 301.
#[test]
fn a_check_compares_exactly_at_the_boundary() {
    for (n, expected) in [(299, "yes_node"), (300, "yes_node"), (301, "no_node")] {
        let run = format!("[ {n} -le 300 ]");
        let report = run_text(&check_flow(&run, &json!({})));
        assert!(ran(&report, expected), "{n}: {report:?}");
    }
}
