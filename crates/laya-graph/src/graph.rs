//! The execution form: nodes, edges, and nothing hand-authored.
//!
//! This is the output of [`crate::lower::lower`] — the authoring surface
//! ([`crate::flow::Flow`]) is for people; this is for a scheduler (part 3) to
//! walk.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One rule in a `route` node: match `eq`, or — when `eq` is `None` — the
/// fallback, which matches whatever no other rule did.
///
/// [`crate::lower::lower`] guarantees the fallback rule is always LAST, so
/// the first matching rule (scanned in order) is never the fallback unless
/// nothing else matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRule {
    /// The value `on` must equal for this rule to match. `None` is the
    /// fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<String>,
    /// The port this rule sends the payload to.
    pub port: String,
}

/// What a lowered node actually does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeBody {
    /// Run a shell command; its exit code is the result.
    Shell {
        /// The command line.
        run: String,
    },
    /// Call an MCP tool by name.
    Mcp {
        /// The tool name.
        tool: String,
        /// The tool's arguments.
        arguments: serde_json::Value,
    },
    /// Match `on` against `rules`, first match wins.
    Route {
        /// The member-access expression being matched, e.g. `$.payload.label`.
        on: String,
        /// The match rules. The fallback (`eq: None`) is always last.
        rules: Vec<RouteRule>,
    },
}

impl NodeBody {
    /// For a `route` node, the port the first matching rule sends `value`
    /// to — first-match-wins over [`Self::Route`]'s `rules`, in order.
    ///
    /// `None` for a non-route body, or for a route body whose rules neither
    /// match `value` nor carry a fallback (refused by `validate`'s
    /// [`crate::error::FlowError::IncompletePorts`] before a graph with that
    /// shape can exist).
    #[must_use]
    pub fn route_port(&self, value: &str) -> Option<&str> {
        match self {
            Self::Route { rules, .. } => rules
                .iter()
                .find(|rule| rule.eq.as_deref().is_none_or(|eq| eq == value))
                .map(|rule| rule.port.as_str()),
            Self::Shell { .. } | Self::Mcp { .. } => None,
        }
    }
}

/// One node in the execution form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    /// The node's id, unique within the graph.
    pub id: String,
    /// What the node does.
    pub body: NodeBody,
    /// Whether this node edits a repository (the worktree / mutual-exclusion
    /// switch — scheduled in part 3).
    #[serde(default)]
    pub writes: bool,
    /// Whether a failure here is non-fatal to the run (part 3).
    #[serde(default)]
    pub allow_failure: bool,
    /// The deadline for this node's own work, from the node's `timeout` or
    /// else the flow's. `None` means unbounded (R1, R3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<std::time::Duration>,
}

/// One edge: `from` runs before `to`. `port` is set when the edge
/// originates from a specific `route` rule, so a render (part 2) can label
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The upstream node id.
    pub from: String,
    /// The downstream node id.
    pub to: String,
    /// The route port this edge belongs to, when it originates from a
    /// `route` node's rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
}

/// The execution form a validated, lowered flow becomes. Never
/// hand-authored — only [`crate::lower::lower`] produces one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Graph {
    /// Every node, in the order they were produced.
    pub nodes: Vec<GraphNode>,
    /// Every edge, in the order they were produced.
    pub edges: Vec<Edge>,
}

impl Graph {
    /// The edge set as `(from, to)` pairs, order-independent — for asserting
    /// "exactly these edges exist" without caring what order they were
    /// emitted in.
    #[must_use]
    pub fn edge_set(&self) -> BTreeSet<(String, String)> {
        self.edges
            .iter()
            .map(|edge| (edge.from.clone(), edge.to.clone()))
            .collect()
    }

    /// Find a node by id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}
