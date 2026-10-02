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
use std::sync::atomic::{AtomicBool, Ordering};

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
/// if an edge names a node the graph does not declare. [`RunError::Cycle`] if
/// the graph is not a DAG — refused before anything runs. [`RunError::NodeTaskFailed`]
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

    refuse_cycle(graph)?;

    let (mut indegree, successors) = dependency_tables(graph);
    let mut ready: BTreeSet<String> = indegree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let halt = Arc::new(AtomicBool::new(false));
    let gates = Gates {
        fanout: Arc::new(Semaphore::new(opts.limit)),
        write_lock: Arc::new(Semaphore::new(1)),
        halt: Arc::clone(&halt),
    };
    let mut joins: JoinSet<Outcome> = JoinSet::new();
    let mut records = Vec::new();
    let mut stopped_early = false;
    let mut failed_node: Option<String> = None;
    let mut stop_scheduling = false;

    spawn_ready(graph, &mut ready, &mut joins, &opts, &gates);

    while let Some(outcome) = joins.join_next().await {
        let record = match outcome {
            Ok(Outcome::Ran(record)) => record,
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
            spawn_ready(graph, &mut ready, &mut joins, &opts, &gates);
        }

        records.push(record);
    }

    let not_started = nodes_without_record(graph, &records);

    Ok(RunReport {
        run_id: opts.run_id,
        stopped_early,
        failed_node,
        worker_limit: opts.limit,
        records,
        not_started,
    })
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
    Ran(NodeRecord),
    Halted,
}

/// Refuse a graph `topo_order` cannot fully order, naming what is left over
/// (R3). Runs before anything executes.
fn refuse_cycle(graph: &Graph) -> Result<(), RunError> {
    let ordered = topo_order(graph);
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

/// Sorted ids of the graph's nodes with no record: halted tasks plus nodes
/// never spawned because scheduling stopped (R2).
fn nodes_without_record(graph: &Graph, records: &[NodeRecord]) -> Vec<String> {
    let recorded: BTreeSet<&str> = records.iter().map(|r| r.node_id.as_str()).collect();
    graph
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .filter(|id| !recorded.contains(id.as_str()))
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Spawn every currently-ready node, in id order (R8) — the dispatch ORDER is
/// deterministic. Actual concurrency is bounded inside [`run_one`] by the
/// shared semaphores, not by how many tasks are spawned here, so spawning
/// every ready node up front does not itself create unbounded concurrency.
fn spawn_ready(
    graph: &Graph,
    ready: &mut BTreeSet<String>,
    joins: &mut JoinSet<Outcome>,
    opts: &RunOptions,
    gates: &Gates,
) {
    while let Some(id) = pop_first(ready) {
        // Unreachable once `run` has validated every edge's endpoints exist,
        // kept so a future caller of this private helper fails safe rather
        // than panicking.
        let Some(node) = graph.node(&id).cloned() else {
            continue;
        };
        let run_id = opts.run_id.clone();
        let fanout = Arc::clone(&gates.fanout);
        let write_lock = Arc::clone(&gates.write_lock);
        let halt = Arc::clone(&gates.halt);
        let mcp = Arc::clone(&opts.mcp);
        joins.spawn(run_one(node, run_id, fanout, write_lock, halt, mcp));
    }
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
) -> Outcome {
    let Ok(_fanout_permit) = fanout.acquire_owned().await else {
        return Outcome::Ran(failed_record(
            run_id,
            node.id,
            "internal: fan-out semaphore closed",
        ));
    };
    let _write_permit = if node.writes {
        match write_lock.acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => {
                return Outcome::Ran(failed_record(
                    run_id,
                    node.id,
                    "internal: write-lock semaphore closed",
                ));
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

    let allow_failure = node.allow_failure;
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

    // Raised HERE, while this task still holds its permits: the next queued
    // sibling can acquire the instant they drop, which can be before `run`'s
    // loop has seen this record. Setting it in the loop alone would race (R1).
    if !executed.success && !allow_failure {
        halt.store(true, Ordering::SeqCst);
    }

    Outcome::Ran(NodeRecord {
        run_id,
        node_id: node.id,
        success: executed.success,
        exit_code: executed.exit_code,
        output: executed.output,
        error: executed.error,
        duration_ms: executed.duration_ms,
    })
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
                    .enable_time()
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
}
