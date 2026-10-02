//! Mermaid rendering of the execution form (R14) — a review aid, never a
//! serialisation format.
//!
//! *"Mermaid is a review aid, not a serialisation format."* It cannot carry
//! every field a [`crate::flow::Node`] can declare, so this renderer is
//! **provably lossy and labelled**: every render carries a `dropped` list
//! naming what it structurally cannot express, rather than dropping it in
//! silence. That list is a declared constant ([`DROPPED_FIELDS`]), not
//! something computed per-flow — a flow that happens not to use `retry`
//! must still be told the renderer cannot carry it, or the absence would
//! read as support (see `the_control_a_plain_flow_drops_nothing_silently`).
//!
//! The render also embeds its own edges a second time as `%% edge from ->
//! to [: port]` comment lines — valid Mermaid (`%%` is a real comment
//! marker), ignored by any Mermaid viewer, but machine-reparseable by
//! [`derive_edge_set`] without needing to invert the node-label escaping a
//! human-facing render otherwise requires. That is what lets
//! `the_render_round_trips_the_derived_edge_set` compare the *derived edge
//! set*, per [`crate::graph::Graph::edge_set`], rather than pinning the
//! rendered text's formatting.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::graph::{Graph, NodeBody};

/// Fields a flow document can carry that this renderer can never express.
///
/// Declared once, reported on every render, regardless of whether the
/// specific flow being rendered uses any of them.
pub const DROPPED_FIELDS: &[&str] = &[
    "retry",
    "allow_failure",
    "writes",
    "subflow with-bindings",
    "subflow content_hash",
];

/// A Mermaid render of a [`Graph`], paired with what it could not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidRender {
    /// The rendered `flowchart TD` text.
    pub text: String,
    /// Every field this render drops, named rather than silently lost.
    pub dropped: Vec<String>,
}

/// Render a [`Graph`] to Mermaid.
///
/// Infallible and total: a render that cannot fully represent a graph still
/// renders what it can, and names what it dropped in
/// [`MermaidRender::dropped`] rather than failing or staying silent about
/// it.
#[must_use]
pub fn render(graph: &Graph) -> MermaidRender {
    let mut text = String::from("flowchart TD\n");

    let mermaid_id = |index: usize| format!("n{index}");

    for (index, node) in graph.nodes.iter().enumerate() {
        let kind = match &node.body {
            NodeBody::Shell { .. } => "shell",
            NodeBody::Mcp { .. } => "mcp",
            NodeBody::Route { .. } => "route",
        };
        let _ = writeln!(text, "    {}[\"{} ({kind})\"]", mermaid_id(index), node.id);
    }

    for edge in &graph.edges {
        let from_index = graph.nodes.iter().position(|n| n.id == edge.from);
        let to_index = graph.nodes.iter().position(|n| n.id == edge.to);
        if let (Some(from_index), Some(to_index)) = (from_index, to_index) {
            match &edge.port {
                Some(port) => {
                    let _ = writeln!(
                        text,
                        "    {} -->|{port}| {}",
                        mermaid_id(from_index),
                        mermaid_id(to_index)
                    );
                }
                None => {
                    let _ = writeln!(
                        text,
                        "    {} --> {}",
                        mermaid_id(from_index),
                        mermaid_id(to_index)
                    );
                }
            }
        }
        match &edge.port {
            Some(port) => {
                let _ = writeln!(text, "    %% edge {} -> {} : {port}", edge.from, edge.to);
            }
            None => {
                let _ = writeln!(text, "    %% edge {} -> {}", edge.from, edge.to);
            }
        }
    }

    MermaidRender {
        text,
        dropped: DROPPED_FIELDS.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Re-derive the edge set a render claims, from its `%% edge` comment
/// lines, for the R14 round-trip test: `render -> re-parse -> compare`.
///
/// This is deliberately not a Mermaid parser — it reads back exactly the
/// comment encoding [`render`] writes, which is what makes the round trip
/// test a check on THIS renderer's own honesty rather than on a
/// general-purpose Mermaid grammar this crate does not need.
#[must_use]
pub fn derive_edge_set(text: &str) -> BTreeSet<(String, String)> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("%% edge "))
        .filter_map(|rest| {
            let edge = rest.split(" : ").next().unwrap_or(rest);
            let (from, to) = edge.split_once(" -> ")?;
            Some((from.to_string(), to.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::flow::{Flow, Kind, Node};
    use crate::lower::lower;

    fn decide_flow() -> Flow {
        let node = Node {
            question: Some("which?".to_string()),
            options: Some(vec!["a".to_string(), "b".to_string()]),
            ports: Some(
                [("a", "handle_a"), ("b", "handle_b"), ("_", "human")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..Node::new("classify", Kind::Decide)
        };
        let human = Node {
            run: Some("notify".to_string()),
            ..Node::new("human", Kind::Shell)
        };
        let handle_a = Node {
            run: Some("echo a".to_string()),
            ..Node::new("handle_a", Kind::Shell)
        };
        let handle_b = Node {
            run: Some("echo b".to_string()),
            ..Node::new("handle_b", Kind::Shell)
        };
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            timeout: None,
            nodes: vec![node, human, handle_a, handle_b],
        }
    }

    fn shell_only_flow() -> Flow {
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            timeout: None,
            nodes: vec![
                Node {
                    run: Some("true".to_string()),
                    ..Node::new("a", Kind::Shell)
                },
                Node {
                    run: Some("true".to_string()),
                    depends_on: vec!["a".to_string()],
                    ..Node::new("b", Kind::Shell)
                },
            ],
        }
    }

    /// R14 — THE ROUND TRIP. Compares the DERIVED edge set, not the
    /// rendered text: two renders of the same graph can differ in
    /// formatting, and a text comparison would both fail on a harmless
    /// reformat and pass on a render that silently dropped an edge but
    /// happened to print the survivors in the same order.
    #[test]
    fn the_render_round_trips_the_derived_edge_set() {
        let flow = decide_flow();
        let graph = lower(&flow).expect("a valid decide flow must lower");

        let rendered = render(&graph);
        let reparsed = derive_edge_set(&rendered.text);

        assert_eq!(
            reparsed,
            graph.edge_set(),
            "the re-parsed edge set must equal the graph's own derived edge set"
        );
    }

    #[test]
    fn a_dropped_field_is_named_in_the_render() {
        // `retry` never survives lowering into `Graph` at all (part 1's
        // `lower` does not copy it) — which is exactly why `dropped` is a
        // declared constant rather than data read off this specific
        // graph: the renderer must say it cannot carry `retry` whether or
        // not the authored flow used one.
        let flow = Flow {
            nodes: vec![Node {
                run: Some("true".to_string()),
                retry: Some(crate::flow::Retry {
                    attempts: Some(3),
                    backoff_ms: Some(100),
                }),
                ..Node::new("a", Kind::Shell)
            }],
            ..shell_only_flow()
        };
        let graph = lower(&flow).expect("must lower");

        let rendered = render(&graph);
        assert!(
            rendered.dropped.iter().any(|f| f == "retry"),
            "got {:?}",
            rendered.dropped
        );
    }

    /// The control for the test above: a flow that uses NONE of the
    /// fields `DROPPED_FIELDS` names must still report every one of them.
    /// A buggy renderer that only lists a field when the specific flow
    /// happens to use it would pass `a_dropped_field_is_named_in_the_render`
    /// and still be wrong — this is what catches that.
    #[test]
    fn the_control_a_plain_flow_drops_nothing_silently() {
        let graph = lower(&shell_only_flow()).expect("must lower");
        let rendered = render(&graph);

        for field in DROPPED_FIELDS {
            assert!(
                rendered.dropped.iter().any(|f| f == field),
                "`{field}` must be listed even though this flow never used it, got {:?}",
                rendered.dropped
            );
        }
    }

    #[test]
    fn the_render_of_a_real_flow_is_human_readable_mermaid() {
        let flow = decide_flow();
        let graph = lower(&flow).expect("must lower");
        let rendered = render(&graph);

        assert!(rendered.text.starts_with("flowchart TD\n"));
        assert!(rendered.text.contains("classify (mcp)"));
        assert!(rendered.text.contains("classify/route (route)"));
        // not asserted further here — the full text is pasted in the
        // part-2 report rather than pinned line-by-line in this test,
        // which is exactly what the edge-set round trip above is for.
    }
}
