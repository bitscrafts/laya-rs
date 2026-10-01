//! `laya-graph` — the generic DAG engine.
//!
//! Spec 07's whole determinism argument rests on one property: **the graph
//! owns every edge.** Laya supplies a value — a `label`, a `band` — and never
//! a node id, an edge, or control flow. This crate is the pure half of that
//! argument: parse a flow, refuse what must be refused, and lower a `decide`
//! node into the `mcp` + `route` pair that actually runs. No processes, no
//! store, no threads — see `_docs` spec 07 part 1 for why that boundary
//! matters.
//!
//! * [`flow`] — the authoring surface: the typed flow document, and the
//!   closed set of node kinds.
//! * [`on_expr`] — the `on` grammar: member access on a real payload field,
//!   and nothing else. This is what makes the engine provably unable to
//!   compute a predicate.
//! * [`validate`] — every refusal, including cycle detection.
//! * [`lower`] — `decide` -> `mcp` + `route` + edges, fallback rule last.
//! * [`graph`] — the execution form those stages produce.
//! * [`expand`] — subflow expansion: prefix, bind, hash-verify, refuse a
//!   cycle by name rather than hang (R6, R7).
//! * [`render`] — the Mermaid render, provably lossy and labelled (R14).
//! * [`error`] — every refusal, named.

#![forbid(unsafe_code)]

pub mod error;
pub mod expand;
pub mod flow;
pub mod graph;
pub mod lower;
pub mod on_expr;
pub mod render;
pub mod validate;

pub use error::FlowError;
pub use expand::{Library, expand};
pub use flow::{Flow, Kind, Node, Retry};
pub use graph::{Edge, Graph, GraphNode, NodeBody, RouteRule};
pub use lower::lower;
pub use on_expr::PayloadField;
pub use render::{MermaidRender, render};
pub use validate::validate;
