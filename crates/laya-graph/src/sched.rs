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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};

use crate::error::RunError;
use crate::graph::{Graph, GraphNode, NodeBody};
use crate::node::{Executed, McpCaller, execute_mcp, execute_shell};
use crate::run::{NodeRecord, RunReport};

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
    let (mut indegree, successors) = dependency_tables(graph);
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
    order
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
/// # Errors
/// [`RunError::BadLimit`] if `opts.limit` is zero. [`RunError::UnknownNode`]
/// if an edge names a node the graph does not declare. [`RunError::NodeTaskFailed`]
/// if a spawned node's own scheduling task could not be joined — an executor
/// bug, never a node's own reported failure (that lives in the returned
/// `RunReport`, not here; see R11, R12).
pub async fn run(graph: &Graph, opts: RunOptions) -> Result<RunReport, RunError> {
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

    let (mut indegree, successors) = dependency_tables(graph);
    let mut ready: BTreeSet<String> = indegree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let fanout = Arc::new(Semaphore::new(opts.limit));
    let write_lock = Arc::new(Semaphore::new(1));
    let mut joins: JoinSet<NodeRecord> = JoinSet::new();
    let mut records = Vec::new();
    let mut stopped_early = false;
    let mut failed_node: Option<String> = None;
    let mut stop_scheduling = false;

    spawn_ready(graph, &mut ready, &mut joins, &opts, &fanout, &write_lock);

    while let Some(outcome) = joins.join_next().await {
        let record = match outcome {
            Ok(record) => record,
            Err(e) => {
                return Err(RunError::NodeTaskFailed {
                    reason: e.to_string(),
                });
            }
        };

        let allow_failure = graph.node(&record.node_id).is_some_and(|n| n.allow_failure);
        let failed = !record.success;

        if failed && !allow_failure {
            // Fail fast (R11): stop admitting new work. Already-spawned
            // tasks are in-flight and are not cancelled (N4/spec: "no
            // cancellation of in-flight work"); only nodes not yet dispatched
            // are affected, because `stop_scheduling` gates `spawn_ready`
            // below and the failed node's own successors are never unlocked.
            stopped_early = true;
            if failed_node.is_none() {
                failed_node = Some(record.node_id.clone());
            }
            stop_scheduling = true;
        } else if !stop_scheduling {
            if let Some(succs) = successors.get(&record.node_id) {
                for succ in succs {
                    if let Some(deg) = indegree.get_mut(succ) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            ready.insert(succ.clone());
                        }
                    }
                }
            }
            spawn_ready(graph, &mut ready, &mut joins, &opts, &fanout, &write_lock);
        }

        records.push(record);
    }

    Ok(RunReport {
        run_id: opts.run_id,
        stopped_early,
        failed_node,
        worker_limit: opts.limit,
        records,
    })
}

/// Spawn every currently-ready node, in id order (R8) — the dispatch ORDER is
/// deterministic. Actual concurrency is bounded inside [`run_one`] by the
/// shared semaphores, not by how many tasks are spawned here, so spawning
/// every ready node up front does not itself create unbounded concurrency.
fn spawn_ready(
    graph: &Graph,
    ready: &mut BTreeSet<String>,
    joins: &mut JoinSet<NodeRecord>,
    opts: &RunOptions,
    fanout: &Arc<Semaphore>,
    write_lock: &Arc<Semaphore>,
) {
    while let Some(id) = pop_first(ready) {
        // Unreachable once `run` has validated every edge's endpoints exist,
        // kept so a future caller of this private helper fails safe rather
        // than panicking.
        let Some(node) = graph.node(&id).cloned() else {
            continue;
        };
        let run_id = opts.run_id.clone();
        let fanout = Arc::clone(fanout);
        let write_lock = Arc::clone(write_lock);
        let mcp = Arc::clone(&opts.mcp);
        joins.spawn(run_one(node, run_id, fanout, write_lock, mcp));
    }
}

/// Run one node: acquire its permits, do the work, release them. `writes`
/// nodes additionally take the single write permit, so two of them can never
/// both be inside this function's work section at once (R10).
async fn run_one(
    node: GraphNode,
    run_id: String,
    fanout: Arc<Semaphore>,
    write_lock: Arc<Semaphore>,
    mcp: Arc<dyn McpCaller>,
) -> NodeRecord {
    let Ok(_fanout_permit) = fanout.acquire_owned().await else {
        return failed_record(run_id, node.id, "internal: fan-out semaphore closed");
    };
    let _write_permit = if node.writes {
        match write_lock.acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => {
                return failed_record(run_id, node.id, "internal: write-lock semaphore closed");
            }
        }
    } else {
        None
    };

    let executed = match node.body {
        NodeBody::Shell { run } => {
            join_executed(tokio::task::spawn_blocking(move || execute_shell(&run))).await
        }
        NodeBody::Mcp { tool, arguments } => {
            join_executed(tokio::task::spawn_blocking(move || {
                execute_mcp(mcp.as_ref(), &tool, &arguments)
            }))
            .await
        }
        // A lowered `route` node picks a port; it has no work of its own to
        // run here. Threading a payload through it is the CLI's concern
        // (a later part) — out of scope for R12, which covers `shell`/`mcp`.
        NodeBody::Route { .. } => Executed {
            success: true,
            exit_code: None,
            output: String::new(),
            error: None,
            duration_ms: 0,
        },
    };

    NodeRecord {
        run_id,
        node_id: node.id,
        success: executed.success,
        exit_code: executed.exit_code,
        output: executed.output,
        error: executed.error,
        duration_ms: executed.duration_ms,
    }
}

async fn join_executed(handle: JoinHandle<Executed>) -> Executed {
    match handle.await {
        Ok(executed) => executed,
        Err(e) => Executed {
            success: false,
            exit_code: None,
            output: String::new(),
            error: Some(format!("node task failed: {e}")),
            duration_ms: 0,
        },
    }
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
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::graph::Edge;

    // ---- test fixtures ------------------------------------------------------

    fn shell(id: &str, run: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            body: NodeBody::Shell {
                run: run.to_string(),
            },
            writes: false,
            allow_failure: false,
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
        fn call(&self, tool: &str, _arguments: &serde_json::Value) -> Result<String, String> {
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
        fn call(&self, tool: &str, _arguments: &serde_json::Value) -> Result<String, String> {
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
        fn call(&self, tool: &str, _arguments: &serde_json::Value) -> Result<String, String> {
            std::thread::sleep(Duration::from_millis(20));
            Ok(format!("ran:{tool}"))
        }
    }

    // ---- R8: deterministic topo order ---------------------------------------

    #[test]
    fn the_topo_order_is_deterministic_across_runs() {
        let graph = Graph {
            nodes: vec![mcp("c", "t"), mcp("a", "t"), mcp("b", "t")],
            edges: vec![edge("a", "c"), edge("b", "c")],
        };
        let first = topo_order(&graph);
        for n in 0..50 {
            assert_eq!(
                topo_order(&graph),
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
        let order = topo_order(&graph);
        let pos_a = order.iter().position(|id| id == "a").expect("a present");
        let pos_z = order.iter().position(|id| id == "z").expect("z present");
        assert!(pos_a < pos_z, "`a` must precede `z`: {order:?}");
    }

    // ---- R9: bounded concurrent fan-out --------------------------------------

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

        let report = run(
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

        let report = run(
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
        let report = run(
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

        let report = run(
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

        let report = run(
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
        let report = run(
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
        let report = run(
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
        let report = run(
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
        let report = run(
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
        let report = run(
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

    // ---- limit validation -------------------------------------------------------

    #[tokio::test]
    async fn a_zero_limit_is_refused_rather_than_deadlocking() {
        let graph = Graph {
            nodes: vec![shell("n1", "exit 0")],
            edges: vec![],
        };
        let result = run(
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
}
