//! Thread a `decide` node's real payload into its `route` node, then hand the
//! rest of the graph to [`crate::sched::run`].
//!
//! This is the piece [`crate::node`]'s own docs name as deliberately out of
//! scope for part 3: *"Threading a payload through it is the CLI's concern (a
//! later part)."* [`crate::sched::run`] cannot do this itself: its dependency
//! tables are built once from the WHOLE edge set, so finishing a `route` node
//! unlocks every edge leaving it — all of a `decide`'s branches, not only the
//! one the answer selected. That is correct for [`crate::sched::run`]'s own
//! job (ordinary DAG fan-out has no conditional edges to begin with) and
//! wrong for a graph containing `route` nodes, which is exactly why this
//! lives in a NEW module rather than as an edit to `sched.rs` (frozen: parts
//! 1-3 are committed and consumed, not revised).
//!
//! The approach: resolve every `decide`-lowered `mcp` + `route` pair
//! sequentially, in the order encountered, calling the real
//! [`crate::node::McpCaller`] and pruning the branches the answer did not
//! select out of the live graph *before* anything downstream is scheduled.
//! Once no `route` node remains, the (possibly pruned) remainder is handed to
//! [`crate::sched::run`] unchanged — so a flow with no `decide` at all
//! degrades to exactly that call, and parts 1-3's own tested guarantees
//! (R8-R11) still apply to everything after the last routing decision.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use crate::error::RunError;
use crate::graph::{Edge, Graph, GraphNode, NodeBody};
use crate::node::execute_mcp;
use crate::run::{NodeRecord, RunReport};
use crate::sched::{self, RunOptions};

/// Run `graph` to completion, resolving every `route` node against the real
/// answer its upstream `mcp` node (a lowered `decide`) returns, before
/// scheduling anything downstream of it.
///
/// This is the entry point both `laya graph run` and the acceptance test use
/// (G21): a flow reaches here only by going validate -> expand -> lower, so a
/// route decision made here is always driven by a graph an author actually
/// drew, never a hand-built shortcut.
///
/// # Errors
/// [`RunError::BadLimit`] if `opts.limit` is zero (checked up front, same as
/// [`crate::sched::run`]). [`RunError::UnknownNode`] if a `route` node's
/// upstream is missing, is not an `mcp` node, or its chosen port names a node
/// the graph does not declare — all defensive: [`crate::lower::lower`] cannot
/// produce such a graph from a [`crate::validate::validate`]d flow.
/// [`RunError::NodeTaskFailed`] if the blocking call to `McpCaller` could not
/// be joined. A route value that cannot be read as the string `on` expects
/// (malformed or wrong-typed decide response) is DATA, not `Err`: the route
/// node's record is `success: false` and the run is `stopped_early` (R4).
pub async fn run_flow(graph: &Graph, opts: RunOptions) -> Result<RunReport, RunError> {
    if opts.limit == 0 {
        return Err(RunError::BadLimit { limit: opts.limit });
    }

    let mut nodes = graph.nodes.clone();
    let mut edges = graph.edges.clone();
    let mut records: Vec<NodeRecord> = Vec::new();

    while let Some((route_node, upstream_node)) = find_route_pair(&nodes, &edges)? {
        let NodeBody::Mcp { tool, arguments } = upstream_node.body.clone() else {
            // `lower_decide` always emits an `mcp` node immediately upstream
            // of its `route` node — a non-`mcp` upstream means this graph
            // was not produced by `lower`, which G21 forbids.
            return Err(RunError::UnknownNode {
                node_id: upstream_node.id.clone(),
            });
        };

        let upstream_record =
            call_upstream(&opts, upstream_node.id.clone(), tool, arguments).await?;
        let upstream_failed = !upstream_record.success;
        records.push(upstream_record.clone());

        remove_node(&mut nodes, &mut edges, &upstream_node.id);
        remove_node(&mut nodes, &mut edges, &route_node.id);

        if upstream_failed && !upstream_node.allow_failure {
            return Ok(stopped_report(opts, upstream_node.id, records, &nodes));
        }

        let (route_record, unselected) =
            resolve_route(&opts.run_id, &route_node, &upstream_record.output)?;
        if !route_record.success {
            // A wrong-typed route value is an upstream fault, reported as
            // data exactly like a failing upstream call (R4).
            let failed = route_record.node_id.clone();
            records.push(route_record);
            return Ok(stopped_report(opts, failed, records, &nodes));
        }
        records.push(route_record);
        prune_unreached(&mut nodes, &mut edges, unselected);
    }

    let pruned = Graph { nodes, edges };
    let tail_opts = RunOptions {
        run_id: opts.run_id.clone(),
        limit: opts.limit,
        mcp: Arc::clone(&opts.mcp),
    };
    let mut tail = sched::run(&pruned, tail_opts).await?;
    records.append(&mut tail.records);

    Ok(RunReport {
        run_id: opts.run_id,
        stopped_early: tail.stopped_early,
        failed_node: tail.failed_node,
        worker_limit: tail.worker_limit,
        records,
        not_started: tail.not_started,
    })
}

/// The report for a run stopped before the tail: every node still live in
/// `remaining` never started.
fn stopped_report(
    opts: RunOptions,
    failed_node: String,
    records: Vec<NodeRecord>,
    remaining: &[GraphNode],
) -> RunReport {
    let mut not_started: Vec<String> = remaining.iter().map(|n| n.id.clone()).collect();
    not_started.sort();
    RunReport {
        run_id: opts.run_id,
        stopped_early: true,
        failed_node: Some(failed_node),
        worker_limit: opts.limit,
        records,
        not_started,
    }
}

/// Find a `route` node still live in `nodes`, together with its upstream
/// node (the edge whose `to` names it) — `find`, never a position + index,
/// so a malformed graph reports [`RunError::UnknownNode`] instead of
/// panicking.
fn find_route_pair(
    nodes: &[GraphNode],
    edges: &[Edge],
) -> Result<Option<(GraphNode, GraphNode)>, RunError> {
    let Some(route_node) = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::Route { .. }))
        .cloned()
    else {
        return Ok(None);
    };
    let Some(upstream_id) = edges
        .iter()
        .find(|e| e.to == route_node.id)
        .map(|e| e.from.clone())
    else {
        return Err(RunError::UnknownNode {
            node_id: route_node.id,
        });
    };
    let Some(upstream_node) = nodes.iter().find(|n| n.id == upstream_id).cloned() else {
        return Err(RunError::UnknownNode {
            node_id: upstream_id,
        });
    };
    Ok(Some((route_node, upstream_node)))
}

/// Call the real `mcp` node feeding a `route` node, off the async executor
/// (the same `spawn_blocking` pattern [`crate::sched`] uses for every `mcp`
/// node), and wrap the result as its [`NodeRecord`].
async fn call_upstream(
    opts: &RunOptions,
    node_id: String,
    tool: String,
    arguments: Value,
) -> Result<NodeRecord, RunError> {
    let caller = Arc::clone(&opts.mcp);
    let executed =
        tokio::task::spawn_blocking(move || execute_mcp(caller.as_ref(), &tool, &arguments))
            .await
            .map_err(|e| RunError::NodeTaskFailed {
                reason: e.to_string(),
            })?;
    Ok(NodeRecord {
        run_id: opts.run_id.clone(),
        node_id,
        success: executed.success,
        exit_code: executed.exit_code,
        output: executed.output,
        error: executed.error,
        duration_ms: executed.duration_ms,
    })
}

/// Match `route_node`'s `on` expression against `upstream_output`, returning
/// the route node's own [`NodeRecord`] (carrying the matched value and the
/// chosen port — the evidence R-ACC requires) plus every OTHER candidate
/// port this decision did not select, for the caller to prune.
fn resolve_route(
    run_id: &str,
    route_node: &GraphNode,
    upstream_output: &str,
) -> Result<(NodeRecord, BTreeSet<String>), RunError> {
    let NodeBody::Route { on, rules } = route_node.body.clone() else {
        return Err(RunError::UnknownNode {
            node_id: route_node.id.clone(),
        });
    };
    let value = match extract_on_value(upstream_output, &on) {
        Ok(value) => value,
        Err(reason) => {
            let record = NodeRecord {
                run_id: run_id.to_string(),
                node_id: route_node.id.clone(),
                success: false,
                exit_code: None,
                output: String::new(),
                error: Some(format!("route `{}`: {reason}", route_node.id)),
                duration_ms: 0,
            };
            return Ok((record, BTreeSet::new()));
        }
    };
    let body = NodeBody::Route {
        on: on.clone(),
        rules: rules.clone(),
    };
    let Some(target) = body.route_port(&value) else {
        return Err(RunError::UnknownNode {
            node_id: route_node.id.clone(),
        });
    };
    let target = target.to_string();

    let record = NodeRecord {
        run_id: run_id.to_string(),
        node_id: route_node.id.clone(),
        success: true,
        exit_code: None,
        output: serde_json::json!({
            "on": on,
            "value": value,
            "selected_port": target,
        })
        .to_string(),
        error: None,
        duration_ms: 0,
    };

    let mut unselected: BTreeSet<String> = rules.iter().map(|r| r.port.clone()).collect();
    unselected.remove(&target);
    Ok((record, unselected))
}

/// Remove `id` and every edge touching it.
fn remove_node(nodes: &mut Vec<GraphNode>, edges: &mut Vec<Edge>, id: &str) {
    nodes.retain(|n| n.id != id);
    edges.retain(|e| e.from != id && e.to != id);
}

/// Remove every node in `targets`, and cascade: a successor left with no
/// remaining inbound edge was reachable only through a pruned branch, so it
/// is pruned too. A successor some OTHER surviving path still reaches is
/// left alone — pruning one branch of a `route` must never remove a node a
/// different part of the graph still depends on.
fn prune_unreached(
    nodes: &mut Vec<GraphNode>,
    edges: &mut Vec<Edge>,
    mut targets: BTreeSet<String>,
) {
    while let Some(id) = targets.iter().next().cloned() {
        targets.remove(&id);
        if !nodes.iter().any(|n| n.id == id) {
            continue;
        }
        let successors: Vec<String> = edges
            .iter()
            .filter(|e| e.from == id)
            .map(|e| e.to.clone())
            .collect();
        remove_node(nodes, edges, &id);
        for succ in successors {
            if !edges.iter().any(|e| e.to == succ) {
                targets.insert(succ);
            }
        }
    }
}

/// Read `$.payload.<field>` out of an `mcp` node's raw output text.
///
/// The output is JSON shaped `{"payload": {...}}` — the [`crate::node::McpCaller`]
/// implementation is the boundary that puts it in this shape (the real
/// `laya-mcp` `decide` tool returns `payloads` keyed by question id; a caller
/// reshapes the one this flow asked for into the singular `payload` object
/// this grammar expects). `lower_decide` only ever sets `on` to `label` or
/// `band`, both `String` fields on `DecisionPayload`, so reading the result
/// as a JSON string is always correct for a graph `lower` actually produced.
fn extract_on_value(output: &str, on: &str) -> Result<String, String> {
    let field = on.strip_prefix("$.payload.").unwrap_or(on);
    let parsed: Value =
        serde_json::from_str(output).map_err(|e| format!("output is not JSON: {e}: {output}"))?;
    let found = parsed.get("payload").and_then(|p| p.get(field));
    match found {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(format!(
            "`payload.{field}` is {}, expected a string, in: {output}",
            json_type(other)
        )),
    }
}

/// The JSON type of `value`, `missing` when absent — named in a route error.
const fn json_type(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::flow::{Flow, Kind, Node};
    use crate::lower::lower;
    use crate::node::McpCaller;

    /// Every test here runs through this: a hang becomes a failure the test
    /// reports itself (R5). The run goes on its own thread under a watchdog,
    /// because `tokio::time::timeout` cannot interrupt a synchronous loop that
    /// never yields.
    async fn bounded(graph: &Graph, opts: RunOptions) -> Result<RunReport, RunError> {
        let graph = graph.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .map(|rt| {
                    rt.block_on(async {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(4),
                            run_flow(&graph, opts),
                        )
                        .await
                    })
                });
            let _ = tx.send(result);
        });
        tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(4)))
            .await
            .expect("watchdog task")
            .expect("the run hung past 4s")
            .expect("runtime")
            .expect("the run hung past 4s")
    }

    struct NoLabel;
    impl McpCaller for NoLabel {
        fn call(&self, _t: &str, _a: &Value) -> Result<String, String> {
            Ok(r#"{"payload":{}}"#.to_string())
        }
    }

    /// Answers `decide` with a fixed JSON `payload.label` of any type.
    struct RawLabelCaller {
        label: Value,
    }
    impl McpCaller for RawLabelCaller {
        fn call(&self, _tool: &str, _arguments: &Value) -> Result<String, String> {
            Ok(serde_json::json!({"payload": {"label": self.label}}).to_string())
        }
    }

    /// Answers `decide` with a fixed label, regardless of arguments — a
    /// controllable stand-in for the real model, used only to prove THIS
    /// module's pruning and reporting, never for R-ACC itself (that needs
    /// the real `laya-mcp`, see `tests/acceptance.rs`).
    struct FixedLabelCaller {
        label: &'static str,
    }
    impl McpCaller for FixedLabelCaller {
        fn call(&self, tool: &str, _arguments: &Value) -> Result<String, String> {
            assert_eq!(tool, "decide", "this test flow has only one mcp node");
            Ok(serde_json::json!({
                "payload": {"label": self.label, "band": "high"}
            })
            .to_string())
        }
    }

    struct FailingCaller;
    impl McpCaller for FailingCaller {
        fn call(&self, _tool: &str, _arguments: &Value) -> Result<String, String> {
            Err("sidecar down".to_string())
        }
    }

    fn decide_flow() -> Flow {
        let classify = Node {
            question: Some("which?".to_string()),
            options: Some(vec!["a".to_string(), "b".to_string()]),
            prompt: Some("state text".to_string()),
            ports: Some(
                [("a", "handle_a"), ("b", "handle_b"), ("_", "human")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..Node::new("classify", Kind::Decide)
        };
        let handle_a = Node {
            run: Some("echo a".to_string()),
            ..Node::new("handle_a", Kind::Shell)
        };
        let handle_b = Node {
            run: Some("echo b".to_string()),
            ..Node::new("handle_b", Kind::Shell)
        };
        let human = Node {
            run: Some("echo human".to_string()),
            ..Node::new("human", Kind::Shell)
        };
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            nodes: vec![classify, handle_a, handle_b, human],
        }
    }

    /// THE SELECTIVITY GUARANTEE this module exists for: of the three
    /// branches a `decide` node can reach, only the one the answer selects
    /// actually runs. Without this, `sched::run`'s own dependency tables
    /// would unlock EVERY branch once the route node finished (see this
    /// module's docs), and a hardcoded route would pass just as well as a
    /// real one.
    #[tokio::test]
    async fn only_the_selected_branch_runs() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-a".to_string(),
                limit: 4,
                mcp: Arc::new(FixedLabelCaller { label: "a" }),
            },
        )
        .await
        .expect("must run");

        assert!(!report.stopped_early);
        let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        assert!(ran.contains(&"handle_a"), "got {ran:?}");
        assert!(!ran.contains(&"handle_b"), "got {ran:?}");
        assert!(!ran.contains(&"human"), "got {ran:?}");
    }

    /// The pairing control: answering `b` instead of `a` must reach the
    /// OTHER branch, not the same one — proof the selection is driven by the
    /// answer, not a fixed path (mirrors R-ACC's own control, against a
    /// controllable caller here rather than the live model).
    #[tokio::test]
    async fn the_control_a_different_answer_reaches_the_other_branch() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-b".to_string(),
                limit: 4,
                mcp: Arc::new(FixedLabelCaller { label: "b" }),
            },
        )
        .await
        .expect("must run");

        let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        assert!(ran.contains(&"handle_b"), "got {ran:?}");
        assert!(!ran.contains(&"handle_a"), "got {ran:?}");
    }

    /// An answer matching none of the declared ports must take the fallback
    /// — same guarantee `lower`'s own control pins, exercised here through
    /// actual execution rather than only `route_port`.
    #[tokio::test]
    async fn an_unmatched_answer_takes_the_fallback() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-fallback".to_string(),
                limit: 4,
                mcp: Arc::new(FixedLabelCaller {
                    label: "not-a-real-option",
                }),
            },
        )
        .await
        .expect("must run");

        let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        assert!(ran.contains(&"human"), "got {ran:?}");
        assert!(!ran.contains(&"handle_a"), "got {ran:?}");
        assert!(!ran.contains(&"handle_b"), "got {ran:?}");
    }

    /// The route node's own record carries the evidence R-ACC requires:
    /// the `label`/`band` value it matched on, not just which branch ran.
    #[tokio::test]
    async fn the_route_record_carries_the_matched_value() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-evidence".to_string(),
                limit: 4,
                mcp: Arc::new(FixedLabelCaller { label: "a" }),
            },
        )
        .await
        .expect("must run");

        let route_record = report
            .records
            .iter()
            .find(|r| r.node_id == "classify/route")
            .expect("the route node must have its own record");
        assert!(
            route_record.output.contains("\"value\":\"a\""),
            "{}",
            route_record.output
        );
        assert!(
            route_record
                .output
                .contains("\"selected_port\":\"handle_a\""),
            "{}",
            route_record.output
        );
    }

    /// R20 — A FAILING decide call must stop the run honestly: `stopped_early:
    /// true`, the failing node named, and nothing downstream (no branch, no
    /// route record) ever runs. This is the falsification target this part's
    /// report names: "a stopped run reported as complete."
    #[tokio::test]
    async fn a_failing_decide_call_stops_the_run_rather_than_picking_a_branch() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-fail".to_string(),
                limit: 4,
                mcp: Arc::new(FailingCaller),
            },
        )
        .await
        .expect("run must not error — a tool failure is DATA, not Err (R12)");

        assert!(
            report.stopped_early,
            "a failed decide call must stop the run"
        );
        assert_eq!(report.failed_node.as_deref(), Some("classify"));
        let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        assert_eq!(
            ran,
            vec!["classify"],
            "nothing downstream may run: got {ran:?}"
        );
    }

    /// The control for the test above: nothing here is "always stop" —
    /// a successful decide call must still complete normally.
    #[tokio::test]
    async fn the_control_a_successful_decide_call_does_not_stop_the_run() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "sel-ok".to_string(),
                limit: 4,
                mcp: Arc::new(FixedLabelCaller { label: "a" }),
            },
        )
        .await
        .expect("must run");
        assert!(!report.stopped_early);
        assert!(report.failed_node.is_none());
    }

    /// A graph with no `route` node at all must degrade to exactly
    /// `sched::run`'s own behaviour — this module must not change anything
    /// about a flow that never uses `decide`.
    #[tokio::test]
    async fn a_route_free_graph_behaves_exactly_like_sched_run() {
        let flow = Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            nodes: vec![Node {
                run: Some("echo plain".to_string()),
                ..Node::new("only", Kind::Shell)
            }],
        };
        let graph = lower(&flow).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "no-route".to_string(),
                limit: 4,
                mcp: Arc::new(FailingCaller), // must never be called
            },
        )
        .await
        .expect("must run");
        assert!(!report.stopped_early);
        assert_eq!(report.records.len(), 1);
        let only = report.records.first().expect("exactly one record");
        assert_eq!(only.node_id, "only");
    }

    #[tokio::test]
    async fn a_zero_limit_is_refused() {
        let graph = lower(&decide_flow()).expect("must lower");
        let result = bounded(
            &graph,
            RunOptions {
                run_id: "zero".to_string(),
                limit: 0,
                mcp: Arc::new(FailingCaller),
            },
        )
        .await;
        assert_eq!(result, Err(RunError::BadLimit { limit: 0 }));
    }

    #[tokio::test]
    async fn a_non_string_route_value_is_data_not_an_error() {
        let graph = lower(&decide_flow()).expect("must lower");
        for (label, ty) in [
            (serde_json::json!(3), "number"),
            (serde_json::json!(true), "bool"),
            (Value::Null, "null"),
            (serde_json::json!({"x": 1}), "object"),
            (serde_json::json!(["a"]), "array"),
        ] {
            let report = bounded(
                &graph,
                RunOptions {
                    run_id: "s9-route".to_string(),
                    limit: 4,
                    mcp: Arc::new(RawLabelCaller { label }),
                },
            )
            .await
            .expect("a wrong-typed route value is data, not Err");
            assert!(report.stopped_early, "{ty}");
            assert_eq!(report.failed_node.as_deref(), Some("classify/route"));
            let route = report
                .records
                .iter()
                .find(|r| r.node_id == "classify/route")
                .expect("the route record exists");
            assert!(!route.success);
            let error = route.error.as_deref().unwrap_or_default();
            assert!(
                error.contains("label") && error.contains(ty),
                "{ty}: {error}"
            );
            let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
            assert!(
                !ran.contains(&"handle_a") && !ran.contains(&"human"),
                "{ran:?}"
            );
            assert_eq!(
                report.records.len() + report.not_started.len(),
                report.records.len() + 3,
                "handle_a, handle_b and human never started"
            );
            assert_eq!(report.not_started, vec!["handle_a", "handle_b", "human"]);
        }
    }

    #[tokio::test]
    async fn a_missing_route_field_names_missing() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "s9-missing".to_string(),
                limit: 4,
                mcp: Arc::new(NoLabel),
            },
        )
        .await
        .expect("data, not Err");
        assert!(report.stopped_early);
        let error = report
            .records
            .last()
            .and_then(|r| r.error.clone())
            .unwrap_or_default();
        assert!(error.contains("missing"), "{error}");
    }

    #[tokio::test]
    async fn the_control_a_string_route_value_still_routes() {
        let graph = lower(&decide_flow()).expect("must lower");
        let report = bounded(
            &graph,
            RunOptions {
                run_id: "s9-route-ok".to_string(),
                limit: 4,
                mcp: Arc::new(RawLabelCaller {
                    label: serde_json::json!("a"),
                }),
            },
        )
        .await
        .expect("must run");
        assert!(!report.stopped_early);
        assert!(report.not_started.is_empty());
        let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        assert!(ran.contains(&"handle_a"), "{ran:?}");
    }
}
