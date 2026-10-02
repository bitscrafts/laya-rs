//! The scheduler: a bounded, concurrent fan-out over a lowered
//! [`crate::graph::Graph`] (R8, R9, R10, R11).
//!
//! **A bounded concurrency limit, not the priority queue.**
//! `laya-core::queue::PriorityQueue` orders work by *class*, for "which
//! request to serve next"; the limiter here bounds *how many may overlap at
//! once*. The two solve different problems — conflating them is exactly what
//! the forbidding test in `laya-mcp` exists to prevent (R16). This crate does
//! not use `PriorityQueue`.
//!
//! **The schedule order is deterministic (R8); the interleaving is not
//! (N6).** [`topo_order`] always breaks ties by node id, so two builds of the
//! same graph dispatch nodes in the same order. Once dispatched, how two
//! concurrently-running nodes interleave in real time is explicitly not
//! reproducible, and nothing here claims otherwise.
//!
//! **This scheduler owns every edge, including the conditional ones (spec 17).** It used to be
//! "frozen" (parts 1-3 committed and consumed) and `route_exec` resolved each `decide` in a
//! separate drain *before* handing the remainder here; that drain pruned by array position, so
//! which nodes ran depended on the order of the flow's `nodes`. The freeze is lifted on
//! purpose: a `route` node is now resolved HERE, when its turn comes, from its upstream node's
//! recorded output, and the graph alone decides what is live.
//!
//! Two edge classes, stated once:
//! * a plain edge (`depends_on`, and the `decide -> route` link) is an **AND**: if any such
//!   predecessor is dead or skipped, the node is skipped, and skipping propagates;
//! * a port edge (`route -> target`) is an **OR**: the target is reachable if any route
//!   selected it.
//!
//! A node with BOTH is live **iff at least one port edge was selected AND every plain
//! predecessor completed** — the gate -> ask pattern of `wc-classify-chained` is exactly this.
//! A node is settled (live or skipped) only when every inbound edge is resolved.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};

use crate::error::RunError;
use crate::graph::{Graph, GraphNode, NodeBody};
use crate::node::{CallContext, Executed, McpCaller, execute_mcp, execute_shell, failed};
use crate::placeholder;
use crate::route_exec::resolve_route;
use crate::run::{NodeRecord, RunReport};
use laya_core::agentic::STATE_CAP_BYTES;

/// The deterministic topological order of `graph`'s nodes (R8).
///
/// Kahn's algorithm, ties broken by sorting the ready set by id at each step.
/// Two builds of the same graph therefore produce byte-identical orders.
///
/// Shorter than `graph.nodes.len()` only when `graph` is not a DAG —
/// defensive: [`crate::validate::validate`] refuses a cycle before a flow can
/// be lowered, but `Graph`'s fields are public, so nothing enforces that on a
/// hand-built one.
#[must_use]
pub fn topo_order(graph: &Graph) -> Vec<String> {
    // An inconsistent table orders nothing: the empty order is "shorter than the graph", which
    // every caller already treats as unorderable. `refuse_cycle` surfaces the named error.
    try_topo_order(graph).unwrap_or_default()
}

/// [`topo_order`], refusing by name a dependency table that is not about `graph` (spec 23, R1).
fn try_topo_order(graph: &Graph) -> Result<Vec<String>, RunError> {
    let (mut indegree, successors) = dependency_tables(graph);
    verify_tables(graph, &indegree)?;
    let mut ready: BTreeSet<String> = indegree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut order = Vec::with_capacity(graph.nodes.len());
    while let Some(id) = pop_first(&mut ready) {
        if let Some(succs) = successors.get(&id) {
            for succ in succs {
                if let Some(deg) = indegree.get_mut(succ) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.insert(succ.clone());
                    }
                }
            }
        }
        order.push(id);
    }
    Ok(order)
}

/// `(indegree, successors)` for every node `graph.nodes` declares — built
/// once and shared by [`topo_order`] and [`run`], so the two can never
/// disagree about the graph's shape.
fn dependency_tables(graph: &Graph) -> (HashMap<String, usize>, HashMap<String, Vec<String>>) {
    let mut indegree = HashMap::new();
    let mut successors: HashMap<String, Vec<String>> = HashMap::new();
    for node in &graph.nodes {
        indegree.entry(node.id.clone()).or_insert(0);
    }
    for edge in &graph.edges {
        *indegree.entry(edge.to.clone()).or_insert(0) += 1;
        successors
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    (indegree, successors)
}

/// Assert the invariant [`dependency_tables`] owes every consumer: an entry for every node id
/// `graph.nodes` declares (spec 23, R1). Checked by the caller on what it was handed, so a
/// replacement of `dependency_tables` itself cannot skip it.
fn verify_tables(graph: &Graph, indegree: &HashMap<String, usize>) -> Result<(), RunError> {
    let missing: BTreeSet<String> = graph
        .nodes
        .iter()
        .filter(|n| !indegree.contains_key(&n.id))
        .map(|n| n.id.clone())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RunError::InconsistentTable {
            missing: missing.into_iter().collect(),
        })
    }
}

/// The smallest id in `set`, removed — the tie-break R8 requires.
fn pop_first(set: &mut BTreeSet<String>) -> Option<String> {
    let first = set.iter().next().cloned()?;
    set.remove(&first);
    Some(first)
}

/// What the scheduler needs beyond the graph itself.
pub struct RunOptions {
    /// This run's id, stamped onto every record (R13).
    pub run_id: String,
    /// The fan-out bound (R9). Zero is refused, never silently clamped: a
    /// caller that asks for zero asked for something that cannot run at all.
    pub limit: usize,
    /// Where an `mcp` node's call actually goes. Injected so this crate never
    /// depends on `laya-mcp` (N2, G16); real wiring lives at the boundary
    /// where both crates meet.
    pub mcp: Arc<dyn McpCaller>,
}

/// Run `graph` to completion, or until a non-`allow_failure` node fails.
///
/// Nodes whose dependencies are satisfied fan out concurrently, bounded by
/// `opts.limit` (R9); two `writes: true` nodes never overlap (R10). A
/// failing node — unless it declares `allow_failure` — stops the run:
/// siblings not yet started are not started, in-flight work is not
/// cancelled, and the returned [`RunReport::stopped_early`] is `true` with
/// [`RunReport::failed_node`] named (R11).
///
/// A node's `timeout` is enforced (R1): an expired shell node is killed with
/// its whole process group and recorded `timed_out`. Shell nodes spawn through
/// `tokio::process`, so the runtime this runs on must have its I/O driver
/// enabled (`enable_all`).
///
/// # Errors
/// [`RunError::BadLimit`] if `opts.limit` is zero. [`RunError::UnknownNode`]
/// if an edge names a node the graph does not declare. [`RunError::Cycle`] if
/// the graph is not a DAG — refused before anything runs. [`RunError::NodeTaskFailed`]
/// if a spawned node's own scheduling task could not be joined — an executor
/// bug, never a node's own reported failure (that lives in the returned
/// `RunReport`, not here; see R11, R12).
pub async fn run(graph: &Graph, opts: RunOptions) -> Result<RunReport, RunError> {
    run_from(graph, opts, Instant::now()).await
}

/// [`run`], measuring every `started_at_ms` and `wall_ms` from `origin`.
///
/// A caller that ran part of the flow before handing the rest here ([`crate::route_exec`]) passes
/// the instant the WHOLE run began, so the timeline of the tail is not reset to zero.
///
/// # Errors
/// As [`run`].
pub async fn run_from(
    graph: &Graph,
    opts: RunOptions,
    origin: Instant,
) -> Result<RunReport, RunError> {
    if opts.limit == 0 {
        return Err(RunError::BadLimit { limit: opts.limit });
    }
    for edge in &graph.edges {
        if graph.node(&edge.from).is_none() {
            return Err(RunError::UnknownNode {
                node_id: edge.from.clone(),
            });
        }
        if graph.node(&edge.to).is_none() {
            return Err(RunError::UnknownNode {
                node_id: edge.to.clone(),
            });
        }
    }

    refuse_cycle(graph)?;

    let tracker = Tracker::new(graph);
    run_tracked(graph, opts, origin, tracker).await
}

/// [`run_from`] after validation, with the tracker supplied — so a test can hand it a tracker
/// that is deliberately inconsistent with the graph (spec 23, R3).
async fn run_tracked(
    graph: &Graph,
    opts: RunOptions,
    origin: Instant,
    mut tracker: Tracker,
) -> Result<RunReport, RunError> {
    let halt = Arc::new(AtomicBool::new(false));
    let gates = Gates {
        fanout: Arc::new(Semaphore::new(opts.limit)),
        write_lock: Arc::new(Semaphore::new(1)),
        halt: Arc::clone(&halt),
    };
    let mut joins: JoinSet<Outcome> = JoinSet::new();
    let mut records = Vec::new();
    let mut failed_node = dispatch(
        graph,
        &mut tracker,
        &mut records,
        &mut joins,
        &opts,
        &gates,
        origin,
    )?;
    let mut stopped_early = failed_node.is_some();
    let mut stop_scheduling = stopped_early;

    while let Some(outcome) = joins.join_next().await {
        let record = match outcome {
            Ok(Outcome::Ran(record, stdout)) => {
                tracker.stdouts.insert(record.node_id.clone(), stdout);
                record
            }
            // Halted before its work began: it gets no record, and
            // `nodes_without_record` accounts for it below (R2).
            Ok(Outcome::Halted) => continue,
            Err(e) => {
                return Err(RunError::NodeTaskFailed {
                    reason: e.to_string(),
                });
            }
        };

        let allow_failure = graph.node(&record.node_id).is_some_and(|n| n.allow_failure);
        let failed = !record.success;
        let completed = record.node_id.clone();
        records.push(record);

        if failed && !allow_failure {
            // Fail fast (R11): stop admitting new work. Already-spawned
            // tasks are in-flight and are not cancelled (N4/spec: "no
            // cancellation of in-flight work"); only nodes not yet dispatched
            // are affected, because `stop_scheduling` gates `dispatch`
            // below and the failed node's own successors are never unlocked.
            stopped_early = true;
            if failed_node.is_none() {
                failed_node = Some(completed);
            }
            stop_scheduling = true;
        } else if !stop_scheduling {
            tracker.resolve(&completed, |_| true);
            if let Some(failed_route) = dispatch(
                graph,
                &mut tracker,
                &mut records,
                &mut joins,
                &opts,
                &gates,
                origin,
            )? {
                stopped_early = true;
                if failed_node.is_none() {
                    failed_node = Some(failed_route);
                }
                stop_scheduling = true;
            }
        }
    }

    let skipped: Vec<String> = tracker.skipped.iter().cloned().collect();
    let not_started = nodes_without_record(graph, &records, &tracker.skipped);
    // A run nothing stopped must have settled every node (spec 23, R2). Leftovers here are a
    // scheduler that went quiet with work undone — reported by name, not as a complete report.
    if !stopped_early && !not_started.is_empty() {
        return Err(stalled(graph, &records, &tracker, not_started));
    }

    Ok(RunReport {
        run_id: opts.run_id,
        stopped_early,
        failed_node,
        worker_limit: opts.limit,
        records,
        not_started,
        skipped,
        skip_reasons: tracker.reasons.clone(),
        wall_ms: elapsed_ms(origin),
    })
}

/// One outgoing edge of a node, as the tracker sees it.
struct OutEdge {
    to: String,
    /// The `port` label when this is a `route` rule's edge (an OR edge); `None` for an AND edge.
    port: Option<String>,
}

/// What a node is still waiting on.
#[derive(Default)]
struct Inbound {
    /// Plain (AND) edges not yet resolved.
    and_left: usize,
    /// Some plain predecessor is dead or skipped.
    and_dead: bool,
    /// Port (OR) edges in total, and not yet resolved.
    port_total: usize,
    port_left: usize,
    /// Some route selected the port leading here.
    port_selected: bool,
}

/// Edge-resolution state for [`run_from`]: which nodes are ready, which are skipped.
struct Tracker {
    state: HashMap<String, Inbound>,
    out: HashMap<String, Vec<OutEdge>>,
    ready: BTreeSet<String>,
    skipped: BTreeSet<String>,
    /// Nodes `dispatch` has already taken from `ready`: a node is settled once (spec 23, R2).
    dispatched: BTreeSet<String>,
    /// Why a node was skipped, where the reason is more than "its path was not taken" (spec 18).
    reasons: BTreeMap<String, String>,
    /// Each ran node's stdout alone, for `{{node_id}}` substitution (spec 18).
    stdouts: HashMap<String, String>,
}

impl Tracker {
    fn new(graph: &Graph) -> Self {
        let mut state: HashMap<String, Inbound> = graph
            .nodes
            .iter()
            .map(|n| (n.id.clone(), Inbound::default()))
            .collect();
        let mut out: HashMap<String, Vec<OutEdge>> = HashMap::new();
        for edge in &graph.edges {
            let inbound = state.entry(edge.to.clone()).or_default();
            if edge.port.is_some() {
                inbound.port_total += 1;
                inbound.port_left += 1;
            } else {
                inbound.and_left += 1;
            }
            out.entry(edge.from.clone()).or_default().push(OutEdge {
                to: edge.to.clone(),
                port: edge.port.clone(),
            });
        }
        let ready = state
            .iter()
            .filter(|(_, s)| s.and_left == 0 && s.port_left == 0)
            .map(|(id, _)| id.clone())
            .collect();
        Self {
            state,
            out,
            ready,
            skipped: BTreeSet::new(),
            dispatched: BTreeSet::new(),
            reasons: BTreeMap::new(),
            stdouts: HashMap::new(),
        }
    }

    /// Skip `id` for `reason` and propagate: its own outgoing edges resolve dead.
    fn skip_with_reason(&mut self, id: &str, reason: String) {
        self.skipped.insert(id.to_string());
        self.reasons.insert(id.to_string(), reason);
        self.resolve(id, |_| false);
    }

    /// Resolve every outgoing edge of `from` (which completed, or was skipped when `live` says
    /// no): `live(edge)` is whether that edge carries a completion. A node whose last inbound
    /// edge resolves is settled — ready, or skipped — and a skip propagates to its own edges.
    fn resolve(&mut self, from: &str, live: impl Fn(&OutEdge) -> bool) {
        let mut work: Vec<(String, bool, bool)> = self
            .out
            .get(from)
            .map(|edges| {
                edges
                    .iter()
                    .map(|e| (e.to.clone(), e.port.is_some(), live(e)))
                    .collect()
            })
            .unwrap_or_default();
        while let Some((to, is_port, edge_live)) = work.pop() {
            let Some(inbound) = self.state.get_mut(&to) else {
                continue;
            };
            if is_port {
                inbound.port_left = inbound.port_left.saturating_sub(1);
                inbound.port_selected |= edge_live;
            } else {
                inbound.and_left = inbound.and_left.saturating_sub(1);
                inbound.and_dead |= !edge_live;
            }
            if inbound.and_left > 0 || inbound.port_left > 0 {
                continue;
            }
            let node_live = !inbound.and_dead && (inbound.port_total == 0 || inbound.port_selected);
            if node_live {
                self.ready.insert(to);
            } else {
                self.skipped.insert(to.clone());
                if let Some(edges) = self.out.get(&to) {
                    work.extend(
                        edges
                            .iter()
                            .map(|e| (e.to.clone(), e.port.is_some(), false)),
                    );
                }
            }
        }
    }
}

/// Drain the ready set in id order (R8): spawn each `shell`/`mcp` node, and resolve each
/// `route` node on the spot from its upstream node's record — selecting ONE port edge and
/// killing the rest. Resolving a route can ready more nodes, hence the loop.
///
/// `Ok(Some(id))` names a route whose value could not be read: its failed record is pushed and
/// the caller stops the run (R4 — a wrong-typed decide answer is data, not `Err`).
fn dispatch(
    graph: &Graph,
    tracker: &mut Tracker,
    records: &mut Vec<NodeRecord>,
    joins: &mut JoinSet<Outcome>,
    opts: &RunOptions,
    gates: &Gates,
    origin: Instant,
) -> Result<Option<String>, RunError> {
    while let Some(id) = pop_first(&mut tracker.ready) {
        // Every iteration must spawn, settle or skip a node, and never the same node twice
        // (spec 23, R2): a repeat is a scheduler that could loop, so it is named instead.
        if !tracker.dispatched.insert(id.clone()) || tracker.skipped.contains(&id) {
            return Err(RunError::SettledTwice { node_id: id });
        }
        // Unreachable once `run_from` has validated every edge's endpoints exist. An iteration
        // that does nothing is the spin R2 forbids, so it is a named stall, not a `continue`.
        let Some(node) = graph.node(&id).cloned() else {
            return Err(RunError::Stalled {
                unsettled: vec![id],
                unresolved_edges: Vec::new(),
            });
        };
        if matches!(node.body, NodeBody::Route { .. }) {
            let upstream = graph
                .edges
                .iter()
                .find(|e| e.to == id && e.port.is_none())
                .and_then(|e| records.iter().find(|r| r.node_id == e.from))
                .ok_or_else(|| RunError::UnknownNode {
                    node_id: id.clone(),
                })?;
            let (route_record, selected) =
                resolve_route(&opts.run_id, &node, &upstream.output, origin)?;
            let route_ok = route_record.success;
            records.push(route_record);
            let Some(selected) = selected.filter(|_| route_ok) else {
                // Spec 21: a route tolerated like its decide selects no port — every target is
                // skipped — and the run continues; otherwise the unreadable route stops it.
                if node.allow_failure {
                    tracker.resolve(&id, |_| false);
                    continue;
                }
                return Ok(Some(id));
            };
            tracker.resolve(&id, |edge| edge.to == selected);
            continue;
        }
        let node = match substitute_prompt(node, records, &tracker.stdouts) {
            Ok(node) => node,
            Err((id, reason)) => {
                tracker.skip_with_reason(&id, reason);
                continue;
            }
        };
        let run_id = opts.run_id.clone();
        let fanout = Arc::clone(&gates.fanout);
        let write_lock = Arc::clone(&gates.write_lock);
        let halt = Arc::clone(&gates.halt);
        let mcp = Arc::clone(&opts.mcp);
        joins.spawn(run_one(node, run_id, fanout, write_lock, halt, mcp, origin));
    }
    Ok(None)
}

/// Resolve `{{node_id}}` in a lowered decide's `prompt` just before it is dispatched (spec 18).
///
/// A node with no placeholder is returned untouched. A referent that failed (even with
/// `allow_failure`), has no record, printed nothing, or printed more than
/// [`STATE_CAP_BYTES`] skips the decide: `Err((id, reason))`. An empty string is never
/// substituted, because a decision over nothing reads exactly like one over real data.
fn substitute_prompt(
    mut node: GraphNode,
    records: &[NodeRecord],
    stdouts: &HashMap<String, String>,
) -> Result<GraphNode, (String, String)> {
    let NodeBody::Mcp { tool, arguments } = &mut node.body else {
        return Ok(node);
    };
    if tool != "decide" {
        return Ok(node);
    }
    let Some(prompt) = arguments.get("prompt").and_then(|p| p.as_str()) else {
        return Ok(node);
    };
    let refs = placeholder::references(prompt);
    if refs.is_empty() {
        return Ok(node);
    }
    let id = node.id.clone();
    let skip = |reason: String| (id.clone(), reason);
    for target in refs {
        let Some(record) = records.iter().find(|r| r.node_id == target) else {
            return Err(skip(format!("referenced node `{target}` has no record")));
        };
        if !record.success {
            return Err(skip(format!(
                "referenced node `{target}` failed ({})",
                record.error.as_deref().unwrap_or("no error recorded")
            )));
        }
        let value = stdouts.get(target).map_or("", |s| s.trim());
        if value.is_empty() {
            return Err(skip(format!("referenced node `{target}` printed nothing")));
        }
        if value.len() > STATE_CAP_BYTES {
            return Err(skip(format!(
                "referenced node `{target}` printed {} bytes, over the {STATE_CAP_BYTES}-byte state cap",
                value.len()
            )));
        }
    }
    let substituted = placeholder::substitute(prompt, |t| stdouts.get(t).map(|s| s.trim()))
        .map_err(|t| skip(format!("referenced node `{t}` has no stdout")))?;
    if let Some(slot) = arguments.get_mut("prompt") {
        *slot = serde_json::Value::String(substituted);
    }
    Ok(node)
}

/// The shared gates every node passes before its work begins.
struct Gates {
    fanout: Arc<Semaphore>,
    write_lock: Arc<Semaphore>,
    /// Set by the failing node itself, before it releases its permits; checked after the permits
    /// are held, so a node spawned but still queued never starts (R1).
    halt: Arc<AtomicBool>,
}

/// What a spawned node task came to: its record, or that it was halted
/// before its work began.
enum Outcome {
    Ran(NodeRecord, String),
    Halted,
}

/// Refuse a graph `topo_order` cannot fully order, naming what is left over
/// (R3). Runs before anything executes.
fn refuse_cycle(graph: &Graph) -> Result<(), RunError> {
    let ordered = try_topo_order(graph)?;
    if ordered.len() >= graph.nodes.len() {
        return Ok(());
    }
    let done: BTreeSet<&String> = ordered.iter().collect();
    let nodes: BTreeSet<String> = graph
        .nodes
        .iter()
        .filter(|n| !done.contains(&n.id))
        .map(|n| n.id.clone())
        .collect();
    Err(RunError::Cycle {
        nodes: nodes.into_iter().collect(),
    })
}

/// The [`RunError::Stalled`] for `unsettled` nodes: the edges into them whose source has not
/// settled, or — when every source has — all edges into them, because the tracker still counts
/// them as waiting (spec 23, R2).
fn stalled(
    graph: &Graph,
    records: &[NodeRecord],
    tracker: &Tracker,
    unsettled: Vec<String>,
) -> RunError {
    let settled =
        |id: &str| tracker.skipped.contains(id) || records.iter().any(|r| r.node_id == id);
    let mut unresolved_edges = BTreeSet::new();
    for node in &unsettled {
        let inbound: Vec<&crate::graph::Edge> =
            graph.edges.iter().filter(|e| &e.to == node).collect();
        let open: Vec<&&crate::graph::Edge> =
            inbound.iter().filter(|e| !settled(&e.from)).collect();
        if open.is_empty() {
            unresolved_edges.extend(inbound.iter().map(|e| format!("{} -> {}", e.from, e.to)));
        } else {
            unresolved_edges.extend(open.iter().map(|e| format!("{} -> {}", e.from, e.to)));
        }
    }
    RunError::Stalled {
        unsettled,
        unresolved_edges: unresolved_edges.into_iter().collect(),
    }
}

/// Sorted ids of the graph's nodes with no record that were not skipped either: halted tasks
/// plus nodes never spawned because scheduling stopped (R2). A skipped node is accounted for
/// in `RunReport::skipped`, not here.
fn nodes_without_record(
    graph: &Graph,
    records: &[NodeRecord],
    skipped: &BTreeSet<String>,
) -> Vec<String> {
    let recorded: BTreeSet<&str> = records.iter().map(|r| r.node_id.as_str()).collect();
    graph
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .filter(|id| !recorded.contains(id.as_str()) && !skipped.contains(id))
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Run one node: acquire its permits, check the halt flag, do the work,
/// release the permits. `writes` nodes additionally take the single write
/// permit, so two of them can never both be inside this function's work
/// section at once (R10).
async fn run_one(
    node: GraphNode,
    run_id: String,
    fanout: Arc<Semaphore>,
    write_lock: Arc<Semaphore>,
    halt: Arc<AtomicBool>,
    mcp: Arc<dyn McpCaller>,
    origin: Instant,
) -> Outcome {
    let spawned = Instant::now();
    let Ok(_fanout_permit) = fanout.acquire_owned().await else {
        return Outcome::Ran(
            failed_record(run_id, node.id, "internal: fan-out semaphore closed"),
            String::new(),
        );
    };
    let _write_permit = if node.writes {
        match write_lock.acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => {
                return Outcome::Ran(
                    failed_record(run_id, node.id, "internal: write-lock semaphore closed"),
                    String::new(),
                );
            }
        }
    } else {
        None
    };

    // After the permits, before the work: a failure that landed while this
    // task was queued must stop it from starting (R1).
    if halt.load(Ordering::SeqCst) {
        return Outcome::Halted;
    }

    // Both clocks are read at the moment the work is about to begin: the wait ends here.
    let queued_ms = elapsed_ms(spawned);
    let started_at_ms = elapsed_ms(origin);

    let allow_failure = node.allow_failure;
    let timeout = node.timeout;
    let ctx = CallContext {
        run_id: run_id.clone(),
        node_id: node.id.clone(),
    };
    let executed = match node.body {
        NodeBody::Shell { run } => execute_shell(&run, timeout).await,
        NodeBody::Mcp { tool, arguments } => run_mcp(mcp, tool, arguments, ctx, timeout).await,
        // A `route` node is resolved by `dispatch`, from its upstream's record; it is never
        // spawned. Reaching here is a scheduler bug, reported as data rather than a panic.
        NodeBody::Route { .. } => failed(
            "internal: a route node reached run_one".to_string(),
            0,
            false,
        ),
    };

    // Raised HERE, while this task still holds its permits: the next queued
    // sibling can acquire the instant they drop, which can be before `run`'s
    // loop has seen this record. Setting it in the loop alone would race (R1).
    if !executed.success && !allow_failure {
        halt.store(true, Ordering::SeqCst);
    }

    Outcome::Ran(
        NodeRecord {
            run_id,
            node_id: node.id,
            success: executed.success,
            exit_code: executed.exit_code,
            output: executed.output,
            error: executed.error,
            duration_ms: executed.duration_ms,
            timed_out: executed.timed_out,
            queued_ms,
            started_at_ms,
        },
        executed.stdout,
    )
}

/// Run an `mcp` node under its optional deadline (R1).
///
/// The caller is synchronous and cannot be interrupted from here: an expiry
/// reports the node as timed out and stops waiting, while the abandoned call
/// ends by the caller's own bound (R4 makes the real client kill its child).
async fn run_mcp(
    mcp: Arc<dyn McpCaller>,
    tool: String,
    arguments: serde_json::Value,
    ctx: CallContext,
    timeout: Option<Duration>,
) -> Executed {
    let start = Instant::now();
    let work =
        tokio::task::spawn_blocking(move || execute_mcp(mcp.as_ref(), &tool, &arguments, &ctx));
    let Some(limit) = timeout else {
        return join_executed(work).await;
    };
    match tokio::time::timeout(limit, join_executed(work)).await {
        Ok(executed) => executed,
        Err(_elapsed) => failed(
            format!("timed out after {}ms", limit.as_millis()),
            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            true,
        ),
    }
}

async fn join_executed(handle: JoinHandle<Executed>) -> Executed {
    match handle.await {
        Ok(executed) => executed,
        Err(e) => Executed {
            success: false,
            exit_code: None,
            output: String::new(),
            stdout: String::new(),
            error: Some(format!("node task failed: {e}")),
            duration_ms: 0,
            timed_out: false,
        },
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn failed_record(run_id: String, node_id: String, reason: &str) -> NodeRecord {
    NodeRecord {
        run_id,
        node_id,
        success: false,
        exit_code: None,
        output: String::new(),
        error: Some(reason.to_string()),
        duration_ms: 0,
        timed_out: false,
        queued_ms: 0,
        started_at_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use crate::graph::Edge;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- test fixtures ------------------------------------------------------

    fn shell(id: &str, run: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            body: NodeBody::Shell {
                run: run.to_string(),
            },
            writes: false,
            allow_failure: false,
            timeout: None,
        }
    }

    fn mcp(id: &str, tool: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            body: NodeBody::Mcp {
                tool: tool.to_string(),
                arguments: serde_json::json!({}),
            },
            writes: false,
            allow_failure: false,
            timeout: None,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            port: None,
        }
    }

    /// Refuses every call — for tests whose graph is shell-only, so an
    /// unexpected `mcp` call is a loud failure rather than silently ignored.
    struct NoopCaller;
    impl McpCaller for NoopCaller {
        fn call(
            &self,
            tool: &str,
            _arguments: &serde_json::Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            Err(format!(
                "unexpected mcp call to `{tool}` in a shell-only test"
            ))
        }
    }

    /// Signals start/finish around its own work via a shared in-flight
    /// counter, and records the PEAK it ever reached — the measurement the
    /// brief asks for, not merely "the run completed".
    struct CountingCaller {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        hold_ms: u64,
        fail_tools: Vec<String>,
    }

    impl McpCaller for CountingCaller {
        fn call(
            &self,
            tool: &str,
            _arguments: &serde_json::Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(self.hold_ms));
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if self.fail_tools.iter().any(|t| t == tool) {
                Err(format!("{tool} failed"))
            } else {
                Ok(format!("{tool} ok"))
            }
        }
    }

    /// Returns output that names the tool it was called with — so
    /// attribution (R13) can be checked against something other than the
    /// record's own `node_id` field.
    struct TaggingCaller;
    impl McpCaller for TaggingCaller {
        fn call(
            &self,
            tool: &str,
            _arguments: &serde_json::Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            std::thread::sleep(Duration::from_millis(20));
            Ok(format!("ran:{tool}"))
        }
    }

    /// Run `work` on its own thread and fail if it has not finished in 4s.
    /// `tokio::time::timeout` alone cannot do this: a synchronous loop that
    /// never yields starves the very runtime the timer lives on, so only a
    /// watchdog outside that thread turns such a hang into a failure (R5).
    fn within_4s<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(Duration::from_secs(4))
            .expect("the work hung past 4s")
    }

    /// Every scheduler test runs through this; the run is also wrapped in
    /// `tokio::time::timeout` for an async hang (R5).
    async fn run_bounded(graph: &Graph, opts: RunOptions) -> Result<RunReport, RunError> {
        let graph = graph.clone();
        tokio::task::spawn_blocking(move || {
            within_4s(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .expect("runtime")
                    .block_on(async {
                        tokio::time::timeout(Duration::from_secs(4), run(&graph, opts))
                            .await
                            .expect("the run hung past 4s")
                    })
            })
        })
        .await
        .expect("watchdog task")
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("laya-sched-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        dir
    }

    fn touch(id: &str, dir: &std::path::Path) -> GraphNode {
        shell(id, &format!("touch {}", dir.join(id).display()))
    }

    fn opts(run_id: &str, limit: usize) -> RunOptions {
        RunOptions {
            run_id: run_id.to_string(),
            limit,
            mcp: Arc::new(NoopCaller),
        }
    }

    // ---- R8: deterministic topo order ---------------------------------------

    #[test]
    fn the_topo_order_is_deterministic_across_runs() {
        let graph = Graph {
            nodes: vec![mcp("c", "t"), mcp("a", "t"), mcp("b", "t")],
            edges: vec![edge("a", "c"), edge("b", "c")],
        };
        let first = within_4s({
            let graph = graph.clone();
            move || topo_order(&graph)
        });
        for n in 0..50 {
            assert_eq!(
                within_4s({
                    let graph = graph.clone();
                    move || topo_order(&graph)
                }),
                first,
                "build {n} diverged from the first"
            );
        }
    }

    #[test]
    fn independent_nodes_are_ordered_by_id_not_by_map_order() {
        let graph = Graph {
            nodes: vec![mcp("z", "t"), mcp("a", "t")],
            edges: vec![],
        };
        let order = within_4s(move || topo_order(&graph));
        let pos_a = order.iter().position(|id| id == "a").expect("a present");
        let pos_z = order.iter().position(|id| id == "z").expect("z present");
        assert!(pos_a < pos_z, "`a` must precede `z`: {order:?}");
    }

    // ---- R9: bounded concurrent fan-out --------------------------------------

    /// Seeded LCG (Knuth MMIX constants). A failure reproduces from the printed seed.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or(0) % n.max(1)
        }
    }

    /// Records, per call, how many calls and how many WRITER calls were in flight, and
    /// sleeps 0-2 ms so overlap is real. The tool name is the node id.
    struct RecordingCaller {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        writers_in_flight: AtomicUsize,
        peak_writers: AtomicUsize,
        writers: Vec<String>,
        failing: Vec<String>,
        sleep_ms: Vec<u64>,
        ran: std::sync::Mutex<Vec<String>>,
    }

    impl McpCaller for RecordingCaller {
        fn call(
            &self,
            tool: &str,
            _arguments: &serde_json::Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            let is_writer = self.writers.iter().any(|w| w == tool);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            if is_writer {
                let w = self.writers_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak_writers.fetch_max(w, Ordering::SeqCst);
            }
            self.ran.lock().unwrap().push(tool.to_string());
            let idx: usize = tool.trim_start_matches('n').parse().unwrap_or(0);
            std::thread::sleep(Duration::from_millis(self.sleep_ms[idx]));
            if is_writer {
                self.writers_in_flight.fetch_sub(1, Ordering::SeqCst);
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if self.failing.iter().any(|f| f == tool) {
                Err(format!("{tool} failed"))
            } else {
                Ok(format!("{tool} ok"))
            }
        }
    }

    /// Model-based: random DAGs (n <= 12, edges only i -> j with i < j), random failing
    /// subset, `allow_failure`, `writes`, limit 1..=4. The invariants are checked against the
    /// graph and the options, never against the scheduler's own bookkeeping.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn scheduler_invariants_hold_for_random_dags() {
        const SEED: u64 = 0x135C_ED01;
        let mut rng = Lcg(SEED);
        let (mut saw_stop, mut saw_allowed_failure, mut saw_overlap, mut saw_writer) =
            (false, false, false, false);
        let (mut saw_multi_dep_ran, mut saw_full_ok) = (false, false);
        for case in 0..200_u32 {
            let n = 1 + rng.below(12);
            let id = |i: usize| format!("n{i}");
            let mut nodes: Vec<GraphNode> = (0..n).map(|i| mcp(&id(i), &id(i))).collect();
            let mut edges = Vec::new();
            for j in 1..n {
                for i in 0..j {
                    if rng.below(100) < 25 {
                        edges.push(edge(&id(i), &id(j)));
                    }
                }
            }
            // Roughly one case in three has no failures at all, so (iv) is really exercised.
            let any_fail = rng.below(3) != 0;
            let mut failing = Vec::new();
            for (i, node) in nodes.iter_mut().enumerate() {
                if any_fail && rng.below(100) < 20 {
                    failing.push(id(i));
                }
                node.allow_failure = rng.below(100) < 30;
                node.writes = rng.below(100) < 35;
            }
            let limit = 1 + rng.below(4);
            let caller = Arc::new(RecordingCaller {
                in_flight: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                writers_in_flight: AtomicUsize::new(0),
                peak_writers: AtomicUsize::new(0),
                writers: nodes
                    .iter()
                    .filter(|x| x.writes)
                    .map(|x| x.id.clone())
                    .collect(),
                failing: failing.clone(),
                sleep_ms: (0..n).map(|_| rng.below(3) as u64).collect(),
                ran: std::sync::Mutex::new(Vec::new()),
            });
            let graph = Graph { nodes, edges };
            let report = run_bounded(
                &graph,
                RunOptions {
                    run_id: format!("prop-{case}"),
                    limit,
                    mcp: Arc::clone(&caller) as Arc<dyn McpCaller>,
                },
            )
            .await
            .unwrap_or_else(|e| panic!("run errored: seed {SEED:#x}, case {case}: {e:?}"));

            let ctx = format!(
                "seed {SEED:#x}, case {case}, n={n}, limit={limit}, edges={:?}, failing={failing:?}",
                graph.edge_set()
            );
            let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
            let succeeded = |id: &str| report.records.iter().any(|r| r.node_id == id && r.success);
            // (i) a node ran => every dependency ran, and succeeded or was allowed to fail.
            for e in &graph.edges {
                if ran.contains(&e.to.as_str()) {
                    assert!(
                        ran.contains(&e.from.as_str()),
                        "(i) {} ran without {}: {ctx}",
                        e.to,
                        e.from
                    );
                    let dep_allowed = graph.node(&e.from).is_some_and(|d| d.allow_failure);
                    assert!(
                        succeeded(&e.from) || dep_allowed,
                        "(i) {} ran after {} failed without allow_failure: {ctx}",
                        e.to,
                        e.from
                    );
                }
            }
            // (ii) / (iii) peak overlap, as measured by the callee.
            let peak = caller.peak.load(Ordering::SeqCst);
            let peak_w = caller.peak_writers.load(Ordering::SeqCst);
            assert!(peak <= limit, "(ii) peak {peak} > limit: {ctx}");
            assert!(peak_w <= 1, "(iii) {peak_w} writers overlapped: {ctx}");
            // A node never runs twice.
            let mut uniq = ran.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), ran.len(), "a node ran twice: {ctx}");
            // (iv) no failures => all n ran.
            if failing.is_empty() {
                assert_eq!(
                    ran.len(),
                    n,
                    "(iv) a failure-free run did not run everything: {ctx}"
                );
                assert!(!report.stopped_early, "(iv) {ctx}");
                saw_full_ok = true;
            }
            // (v) stopped_early => records + not_started == n. (Also true when it did not stop.)
            assert_eq!(
                report.records.len() + report.not_started.len(),
                n,
                "(v) {ctx}"
            );
            if report.stopped_early {
                saw_stop = true;
            } else {
                assert!(
                    report.not_started.is_empty(),
                    "(v) a complete run left nodes: {ctx}"
                );
            }
            saw_allowed_failure |= report
                .records
                .iter()
                .any(|r| !r.success && graph.node(&r.node_id).is_some_and(|x| x.allow_failure));
            saw_overlap |= peak >= 2;
            saw_writer |= peak_w == 1;
            saw_multi_dep_ran |= graph
                .edges
                .iter()
                .any(|e| ran.contains(&e.to.as_str()) && ran.contains(&e.from.as_str()));
        }
        // CONTROL: the generator reached every regime the invariants are about, so a vacuous
        // pass (nothing ever failed, nothing ever overlapped) is impossible.
        assert!(
            saw_stop
                && saw_allowed_failure
                && saw_overlap
                && saw_writer
                && saw_multi_dep_ran
                && saw_full_ok,
            "generator coverage: stop={saw_stop} allowed_failure={saw_allowed_failure} overlap={saw_overlap} writer={saw_writer} dep={saw_multi_dep_ran} ok={saw_full_ok}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn a_fan_out_runs_concurrently_up_to_the_limit() {
        let peak = Arc::new(AtomicUsize::new(0));
        let caller = Arc::new(CountingCaller {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::clone(&peak),
            hold_ms: 150,
            fail_tools: vec![],
        });
        let nodes: Vec<GraphNode> = (0..8)
            .map(|i| mcp(&format!("n{i}"), &format!("t{i}")))
            .collect();
        let graph = Graph {
            nodes,
            edges: vec![],
        };

        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r9-fanout".to_string(),
                limit: 4,
                mcp: caller,
            },
        )
        .await
        .expect("run must not error");

        assert!(!report.stopped_early);
        assert_eq!(report.records.len(), 8);
        let observed = peak.load(Ordering::SeqCst);
        println!("R9 observed max overlap (8 nodes, limit 4) = {observed}");
        assert!(
            observed <= 4,
            "must never exceed the limit: observed {observed}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn the_control_a_serial_limit_runs_one_at_a_time() {
        let peak = Arc::new(AtomicUsize::new(0));
        let caller = Arc::new(CountingCaller {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::clone(&peak),
            hold_ms: 60,
            fail_tools: vec![],
        });
        let nodes: Vec<GraphNode> = (0..8)
            .map(|i| mcp(&format!("n{i}"), &format!("t{i}")))
            .collect();
        let graph = Graph {
            nodes,
            edges: vec![],
        };

        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r9-serial".to_string(),
                limit: 1,
                mcp: caller,
            },
        )
        .await
        .expect("run must not error");

        assert!(!report.stopped_early);
        let observed = peak.load(Ordering::SeqCst);
        println!("R9 control observed max overlap (limit 1) = {observed}");
        assert_eq!(
            observed, 1,
            "a limit of 1 must never exceed 1 in flight: observed {observed}"
        );
    }

    #[tokio::test]
    async fn the_effective_worker_count_is_reported() {
        let caller = Arc::new(NoopCaller);
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0")],
            edges: vec![],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r9-report".to_string(),
                limit: 4,
                mcp: caller,
            },
        )
        .await
        .expect("run must not error");
        assert_eq!(report.worker_limit, 4);
    }

    // ---- R10: writers are mutually exclusive ---------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_writing_nodes_do_not_overlap() {
        let peak = Arc::new(AtomicUsize::new(0));
        let caller = Arc::new(CountingCaller {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::clone(&peak),
            hold_ms: 100,
            fail_tools: vec![],
        });
        let mut w1 = mcp("w1", "t1");
        w1.writes = true;
        let mut w2 = mcp("w2", "t2");
        w2.writes = true;
        let graph = Graph {
            nodes: vec![w1, w2],
            edges: vec![],
        };

        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r10-writers".to_string(),
                limit: 4,
                mcp: caller,
            },
        )
        .await
        .expect("run must not error");

        assert!(!report.stopped_early);
        let observed = peak.load(Ordering::SeqCst);
        println!("R10 observed max overlap (two writers) = {observed}");
        assert_eq!(
            observed, 1,
            "two writers must never overlap: observed {observed}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_control_two_read_only_nodes_may_overlap() {
        let peak = Arc::new(AtomicUsize::new(0));
        let caller = Arc::new(CountingCaller {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::clone(&peak),
            hold_ms: 100,
            fail_tools: vec![],
        });
        let graph = Graph {
            nodes: vec![mcp("r1n", "t1"), mcp("r2n", "t2")],
            edges: vec![],
        };

        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r10-readers".to_string(),
                limit: 4,
                mcp: caller,
            },
        )
        .await
        .expect("run must not error");

        assert!(!report.stopped_early);
        let observed = peak.load(Ordering::SeqCst);
        println!("R10 control observed overlap (two readers) = {observed}");
        assert!(
            observed > 1,
            "read-only nodes must be able to overlap, or R10 is satisfied by serialising \
             everything: observed {observed}"
        );
    }

    // ---- R11: fail fast -------------------------------------------------------

    #[tokio::test]
    async fn a_failure_stops_the_run_and_reports_stopped_early() {
        let graph = Graph {
            nodes: vec![
                shell("n1", "exit 0"),
                shell("n2", "exit 1"),
                shell("n3", "exit 0"),
            ],
            edges: vec![edge("n1", "n2"), edge("n2", "n3")],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r11-fail".to_string(),
                limit: 4,
                mcp: Arc::new(NoopCaller),
            },
        )
        .await
        .expect("run must not error");

        assert!(report.stopped_early, "a failing node must stop the run");
        assert_eq!(report.failed_node.as_deref(), Some("n2"));
        assert!(
            !report.records.iter().any(|r| r.node_id == "n3"),
            "n3 depends on the failed node and must never have started: {:?}",
            report.records
        );
    }

    #[tokio::test]
    async fn allow_failure_lets_the_run_continue() {
        let mut n2 = shell("n2", "exit 1");
        n2.allow_failure = true;
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0"), n2, shell("n3", "exit 0")],
            edges: vec![edge("n1", "n2"), edge("n2", "n3")],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r11-allow".to_string(),
                limit: 4,
                mcp: Arc::new(NoopCaller),
            },
        )
        .await
        .expect("run must not error");

        assert!(
            !report.stopped_early,
            "allow_failure must let the run complete"
        );
        let n2_record = report
            .records
            .iter()
            .find(|r| r.node_id == "n2")
            .expect("n2 ran");
        assert!(!n2_record.success, "n2's record must still say it failed");
        assert!(
            report.records.iter().any(|r| r.node_id == "n3"),
            "n3 must still run once n2 is allowed to fail"
        );
    }

    #[tokio::test]
    async fn the_control_a_completed_run_does_not_say_stopped_early() {
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0"), shell("n2", "exit 0")],
            edges: vec![edge("n1", "n2")],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r11-control".to_string(),
                limit: 4,
                mcp: Arc::new(NoopCaller),
            },
        )
        .await
        .expect("run must not error");
        assert!(!report.stopped_early);
        assert!(report.failed_node.is_none());
    }

    // ---- R13: explicit attribution --------------------------------------------

    #[tokio::test]
    async fn every_record_carries_run_id_and_node_id() {
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0"), shell("n2", "exit 0")],
            edges: vec![edge("n1", "n2")],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r13-basic".to_string(),
                limit: 2,
                mcp: Arc::new(NoopCaller),
            },
        )
        .await
        .expect("run must not error");

        assert_eq!(report.records.len(), 2);
        for record in &report.records {
            assert_eq!(record.run_id, "r13-basic");
            assert!(!record.node_id.is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_concurrent_run_attribution_is_still_correct() {
        let nodes: Vec<GraphNode> = (0..4)
            .map(|i| mcp(&format!("n{i}"), &format!("tool{i}")))
            .collect();
        let graph = Graph {
            nodes,
            edges: vec![],
        };
        let report = run_bounded(
            &graph,
            RunOptions {
                run_id: "r13-concurrent".to_string(),
                limit: 4,
                mcp: Arc::new(TaggingCaller),
            },
        )
        .await
        .expect("run must not error");

        assert_eq!(report.records.len(), 4);
        for i in 0..4 {
            let node_id = format!("n{i}");
            let record = report
                .records
                .iter()
                .find(|r| r.node_id == node_id)
                .unwrap_or_else(|| panic!("{node_id} missing from records"));
            assert_eq!(
                record.output,
                format!("ran:tool{i}"),
                "record for {node_id} must carry its OWN output, not another node's"
            );
        }
    }

    // ---- spec 09: halt flag, accounting, cycles -----------------------------

    fn fail_slowly() -> GraphNode {
        shell("a", "sleep 0.2; exit 1")
    }

    #[tokio::test]
    async fn a_failure_stops_queued_siblings_from_starting() {
        let dir = tmp_dir("halt");
        let graph = Graph {
            nodes: vec![
                fail_slowly(),
                touch("b", &dir),
                touch("c", &dir),
                touch("d", &dir),
            ],
            edges: vec![],
        };
        let report = run_bounded(&graph, opts("s9-halt", 1)).await.expect("run");
        assert!(report.stopped_early);
        for id in ["b", "c", "d"] {
            assert!(
                !dir.join(id).exists(),
                "`{id}` ran after the run had failed"
            );
        }
        assert_eq!(report.not_started, vec!["b", "c", "d"]);
        assert_eq!(report.records.len(), 1, "{:?}", report.records);
    }

    #[tokio::test]
    async fn the_control_allow_failure_does_not_halt_siblings() {
        let dir = tmp_dir("nohalt");
        let mut a = fail_slowly();
        a.allow_failure = true;
        let graph = Graph {
            nodes: vec![a, touch("b", &dir), touch("c", &dir), touch("d", &dir)],
            edges: vec![],
        };
        let report = run_bounded(&graph, opts("s9-nohalt", 1))
            .await
            .expect("run");
        assert!(!report.stopped_early);
        for id in ["b", "c", "d"] {
            assert!(dir.join(id).exists(), "`{id}` should have run");
        }
        assert!(report.not_started.is_empty());
    }

    #[tokio::test]
    async fn every_node_is_either_recorded_or_not_started() {
        // a -> b -> d, a -> c -> d, plus independent e, f. `b` fails.
        let graph = Graph {
            nodes: vec![
                shell("a", "exit 0"),
                shell("b", "exit 1"),
                shell("c", "sleep 0.1; exit 0"),
                shell("d", "exit 0"),
                shell("e", "exit 0"),
                shell("f", "exit 0"),
            ],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        };
        let report = run_bounded(&graph, opts("s9-acct", 2)).await.expect("run");
        assert!(report.stopped_early);
        assert_eq!(report.records.len() + report.not_started.len(), 6);
        assert!(
            report.not_started.contains(&"d".to_string()),
            "{:?}",
            report.not_started
        );
        let mut sorted = report.not_started.clone();
        sorted.sort();
        assert_eq!(sorted, report.not_started, "not_started must be sorted");
        for id in &report.not_started {
            assert!(
                !report.records.iter().any(|r| &r.node_id == id),
                "`{id}` is both recorded and not started"
            );
        }
    }

    #[tokio::test]
    async fn a_cyclic_graph_is_refused_not_partially_run() {
        let dir = tmp_dir("cycle");
        let graph = Graph {
            nodes: vec![touch("p", &dir), touch("q", &dir), touch("z", &dir)],
            edges: vec![edge("p", "q"), edge("q", "p")],
        };
        let result = run_bounded(&graph, opts("s9-cycle", 2)).await;
        assert_eq!(
            result,
            Err(RunError::Cycle {
                nodes: vec!["p".to_string(), "q".to_string()]
            })
        );
        assert!(
            !dir.join("z").exists(),
            "`z` ran although the graph was refused"
        );
    }

    #[tokio::test]
    async fn the_control_a_diamond_is_not_a_cycle() {
        let dir = tmp_dir("diamond");
        let log = dir.join("order");
        let step = |id: &str| shell(id, &format!("echo {id} >> {}", log.display()));
        let graph = Graph {
            nodes: vec![step("a"), step("b"), step("c"), step("d")],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        };
        let order = within_4s({
            let graph = graph.clone();
            move || topo_order(&graph)
        });
        assert_eq!(order.len(), 4);
        assert_eq!(order.first().map(String::as_str), Some("a"));
        assert_eq!(order.last().map(String::as_str), Some("d"));

        let report = run_bounded(&graph, opts("s9-diamond", 4))
            .await
            .expect("run");
        assert!(!report.stopped_early);
        let lines = std::fs::read_to_string(&log).expect("order log");
        let ran: Vec<&str> = lines.lines().collect();
        assert_eq!(ran.iter().filter(|l| **l == "d").count(), 1, "{ran:?}");
        assert_eq!(ran.last(), Some(&"d"), "d must run after b and c: {ran:?}");
        assert_eq!(ran.first(), Some(&"a"), "{ran:?}");
    }

    // ---- limit validation -------------------------------------------------------

    #[tokio::test]
    async fn a_zero_limit_is_refused_rather_than_deadlocking() {
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0")],
            edges: vec![],
        };
        let result = run_bounded(
            &graph,
            RunOptions {
                run_id: "r-zero".to_string(),
                limit: 0,
                mcp: Arc::new(NoopCaller),
            },
        )
        .await;
        assert_eq!(result, Err(RunError::BadLimit { limit: 0 }));
    }

    // ---- spec 10: deadlines that fire -----------------------------------------

    fn with_timeout(mut node: GraphNode, secs: f64) -> GraphNode {
        node.timeout = Some(Duration::from_secs_f64(secs));
        node
    }

    async fn run_one_node(node: GraphNode, run_id: &str) -> RunReport {
        let graph = Graph {
            nodes: vec![node],
            edges: vec![],
        };
        run_bounded(&graph, opts(run_id, 2))
            .await
            .expect("run must not error")
    }

    /// Whether `pid` is no longer a running process. A killed process that
    /// nobody has reaped is a zombie: `kill -0` still succeeds on it, but it
    /// is not running, so a zombie counts as gone.
    fn process_is_gone(pid: &str) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
            stat.rsplit(") ")
                .next()
                .is_some_and(|rest| rest.starts_with('Z'))
        })
    }

    fn read_pid(file: &std::path::Path) -> String {
        std::fs::read_to_string(file)
            .expect("the node wrote its grandchild's pid")
            .trim()
            .to_string()
    }

    #[tokio::test]
    async fn a_node_past_its_timeout_is_killed_and_reported() {
        let report = run_one_node(with_timeout(shell("n1", "sleep 30"), 1.0), "r-to").await;
        let record = &report.records[0];
        assert!(!record.success, "{record:?}");
        assert!(record.timed_out, "{record:?}");
        assert!(
            (900..3000).contains(&record.duration_ms),
            "duration_ms must be about the deadline, got {}",
            record.duration_ms
        );
        assert!(report.stopped_early, "an expired node is a failure");
        assert_eq!(report.failed_node.as_deref(), Some("n1"));
    }

    #[tokio::test]
    async fn the_control_a_node_within_its_timeout_succeeds() {
        let report = run_one_node(with_timeout(shell("n1", "sleep 0.1"), 5.0), "r-in").await;
        let record = &report.records[0];
        assert!(record.success, "{record:?}");
        assert!(!record.timed_out, "{record:?}");
        assert!(!report.stopped_early);
    }

    #[tokio::test]
    async fn the_control_a_node_without_a_timeout_is_unbounded_as_before() {
        let report = run_one_node(shell("n1", "sleep 1.5"), "r-none").await;
        let record = &report.records[0];
        assert!(record.success, "{record:?}");
        assert!(!record.timed_out, "{record:?}");
        assert!(record.duration_ms >= 1400, "{record:?}");
    }

    #[tokio::test]
    async fn a_timeout_leaves_no_orphan() {
        let dir = tmp_dir("orphan");
        let pid_file = dir.join("pid-killed");
        let node = with_timeout(
            shell(
                "n1",
                &format!("sleep 600 & echo $! > {}; wait", pid_file.display()),
            ),
            1.0,
        );
        let report = run_one_node(node, "r-orphan").await;
        assert!(report.records[0].timed_out, "{:?}", report.records[0]);

        let pid = read_pid(&pid_file);
        // The group signal is sent before the record returns, but delivery
        // is the kernel's: allow it a moment, never an unbounded wait.
        let mut gone = false;
        for _ in 0..20 {
            if process_is_gone(&pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(gone, "grandchild {pid} outlived the timeout");
    }

    #[tokio::test]
    async fn the_control_a_node_that_finishes_in_time_is_not_group_killed() {
        // Proves `process_is_gone` can answer "alive", and that nothing is
        // signalled when the deadline is not reached: a backgrounded child of
        // a node that completed in time must still be running afterwards.
        let dir = tmp_dir("not-killed");
        let pid_file = dir.join("pid-kept");
        let node = with_timeout(
            shell(
                "n1",
                &format!(
                    "sleep 3 >/dev/null 2>&1 & echo $! > {}; exit 0",
                    pid_file.display()
                ),
            ),
            5.0,
        );
        let report = run_one_node(node, "r-kept").await;
        assert!(report.records[0].success, "{:?}", report.records[0]);
        let pid = read_pid(&pid_file);
        assert!(
            !process_is_gone(&pid),
            "an in-time node's child must not have been killed"
        );
    }

    #[tokio::test]
    async fn a_flow_timeout_is_inherited() {
        let flow = crate::flow::Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: vec![],
            timeout: Some(1),
            nodes: vec![crate::flow::Node {
                run: Some("sleep 30".to_string()),
                ..crate::flow::Node::new("n1", crate::flow::Kind::Shell)
            }],
        };
        let graph = crate::lower::lower(&flow).expect("lower");
        let report = run_bounded(&graph, opts("r-inherit", 2))
            .await
            .expect("run");
        assert!(report.records[0].timed_out, "{:?}", report.records[0]);
    }

    #[tokio::test]
    async fn the_control_a_node_timeout_overrides_the_flow_timeout() {
        // Near miss for inheritance: the node declares its own, longer
        // deadline, so the flow's 1 s must NOT apply to it.
        let flow = crate::flow::Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: vec![],
            timeout: Some(1),
            nodes: vec![crate::flow::Node {
                run: Some("sleep 1.5".to_string()),
                timeout: Some(3),
                ..crate::flow::Node::new("n1", crate::flow::Kind::Shell)
            }],
        };
        let graph = crate::lower::lower(&flow).expect("lower");
        let report = run_bounded(&graph, opts("r-override", 2))
            .await
            .expect("run");
        let record = &report.records[0];
        assert!(record.success && !record.timed_out, "{record:?}");
    }

    #[tokio::test]
    async fn a_timed_out_node_halts_queued_siblings_but_allow_failure_does_not() {
        // The stop flag and the deadline interact: an expiry is a failure, so
        // a queued sibling must not start; with `allow_failure` it must.
        let dir = tmp_dir("timeout-halt");
        for (allow, label) in [(false, "strict"), (true, "lenient")] {
            let mut slow = with_timeout(shell("n1", "sleep 30"), 0.5);
            slow.allow_failure = allow;
            let marker = dir.join(label);
            let graph = Graph {
                nodes: vec![slow, shell("n2", &format!("touch {}", marker.display()))],
                edges: vec![],
            };
            let report = run_bounded(&graph, opts(label, 1)).await.expect("run");
            assert_eq!(
                marker.exists(),
                allow,
                "{label}: sibling ran = {}, expected {allow}; {report:?}",
                marker.exists()
            );
            assert_eq!(report.stopped_early, !allow, "{label}: {report:?}");
        }
    }

    struct SlowCaller(u64);
    impl McpCaller for SlowCaller {
        fn call(
            &self,
            _tool: &str,
            _arguments: &serde_json::Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            std::thread::sleep(Duration::from_millis(self.0));
            Ok("done".to_string())
        }
    }

    #[tokio::test]
    async fn an_mcp_node_past_its_timeout_is_reported_timed_out() {
        let graph = Graph {
            nodes: vec![with_timeout(mcp("n1", "slow"), 0.5)],
            edges: vec![],
        };
        let mut o = opts("r-mcp-to", 2);
        o.mcp = Arc::new(SlowCaller(1500));
        let report = run_bounded(&graph, o).await.expect("run");
        let record = &report.records[0];
        assert!(!record.success && record.timed_out, "{record:?}");
    }

    #[tokio::test]
    async fn the_control_an_mcp_node_within_its_timeout_succeeds() {
        let graph = Graph {
            nodes: vec![with_timeout(mcp("n1", "quick"), 5.0)],
            edges: vec![],
        };
        let mut o = opts("r-mcp-in", 2);
        o.mcp = Arc::new(SlowCaller(50));
        let report = run_bounded(&graph, o).await.expect("run");
        let record = &report.records[0];
        assert!(record.success && !record.timed_out, "{record:?}");
    }

    // ---- spec 11: the timeline and the run context --------------------------------

    #[tokio::test]
    async fn a_record_separates_queue_wait_from_execution() {
        // R5. With `limit: 1` the second node cannot start until the first releases its permit,
        // so it WAITS ~300 ms and then WORKS ~300 ms. `duration_ms` alone cannot tell a starved
        // run from a slow one: it starts only after the permit is held.
        let graph = Graph {
            nodes: vec![shell("a", "sleep 0.3"), shell("b", "sleep 0.3")],
            edges: vec![],
        };
        let report = run_bounded(&graph, opts("r5-limit1", 1))
            .await
            .expect("run");
        let a = report.records.iter().find(|r| r.node_id == "a").expect("a");
        let b = report.records.iter().find(|r| r.node_id == "b").expect("b");
        assert!(a.queued_ms < 100, "the first node never waits: {a:?}");
        assert!(b.queued_ms >= 250, "the second node waited for a: {b:?}");
        assert!(b.duration_ms >= 250, "and then did its own work: {b:?}");
        assert!(
            b.started_at_ms >= 250,
            "its work began after a's finished: {b:?}"
        );
        assert!(
            report.wall_ms >= 550,
            "wall_ms spans the whole run: {report:?}"
        );
    }

    #[tokio::test]
    async fn the_control_a_run_with_room_does_not_queue() {
        // The near-miss: `limit: 2` runs both at once, so NEITHER waits. Without this a
        // `queued_ms` that always reported the node's own duration would pass the test above.
        let graph = Graph {
            nodes: vec![shell("a", "sleep 0.3"), shell("b", "sleep 0.3")],
            edges: vec![],
        };
        let report = run_bounded(&graph, opts("r5-limit2", 2))
            .await
            .expect("run");
        for r in &report.records {
            assert!(r.queued_ms < 100, "no node waits when there is room: {r:?}");
            assert!(r.started_at_ms < 100, "both start at once: {r:?}");
            assert!(r.duration_ms >= 250, "{r:?}");
        }
    }

    struct ContextCaller(std::sync::Mutex<Vec<(String, String)>>);
    impl McpCaller for ContextCaller {
        fn call(
            &self,
            _tool: &str,
            _arguments: &serde_json::Value,
            ctx: &CallContext,
        ) -> Result<String, String> {
            self.0
                .lock()
                .expect("lock")
                .push((ctx.run_id.clone(), ctx.node_id.clone()));
            Ok("ok".to_string())
        }
    }

    #[tokio::test]
    async fn an_mcp_node_hands_its_run_and_node_id_to_the_caller() {
        // R1 at the scheduler seam: the ids must come from the run, not be invented downstream.
        let graph = Graph {
            nodes: vec![mcp("classify", "decide")],
            edges: vec![],
        };
        let caller = Arc::new(ContextCaller(std::sync::Mutex::new(Vec::new())));
        let mut o = opts("run-ctx", 2);
        o.mcp = Arc::clone(&caller) as Arc<dyn McpCaller>;
        let _ = run_bounded(&graph, o).await.expect("run");
        let seen = caller.0.lock().expect("lock").clone();
        assert_eq!(seen, vec![("run-ctx".to_string(), "classify".to_string())]);
    }

    // ---- spec 23: the scheduler cannot wait forever -----------------------------------

    fn two_node_chain() -> Graph {
        Graph {
            nodes: vec![shell("a", "true"), shell("b", "true")],
            edges: vec![edge("a", "b")],
        }
    }

    /// [`run_tracked`] with a deliberately hand-built tracker, under the same 4s watchdog.
    async fn run_tracked_bounded(
        graph: &Graph,
        tracker: Tracker,
        run_id: &str,
    ) -> Result<RunReport, RunError> {
        let graph = graph.clone();
        let o = opts(run_id, 4);
        tokio::task::spawn_blocking(move || {
            within_4s(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("runtime")
                    .block_on(async {
                        tokio::time::timeout(
                            Duration::from_secs(4),
                            run_tracked(&graph, o, Instant::now(), tracker),
                        )
                        .await
                        .expect("the run hung past 4s")
                    })
            })
        })
        .await
        .expect("watchdog task")
    }

    #[test]
    fn an_inconsistent_dependency_table_is_refused_by_name() {
        // The shape the two hanging mutants return: one phantom key, no real node id.
        let graph = two_node_chain();
        let bogus: HashMap<String, usize> = HashMap::from_iter([(String::new(), 1)]);
        assert_eq!(
            verify_tables(&graph, &bogus),
            Err(RunError::InconsistentTable {
                missing: vec!["a".to_string(), "b".to_string()]
            })
        );
        // Near-miss: only ONE id missing names exactly that one.
        let partial: HashMap<String, usize> = HashMap::from_iter([("a".to_string(), 0)]);
        assert_eq!(
            verify_tables(&graph, &partial),
            Err(RunError::InconsistentTable {
                missing: vec!["b".to_string()]
            })
        );
    }

    #[test]
    fn the_control_a_consistent_table_is_accepted() {
        let graph = two_node_chain();
        let (indegree, _) = dependency_tables(&graph);
        assert_eq!(verify_tables(&graph, &indegree), Ok(()));
        // And the real entry points accept the real graph.
        assert_eq!(refuse_cycle(&graph), Ok(()));
        assert_eq!(topo_order(&graph), vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn a_scheduler_that_cannot_settle_a_node_reports_which_one() {
        let graph = two_node_chain();
        let mut tracker = Tracker::new(&graph);
        // `b` waits on one more plain edge than the graph has: nothing can ever resolve it.
        tracker.state.get_mut("b").expect("b").and_left += 1;
        let result = run_tracked_bounded(&graph, tracker, "stall").await;
        assert_eq!(
            result.map(|_| ()),
            Err(RunError::Stalled {
                unsettled: vec!["b".to_string()],
                unresolved_edges: vec!["a -> b".to_string()],
            })
        );
    }

    #[tokio::test]
    async fn a_stall_names_only_the_edges_that_never_resolved() {
        // `d` has two inbound edges: `a -> d` resolved (a ran), `x -> d` never did (x is stuck).
        // The report must name the open one and not the resolved one.
        let graph = Graph {
            nodes: vec![
                shell("a", "true"),
                shell("p", "true"),
                shell("x", "true"),
                shell("d", "true"),
            ],
            edges: vec![edge("p", "x"), edge("x", "d"), edge("a", "d")],
        };
        let mut tracker = Tracker::new(&graph);
        tracker.state.get_mut("x").expect("x").and_left += 1;
        let result = run_tracked_bounded(&graph, tracker, "stall-edges").await;
        assert_eq!(
            result.map(|_| ()),
            Err(RunError::Stalled {
                unsettled: vec!["d".to_string(), "x".to_string()],
                unresolved_edges: vec!["p -> x".to_string(), "x -> d".to_string()],
            })
        );
    }

    #[tokio::test]
    async fn the_control_a_satisfiable_graph_completes() {
        // Slow-but-valid: a chain with a sleeping middle node and a fan-in. The bound must not
        // turn a slow run into a failure.
        let graph = Graph {
            nodes: vec![
                shell("a", "true"),
                shell("b", "sleep 0.5"),
                shell("c", "true"),
                shell("d", "true"),
            ],
            edges: vec![edge("a", "b"), edge("b", "d"), edge("c", "d")],
        };
        let report = run_tracked_bounded(&graph, Tracker::new(&graph), "slow-ok")
            .await
            .expect("a satisfiable graph must complete");
        assert!(!report.stopped_early);
        assert_eq!(report.records.len(), 4);
        assert!(report.not_started.is_empty());
    }

    #[tokio::test]
    async fn no_node_is_settled_twice() {
        let graph = Graph {
            nodes: vec![shell("a", "true"), shell("b", "true"), shell("c", "true")],
            edges: vec![edge("a", "c"), edge("b", "c")],
        };
        let report = run_tracked_bounded(&graph, Tracker::new(&graph), "fan-in")
            .await
            .expect("run");
        let mut ids: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["a", "b", "c"], "each node settles exactly once");
    }

    #[tokio::test]
    async fn a_node_made_ready_twice_is_refused_by_name() {
        let graph = two_node_chain();
        let mut tracker = Tracker::new(&graph);
        // `b` is wrongly ready from the start, so it runs alongside `a` and is readied AGAIN
        // when `a` resolves.
        tracker.state.get_mut("b").expect("b").and_left = 0;
        tracker.ready.insert("b".to_string());
        let result = run_tracked_bounded(&graph, tracker, "twice").await;
        assert_eq!(
            result.map(|_| ()),
            Err(RunError::SettledTwice {
                node_id: "b".to_string()
            })
        );
    }
}
