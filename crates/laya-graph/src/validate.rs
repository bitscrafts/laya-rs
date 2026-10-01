//! Parse-time refusals: everything in [`crate::flow::Flow`] that must be
//! rejected rather than silently tolerated, plus cycle detection.
//!
//! Every refusal is a named [`FlowError`] carrying the offending id — "invalid
//! flow" alone is not acceptable. An unknown node kind is always an error,
//! never a no-op node that "succeeds".

use std::collections::{HashMap, HashSet};

use crate::error::FlowError;
use crate::flow::{Flow, Kind, Node};

/// Validate a parsed flow, refusing rather than warning.
///
/// Checks, in order: duplicate node ids; `depends_on` naming an unknown node;
/// an unsupported `kind`; a `decide` node whose `ports` do not cover its
/// `options` and has no `_` fallback; half a retry policy;
/// `escalate_on_near_tie` without `near_tie_port`; a cycle in `depends_on`.
///
/// # Errors
///
/// Returns the first [`FlowError`] found, named with the offending node id
/// (and, for a cycle, the full chain).
pub fn validate(flow: &Flow) -> Result<(), FlowError> {
    let mut seen = HashSet::new();
    for node in &flow.nodes {
        if !seen.insert(node.id.as_str()) {
            return Err(FlowError::DuplicateNodeId {
                id: node.id.clone(),
            });
        }
    }

    let ids: HashSet<&str> = flow.nodes.iter().map(|n| n.id.as_str()).collect();
    for node in &flow.nodes {
        for dep in &node.depends_on {
            if !ids.contains(dep.as_str()) {
                return Err(FlowError::DanglingDependsOn {
                    id: node.id.clone(),
                    missing: dep.clone(),
                });
            }
        }
    }

    for node in &flow.nodes {
        if let Kind::Other(kind) = &node.kind {
            return Err(FlowError::UnknownKind {
                id: node.id.clone(),
                kind: kind.clone(),
            });
        }
        if node.kind == Kind::Decide {
            validate_decide(node)?;
        }
        validate_retry(node)?;
    }

    detect_cycle(flow)
}

fn validate_decide(node: &Node) -> Result<(), FlowError> {
    let options = node.options.as_deref().unwrap_or_default();
    let ports = node.ports.as_ref();
    let has_fallback = ports.is_some_and(|p| p.contains_key("_"));

    let missing: Vec<String> = options
        .iter()
        .filter(|option| !ports.is_some_and(|p| p.contains_key(option.as_str())))
        .cloned()
        .collect();
    if !missing.is_empty() && !has_fallback {
        return Err(FlowError::IncompletePorts {
            id: node.id.clone(),
            missing,
        });
    }

    if node.escalate_on_near_tie && node.near_tie_port.is_none() {
        return Err(FlowError::MissingNearTiePort {
            id: node.id.clone(),
        });
    }

    Ok(())
}

fn validate_retry(node: &Node) -> Result<(), FlowError> {
    if let Some(retry) = &node.retry
        && retry.attempts.is_some() != retry.backoff_ms.is_some()
    {
        return Err(FlowError::HalfRetryPolicy {
            id: node.id.clone(),
        });
    }
    Ok(())
}

/// Depth-first cycle detection over `depends_on`, tri-colour marked so it
/// always returns — a node already `InProgress` ends the search with a named
/// chain instead of recursing again, so a cycle cannot turn this into a hang.
fn detect_cycle(flow: &Flow) -> Result<(), FlowError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }

    fn visit<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a Node>,
        marks: &mut HashMap<&'a str, Mark>,
        path: &mut Vec<&'a str>,
    ) -> Result<(), FlowError> {
        marks.insert(id, Mark::InProgress);
        path.push(id);

        if let Some(node) = by_id.get(id) {
            for dep in &node.depends_on {
                let dep = dep.as_str();
                match marks.get(dep).copied().unwrap_or(Mark::Unvisited) {
                    Mark::InProgress => {
                        let start = path.iter().position(|p| *p == dep).unwrap_or(0);
                        let mut chain: Vec<&str> = path.iter().skip(start).copied().collect();
                        chain.push(dep);
                        return Err(FlowError::Cycle {
                            chain: chain.join(" -> "),
                        });
                    }
                    Mark::Done => {}
                    Mark::Unvisited => visit(dep, by_id, marks, path)?,
                }
            }
        }

        marks.insert(id, Mark::Done);
        path.pop();
        Ok(())
    }

    let by_id: HashMap<&str, &Node> = flow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut marks: HashMap<&str, Mark> = flow
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), Mark::Unvisited))
        .collect();
    let mut path: Vec<&str> = Vec::new();

    for node in &flow.nodes {
        let id = node.id.as_str();
        if marks.get(id).copied().unwrap_or(Mark::Unvisited) == Mark::Unvisited {
            visit(id, &by_id, &mut marks, &mut path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::flow::Retry;

    fn shell(id: &str, depends_on: &[&str]) -> Node {
        Node {
            run: Some("true".to_string()),
            depends_on: depends_on.iter().map(|s| (*s).to_string()).collect(),
            ..Node::new(id, Kind::Shell)
        }
    }

    fn decide(id: &str, options: &[&str], ports: &[(&str, &str)]) -> Node {
        Node {
            question: Some("q?".to_string()),
            options: Some(options.iter().map(|s| (*s).to_string()).collect()),
            ports: Some(
                ports
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ),
            ..Node::new(id, Kind::Decide)
        }
    }

    fn flow(nodes: Vec<Node>) -> Flow {
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            nodes,
        }
    }

    #[test]
    fn a_duplicate_node_id_is_refused() {
        let f = flow(vec![shell("a", &[]), shell("a", &[])]);
        let err = validate(&f).expect_err("duplicate id must be refused");
        assert!(
            matches!(err, FlowError::DuplicateNodeId { ref id } if id == "a"),
            "message must name `a`, got {err:?}"
        );
    }

    #[test]
    fn a_dangling_depends_on_is_refused() {
        let f = flow(vec![shell("a", &["ghost"])]);
        let err = validate(&f).expect_err("dangling depends_on must be refused");
        assert!(
            matches!(err, FlowError::DanglingDependsOn { ref missing, .. } if missing == "ghost"),
            "message must name `ghost`, got {err:?}"
        );
    }

    #[test]
    fn a_decide_without_a_fallback_is_refused() {
        let f = flow(vec![decide(
            "classify",
            &["a", "b", "c"],
            &[("a", "x"), ("b", "y")],
        )]);
        let err = validate(&f).expect_err("incomplete ports with no fallback must be refused");
        assert!(
            matches!(err, FlowError::IncompletePorts { ref id, .. } if id == "classify"),
            "the port set is incomplete, got {err:?}"
        );
    }

    #[test]
    fn half_a_retry_policy_is_refused() {
        let node = Node {
            retry: Some(Retry {
                attempts: Some(3),
                backoff_ms: None,
            }),
            ..shell("a", &[])
        };
        let f = flow(vec![node]);
        let err = validate(&f).expect_err("half a retry policy is a retry storm");
        assert!(matches!(err, FlowError::HalfRetryPolicy { ref id } if id == "a"));
    }

    #[test]
    fn escalate_on_near_tie_without_a_port_is_refused() {
        let node = Node {
            escalate_on_near_tie: true,
            ..decide(
                "classify",
                &["a", "b"],
                &[("a", "x"), ("b", "y"), ("_", "z")],
            )
        };
        let f = flow(vec![node]);
        let err = validate(&f).expect_err("escalate_on_near_tie needs a destination");
        assert!(matches!(err, FlowError::MissingNearTiePort { ref id } if id == "classify"));
    }

    #[test]
    fn a_dependency_cycle_is_refused() {
        let f = flow(vec![shell("a", &["b"]), shell("b", &["a"])]);
        let err = validate(&f).expect_err("a cycle must be refused, and must return");
        let FlowError::Cycle { chain } = err else {
            panic!("expected Cycle, got {err:?}");
        };
        assert!(chain.contains('a') && chain.contains('b'), "got {chain}");
    }

    #[test]
    fn an_unknown_node_kind_is_refused() {
        let json = r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shelll"}]}"#;
        let f = Flow::parse(json).expect("bad kind must still PARSE");
        let err = validate(&f).expect_err("an unknown kind must never become a no-op node");
        assert!(matches!(err, FlowError::UnknownKind { ref kind, .. } if kind == "shelll"));
    }

    #[test]
    fn agent_and_quorum_are_not_supported_kinds() {
        for kind in ["agent", "quorum"] {
            let json =
                format!(r#"{{"flow":"t","version":1,"nodes":[{{"id":"a","kind":"{kind}"}}]}}"#);
            let f = Flow::parse(&json).expect("must still parse");
            let err = validate(&f).expect_err("v1 implements neither agent nor quorum");
            assert!(matches!(err, FlowError::UnknownKind { kind: ref k, .. } if k == kind));
        }
    }

    #[test]
    fn the_control_a_valid_flow_is_accepted() {
        let f = flow(vec![
            decide(
                "classify",
                &["a", "b"],
                &[("a", "x"), ("b", "y"), ("_", "z")],
            ),
            shell("x", &[]),
            shell("y", &[]),
            shell("z", &[]),
        ]);
        assert!(validate(&f).is_ok(), "a well-formed flow must validate");
    }
}
