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
//! * [`node`] — `shell` and `mcp` node executors (R12).
//! * [`run`] — run records: pure data, carrying `run_id` + `node_id` (R13).
//! * [`sched`] — the scheduler: deterministic order, bounded concurrent
//!   fan-out, fail-fast (R8, R9, R10, R11).
//! * [`route_exec`] — threads a `decide` node's real answer into its `route`
//!   node before scheduling anything downstream (R-ACC, spec 07 part 4): the
//!   piece [`node`]'s own docs name as deliberately out of scope for part 3.

#![forbid(unsafe_code)]

pub mod error;
pub mod expand;
#[cfg(test)]
mod field_contract;
pub mod flow;
pub mod graph;
pub mod lower;
pub mod node;
pub mod on_expr;
pub mod placeholder;
pub mod render;
pub mod route_exec;
#[cfg(test)]
mod routing_order;
pub mod run;
pub mod sched;
#[cfg(test)]
mod shell_value;
pub mod validate;

pub use error::{FlowError, RunError};
pub use expand::{Library, expand};
pub use flow::{Flow, Kind, Node, Retry};
pub use graph::{Edge, Graph, GraphNode, NodeBody, RouteRule};
pub use lower::lower;
pub use node::{CallContext, Executed, McpCaller, execute_mcp, execute_shell};
pub use on_expr::PayloadField;
pub use render::{MermaidRender, render};
pub use route_exec::run_flow;
pub use run::{NodeRecord, RunReport};
pub use sched::{RunOptions, run, topo_order};
pub use validate::validate;
