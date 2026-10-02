//! Spec 18: `{{node_id}}` in a decide's `prompt` is the trimmed stdout of a `depends_on` node.
//! Every test drives validate -> lower -> `run_flow` with a caller that records the arguments it
//! was handed, so what the head WOULD have seen is asserted, not inferred.

#![cfg(test)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::node::{CallContext, McpCaller};
use crate::run::RunReport;
use crate::sched::RunOptions;
use crate::{expand, lower, run_flow, validate};

/// Records every `decide` argument object; answers `yes` iff the first integer in the prompt
/// is over 100, so the branch genuinely follows the substituted value.
struct Recording(Arc<Mutex<Vec<Value>>>);

impl McpCaller for Recording {
    fn call(&self, tool: &str, args: &Value, _ctx: &CallContext) -> Result<String, String> {
        assert_eq!(tool, "decide");
        self.0.lock().unwrap().push(args.clone());
        let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("");
        let n: u64 = prompt
            .split(|c: char| !c.is_ascii_digit())
            .find(|s| !s.is_empty())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let label = if n > 100 { "yes" } else { "no" };
        Ok(json!({"payload": {"label": label, "band": "high"}}).to_string())
    }
}

fn flow_text(shell_run: &str, allow_failure: bool, prompt: &str) -> String {
    json!({"flow":"t","version":1,"nodes":[
        {"id":"n","kind":"shell","run":shell_run,"allow_failure":allow_failure},
        {"id":"d","kind":"decide","depends_on":["n"],"prompt":prompt,
         "question":"Is it over 100?","options":["yes","no"],
         "ports":{"yes":"big","no":"small"}},
        {"id":"big","kind":"shell","run":"echo big"},
        {"id":"small","kind":"shell","run":"echo small"}
    ]})
    .to_string()
}

fn run_text(text: &str) -> (RunReport, Vec<Value>) {
    let raw: Value = serde_json::from_str(text).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    validate(&flow).unwrap();
    let graph = lower(&flow).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let report = rt
        .block_on(run_flow(
            &graph,
            RunOptions {
                run_id: "sv".to_string(),
                limit: 4,
                mcp: Arc::new(Recording(Arc::clone(&seen))),
            },
        ))
        .unwrap();
    let calls = seen.lock().unwrap().clone();
    (report, calls)
}

fn ran(report: &RunReport, id: &str) -> bool {
    report.records.iter().any(|r| r.node_id == id)
}

fn refusal(text: &str) -> String {
    let raw: Value = serde_json::from_str(text).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    validate(&flow).unwrap_err().to_string()
}

const PROMPT: &str = "The value is {{n}}. Is it over 100?";

#[test]
fn a_shell_output_reaches_the_decision_and_changes_the_branch() {
    let (report, calls) = run_text(&flow_text("echo 250", false, PROMPT));
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["prompt"], "The value is 250. Is it over 100?");
    assert!(ran(&report, "big"), "{:?}", report.records);
    assert!(!ran(&report, "small"));
}

#[test]
fn the_control_the_same_flow_with_the_other_value_takes_the_other_branch() {
    let (report, calls) = run_text(&flow_text("echo 7", false, PROMPT));
    assert_eq!(calls[0]["prompt"], "The value is 7. Is it over 100?");
    assert!(ran(&report, "small"), "{:?}", report.records);
    assert!(!ran(&report, "big"));
}

#[test]
fn the_substituted_value_is_the_trimmed_stdout_not_the_stderr() {
    let (_, calls) = run_text(&flow_text(
        "echo warn >&2; printf '  42 \\n\\n'",
        false,
        PROMPT,
    ));
    assert_eq!(calls[0]["prompt"], "The value is 42. Is it over 100?");
}

#[test]
fn an_unlisted_reference_is_refused_by_name() {
    let text = json!({"flow":"t","version":1,"nodes":[
        {"id":"n","kind":"shell","run":"echo 1"},
        {"id":"x","kind":"shell","run":"echo 2"},
        {"id":"d","kind":"decide","depends_on":["n"],"prompt":"v {{x}}",
         "question":"q?","options":["yes","no"],"ports":{"yes":"n","no":"n"}}
    ]})
    .to_string();
    let msg = refusal(&text);
    assert!(msg.contains("`x`") && msg.contains("`d`"), "{msg}");
    assert!(msg.contains("depends_on"), "{msg}");
}

#[test]
fn an_unknown_node_in_a_placeholder_is_refused_by_name() {
    let msg = refusal(&flow_text("echo 1", false, "v {{nosuch}}"));
    assert!(
        msg.contains("nosuch") && msg.contains("`d`") && msg.contains("not a node"),
        "{msg}"
    );
}

#[test]
fn a_decide_referencing_itself_is_refused_by_name() {
    let msg = refusal(&flow_text("echo 1", false, "v {{d}}"));
    assert!(msg.contains("`d`") && msg.contains("itself"), "{msg}");
}

#[test]
fn a_failed_referent_skips_the_decide_rather_than_asking_over_empty() {
    let (report, calls) = run_text(&flow_text("exit 1", true, PROMPT));
    assert!(calls.is_empty(), "the head must never be called: {calls:?}");
    assert!(
        report.skipped.contains(&"d".to_string()),
        "{:?}",
        report.skipped
    );
    assert!(!ran(&report, "d") && !ran(&report, "big") && !ran(&report, "small"));
    let why = report.skip_reasons.get("d").expect("a reason for d");
    assert!(why.contains("`n`") && why.contains("failed"), "{why}");
}

#[test]
fn a_referent_that_printed_nothing_skips_rather_than_substituting_empty() {
    let (report, calls) = run_text(&flow_text("true", false, PROMPT));
    assert!(calls.is_empty(), "{calls:?}");
    assert!(report.skip_reasons["d"].contains("printed nothing"));
}

#[test]
fn a_referent_over_the_state_cap_skips_and_one_at_the_cap_does_not() {
    use laya_core::agentic::STATE_CAP_BYTES;
    let at = format!("head -c {STATE_CAP_BYTES} /dev/zero | tr '\\0' 5");
    let (report, calls) = run_text(&flow_text(&at, false, "v {{n}}"));
    assert_eq!(calls.len(), 1, "at the cap must still be asked");
    assert!(report.skip_reasons.is_empty());
    let over = format!("head -c {} /dev/zero | tr '\\0' 5", STATE_CAP_BYTES + 1);
    let (report, calls) = run_text(&flow_text(&over, false, "v {{n}}"));
    assert!(calls.is_empty());
    assert!(
        report.skip_reasons["d"].contains("state cap"),
        "{:?}",
        report.skip_reasons
    );
}

#[test]
fn the_control_a_successful_referent_does_not_skip() {
    let (report, calls) = run_text(&flow_text("echo 250", true, PROMPT));
    assert_eq!(calls.len(), 1);
    assert!(
        !report.skipped.contains(&"d".to_string()),
        "{:?}",
        report.skipped
    );
    assert!(ran(&report, "d") && ran(&report, "big"));
}

#[test]
fn a_literal_prompt_without_placeholders_is_unchanged() {
    // What the head is handed must be exactly what `lower` compiled, byte for byte.
    let text = include_str!("../tests/fixtures/p6-file-order.flow.json");
    let raw: Value = serde_json::from_str(text).unwrap();
    let flow = expand(&raw, &crate::Library::new()).unwrap();
    let graph = lower(&flow).unwrap();
    let compiled: Vec<String> = graph
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            crate::NodeBody::Mcp { arguments, .. } => Some(arguments.to_string()),
            _ => None,
        })
        .collect();
    assert!(!compiled.is_empty());
    let (_, calls) = run_text(text);
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(
            compiled.contains(&call.to_string()),
            "{call} not in {compiled:?}"
        );
    }
}
