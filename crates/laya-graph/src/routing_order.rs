//! Spec 17: which nodes run is a function of the graph, never of the array order of the
//! flow's `nodes`. Every test drives validate -> lower -> `run_flow` with a [`ScriptedCaller`]
//! that answers `decide` per node id, so no head is needed.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::node::{CallContext, McpCaller};
use crate::run::RunReport;
use crate::sched::RunOptions;
use crate::{expand, lower, run_flow, validate};

/// Answers `decide` with the label scripted for the calling node id.
struct ScriptedCaller(&'static [(&'static str, &'static str)]);

impl McpCaller for ScriptedCaller {
    fn call(&self, tool: &str, _a: &Value, ctx: &CallContext) -> Result<String, String> {
        assert_eq!(tool, "decide");
        let label = self
            .0
            .iter()
            .find(|(id, _)| *id == ctx.node_id)
            .map(|(_, l)| *l)
            .ok_or_else(|| format!("no scripted label for {}", ctx.node_id))?;
        Ok(json!({"payload": {"label": label, "band": "high"}}).to_string())
    }
}

fn run_text(text: &str, script: &'static [(&'static str, &'static str)]) -> RunReport {
    let raw: Value = serde_json::from_str(text).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    validate(&flow).unwrap();
    let graph = lower(&flow).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(run_flow(
        &graph,
        RunOptions {
            run_id: "order".to_string(),
            limit: 4,
            mcp: Arc::new(ScriptedCaller(script)),
        },
    ))
    .unwrap()
}

fn ran(report: &RunReport) -> BTreeSet<String> {
    report.records.iter().map(|r| r.node_id.clone()).collect()
}

const P6: &str = include_str!("../tests/fixtures/p6-file-order.flow.json");
const P6C: &str = include_str!("../tests/fixtures/p6c-file-order-control.flow.json");
const P7: &str = include_str!("../tests/fixtures/p7-join.flow.json");
const P13: &str = include_str!("../tests/fixtures/p13-shared-target.flow.json");
/// Not a fixture file: the fixtures dir is scanned by "every committed flow validates", and this
/// flow is refused by design.
const P12: &str = r#"{"flow":"p12","version":1,"nodes":[
 {"id":"d","kind":"decide","prompt":"The word count in the file is 13 words.","question":"Is the word count greater than 300 words?","options":["yes","no"],"escalate_on_near_tie":true,"near_tie_port":"human","ports":{"yes":"a","no":"b"}},
 {"id":"a","kind":"shell","run":"echo a"},{"id":"b","kind":"shell","run":"echo b"},{"id":"human","kind":"shell","run":"echo human"}
]}"#;

/// `outer_A` answers `no`, so `inner_B` (behind A's `yes` port) must never be reached.
const P6_SCRIPT: &[(&str, &str)] = &[("outer_A", "no"), ("inner_B", "yes")];

#[test]
fn the_files_node_order_does_not_change_which_nodes_run() {
    let a = run_text(P6, P6_SCRIPT);
    let b = run_text(P6C, P6_SCRIPT);
    assert_eq!(ran(&a), ran(&b), "array order changed what ran");
    assert!(!ran(&a).is_empty());
}

#[test]
fn the_control_the_array_order_is_the_only_difference() {
    let strip = |t: &str| {
        let mut v: Value = serde_json::from_str(t).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("flow");
        obj.remove("description");
        let nodes = obj.get_mut("nodes").unwrap().as_array_mut().unwrap();
        nodes.sort_by_key(|n| n["id"].as_str().unwrap().to_string());
        v
    };
    assert_eq!(strip(P6), strip(P6C), "fixtures differ by more than order");
    let ids = |t: &str| -> Vec<String> {
        let v: Value = serde_json::from_str(t).unwrap();
        v.get("nodes")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_ne!(ids(P6), ids(P6C), "the order must actually differ");
}

#[test]
fn a_branch_the_graph_did_not_select_does_not_run() {
    for text in [P6, P6C] {
        let report = run_text(text, P6_SCRIPT);
        let ran = ran(&report);
        assert!(!ran.contains("B_yes"), "{ran:?}");
        assert!(!ran.contains("B_no"), "{ran:?}");
        assert!(!ran.contains("inner_B"), "{ran:?}");
        assert!(ran.contains("A_no"), "{ran:?}");
        assert_eq!(
            report.skipped,
            vec!["B_no", "B_yes", "inner_B", "inner_B/route"]
        );
        assert!(report.not_started.is_empty());
    }
}

#[test]
fn a_node_whose_dependency_was_pruned_is_skipped_and_named() {
    let report = run_text(P7, &[("d", "no")]);
    let ran = ran(&report);
    assert!(!ran.contains("needs_long"), "{ran:?}");
    assert!(!ran.contains("long_path"), "{ran:?}");
    assert!(
        ran.contains("short_path") && ran.contains("prep"),
        "{ran:?}"
    );
    assert_eq!(report.skipped, vec!["long_path", "needs_long"]);
    assert!(report.not_started.is_empty());
}

#[test]
fn two_routes_sharing_a_target_both_reach_it() {
    let report = run_text(P13, &[("d1", "no"), ("d2", "yes")]);
    let ran = ran(&report);
    assert!(ran.contains("handle_yes"), "{ran:?}");
    assert!(ran.contains("handle_no"), "{ran:?}");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn the_control_a_branch_that_is_selected_still_runs() {
    // p6 with outer_A answering `yes`: the port is selected, so inner_B (a mixed node: port
    // edge AND depends_on outer_A) and its own selected branch must run; A_no must not.
    for text in [P6, P6C] {
        let report = run_text(text, &[("outer_A", "yes"), ("inner_B", "yes")]);
        let ran = ran(&report);
        assert!(ran.contains("inner_B"), "{ran:?}");
        assert!(ran.contains("B_yes"), "{ran:?}");
        assert!(!ran.contains("A_no"), "{ran:?}");
        assert!(!ran.contains("B_no"), "{ran:?}");
        assert_eq!(report.skipped, vec!["A_no", "B_no"]);
        assert!(!report.stopped_early);
    }
}

#[test]
fn a_near_tie_target_does_not_run_without_a_near_tie() {
    let raw: Value = serde_json::from_str(P12).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    let err = validate(&flow).expect_err("near-tie routing is refused until implemented");
    assert!(err.to_string().contains("not implemented"), "{err}");
}

#[test]
fn an_old_report_without_skipped_still_deserialises() {
    let old =
        r#"{"run_id":"r","stopped_early":false,"failed_node":null,"worker_limit":1,"records":[]}"#;
    let report: RunReport = serde_json::from_str(old).unwrap();
    assert!(report.skipped.is_empty());
}

/// `x` is a MIXED node: a port edge from `d1` AND `depends_on` `g`, which `d2` may or may not
/// select. `z` depends on `x`, so a skip must propagate.
const MIXED: &str = r#"{"flow":"mixed","version":1,"nodes":[
 {"id":"d1","kind":"decide","prompt":"p","question":"q","options":["yes","no"],"ports":{"yes":"x","no":"y"}},
 {"id":"d2","kind":"decide","prompt":"p","question":"q","options":["yes","no"],"ports":{"yes":"g","no":"h"}},
 {"id":"g","kind":"shell","run":"echo g"},
 {"id":"h","kind":"shell","run":"echo h"},
 {"id":"x","kind":"shell","depends_on":["g"],"run":"echo x"},
 {"id":"y","kind":"shell","run":"echo y"},
 {"id":"z","kind":"shell","depends_on":["x"],"run":"echo z"}
]}"#;

#[test]
fn a_mixed_node_with_a_selected_port_but_a_skipped_dependency_is_skipped_transitively() {
    let report = run_text(MIXED, &[("d1", "yes"), ("d2", "no")]);
    let ran = ran(&report);
    assert!(ran.contains("h") && !ran.contains("g"), "{ran:?}");
    assert!(!ran.contains("x"), "port selected but g skipped: {ran:?}");
    assert!(!ran.contains("z"), "z depends on the skipped x: {ran:?}");
    assert_eq!(report.skipped, vec!["g", "x", "y", "z"]);
}

#[test]
fn the_control_a_mixed_node_with_both_conditions_met_runs_and_so_does_its_dependent() {
    let report = run_text(MIXED, &[("d1", "yes"), ("d2", "yes")]);
    let ran = ran(&report);
    for id in ["g", "x", "z"] {
        assert!(ran.contains(id), "{id} must run: {ran:?}");
    }
    assert!(!ran.contains("y") && !ran.contains("h"), "{ran:?}");
    assert_eq!(report.skipped, vec!["h", "y"]);
}

#[test]
fn a_mixed_node_whose_dependency_ran_but_whose_port_was_not_selected_is_skipped() {
    // The other half of the AND: g runs, but d1 selected `y`, so x has no selected port edge.
    let report = run_text(MIXED, &[("d1", "no"), ("d2", "yes")]);
    let ran = ran(&report);
    assert!(ran.contains("g") && ran.contains("y"), "{ran:?}");
    assert!(!ran.contains("x") && !ran.contains("z"), "{ran:?}");
    assert_eq!(report.skipped, vec!["h", "x", "z"]);
}
