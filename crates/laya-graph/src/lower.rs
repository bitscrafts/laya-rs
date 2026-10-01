//! `decide` -> `mcp` + `route`, plus edges.
//!
//! A `decide` node becomes exactly two nodes and one edge per port:
//!
//! ```text
//! decide {id}   ->   {id}         kind=mcp   tool=decide
//!                     {id}/route  kind=route
//! edges:  {id} -> {id}/route
//!         {id}/route:<port> -> <target>   for every port
//! ```
//!
//! `lower` is a pure function: it never mutates its input, so
//! `lowering_leaves_the_input_flow_unchanged` can assert the input `Flow`
//! compares equal after the call.

use serde_json::json;

use crate::error::FlowError;
use crate::flow::{Flow, Kind, Node};
use crate::graph::{Edge, Graph, GraphNode, NodeBody, RouteRule};
use crate::on_expr;

/// Lower a validated flow into its execution form.
///
/// # Errors
///
/// Returns [`FlowError::BadOnExpr`] if a `decide` node's `on` field selects a
/// payload field the member-access grammar does not accept (only `label` and
/// `band` are legal values for `decide.on`, and both always produce a valid
/// `$.payload.<field>` expression — so this only fires if `lower` is called
/// on a flow `validate` was never run against). Returns
/// [`FlowError::UnknownKind`] for a `route` or unsupported node reaching this
/// stage, for the same reason: lowering assumes a validated flow.
pub fn lower(flow: &Flow) -> Result<Graph, FlowError> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for node in &flow.nodes {
        match &node.kind {
            Kind::Decide => lower_decide(node, &mut nodes, &mut edges)?,
            Kind::Shell => nodes.push(GraphNode {
                id: node.id.clone(),
                body: NodeBody::Shell {
                    run: node.run.clone().unwrap_or_default(),
                },
                writes: node.writes,
                allow_failure: node.allow_failure,
            }),
            Kind::Mcp => nodes.push(GraphNode {
                id: node.id.clone(),
                body: NodeBody::Mcp {
                    tool: node.tool.clone().unwrap_or_default(),
                    arguments: node.arguments.clone().unwrap_or_else(|| json!({})),
                },
                writes: node.writes,
                allow_failure: node.allow_failure,
            }),
            Kind::Route | Kind::Other(_) => {
                return Err(FlowError::UnknownKind {
                    id: node.id.clone(),
                    kind: node.kind.as_str().to_string(),
                });
            }
        }

        for dep in &node.depends_on {
            edges.push(Edge {
                from: dep.clone(),
                to: node.id.clone(),
                port: None,
            });
        }
    }

    Ok(Graph { nodes, edges })
}

fn lower_decide(
    node: &Node,
    nodes: &mut Vec<GraphNode>,
    edges: &mut Vec<Edge>,
) -> Result<(), FlowError> {
    let options = node.options.clone().unwrap_or_default();
    let question = node.question.clone().unwrap_or_default();

    nodes.push(GraphNode {
        id: node.id.clone(),
        body: NodeBody::Mcp {
            tool: "decide".to_string(),
            arguments: json!({
                "question": question,
                "options": options,
                "prompt": node.prompt,
            }),
        },
        writes: node.writes,
        allow_failure: node.allow_failure,
    });

    let field = match node.on.as_deref() {
        Some("band") => "band",
        _ => "label",
    };
    let on_expr = format!("$.payload.{field}");
    on_expr::parse(&on_expr)?;

    let route_id = format!("{}/route", node.id);
    let ports = node.ports.clone().unwrap_or_default();

    // THE FALLBACK RULE IS EMITTED LAST (R5). `ports` is a `BTreeMap`, so
    // iterating it directly would order rules lexicographically by key —
    // `_` (0x5F) sorts ahead of every lowercase option name, which would put
    // the fallback FIRST and, under first-match-wins, make it swallow every
    // answer. Real answers are collected first and the fallback is appended
    // afterward, regardless of where `_` falls in the map's own order.
    let mut rules: Vec<RouteRule> = ports
        .iter()
        .filter(|(key, _)| key.as_str() != "_")
        .map(|(key, target)| RouteRule {
            eq: Some(key.clone()),
            port: target.clone(),
        })
        .collect();
    if let Some(fallback_target) = ports.get("_") {
        rules.push(RouteRule {
            eq: None,
            port: fallback_target.clone(),
        });
    }

    nodes.push(GraphNode {
        id: route_id.clone(),
        body: NodeBody::Route {
            on: on_expr,
            rules: rules.clone(),
        },
        writes: false,
        allow_failure: false,
    });

    edges.push(Edge {
        from: node.id.clone(),
        to: route_id.clone(),
        port: None,
    });
    for rule in &rules {
        edges.push(Edge {
            from: route_id.clone(),
            to: rule.port.clone(),
            port: Some(rule.eq.clone().unwrap_or_else(|| "_".to_string())),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::flow::Node;

    fn decide_flow(ports: &[(&str, &str)]) -> Flow {
        let node = Node {
            question: Some("which?".to_string()),
            options: Some(vec!["a".to_string(), "b".to_string(), "c".to_string()]),
            ports: Some(
                ports
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ),
            ..Node::new("classify", Kind::Decide)
        };
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            nodes: vec![node],
        }
    }

    #[test]
    fn a_decide_lowers_to_two_nodes_and_its_edges() {
        let flow = decide_flow(&[("a", "handle_a"), ("b", "handle_b"), ("_", "human")]);
        let graph = lower(&flow).expect("a valid decide node must lower");

        let classify = graph.node("classify").expect("mcp node must exist");
        assert!(matches!(&classify.body, NodeBody::Mcp { tool, .. } if tool == "decide"));

        let route = graph.node("classify/route").expect("route node must exist");
        assert!(matches!(&route.body, NodeBody::Route { .. }));

        let expected: std::collections::BTreeSet<(String, String)> = [
            ("classify".to_string(), "classify/route".to_string()),
            ("classify/route".to_string(), "handle_a".to_string()),
            ("classify/route".to_string(), "handle_b".to_string()),
            ("classify/route".to_string(), "human".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(graph.edge_set(), expected, "exactly the 4 expected edges");
    }

    #[test]
    fn lowering_leaves_the_input_flow_unchanged() {
        let flow = decide_flow(&[("a", "handle_a"), ("b", "handle_b"), ("_", "human")]);
        let before = flow.clone();
        let _ = lower(&flow).expect("must lower");
        assert_eq!(flow, before, "lower must not mutate its input");
    }

    /// R5 — THE ACCEPTANCE TEST. 3 real ports + a fallback; the label
    /// matches the SECOND rule. If the fallback were first (or merely in
    /// `ports`' own iteration order, as the first version of `lower_decide`
    /// above deliberately was), first-match-wins means EVERY answer —
    /// including this one — takes the fallback. Only asserting the chosen
    /// port is `b`'s own target, not the fallback, catches that.
    #[test]
    fn the_fallback_is_last_and_a_real_answer_does_not_take_it() {
        let flow = decide_flow(&[("a", "handle_a"), ("b", "handle_b"), ("_", "human")]);
        let graph = lower(&flow).expect("must lower");
        let route = graph.node("classify/route").expect("route node must exist");

        let NodeBody::Route { rules, .. } = &route.body else {
            panic!("expected a Route body");
        };
        assert!(
            rules.last().is_some_and(|r| r.eq.is_none()),
            "the fallback rule must be LAST, got {rules:?}"
        );

        let chosen = route.body.route_port("b");
        assert_eq!(
            chosen,
            Some("handle_b"),
            "label `b` must reach its OWN port, not the fallback `human` — got {chosen:?}"
        );
    }

    /// R5's control: the fallback must still be reachable by something that
    /// genuinely matches nothing else, or "the fallback is last" could have
    /// been satisfied by removing it instead of ordering it correctly.
    #[test]
    fn the_control_a_genuinely_unmatched_answer_takes_the_fallback() {
        let flow = decide_flow(&[("a", "handle_a"), ("b", "handle_b"), ("_", "human")]);
        let graph = lower(&flow).expect("must lower");
        let route = graph.node("classify/route").expect("route node must exist");

        let chosen = route.body.route_port("not_a_declared_option");
        assert_eq!(
            chosen,
            Some("human"),
            "an unmatched answer must reach the fallback"
        );
    }
}
