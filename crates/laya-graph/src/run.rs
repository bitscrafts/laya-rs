//! Run records: pure data, carrying `run_id` + `node_id` explicitly (R13).
//!
//! No on-disk store — `laya-graph` does not depend on `laya-store` (G16).
//! `laya-store`'s `Record` is a two-variant enum (`Decision`, `Outcome`) with
//! no variant a `shell` node's result could live in, so a run's records stay
//! here, in `RunReport`, for the day a consumer wants to persist one. The
//! attribution fields a persisted decision needs already exist on
//! `DecisionRow` (`run_id`, `node_id`) — nothing new is required there
//! either.

use serde::{Deserialize, Serialize};

/// One node's result within a run.
///
/// Attribution is explicit, never positional: once nodes run in parallel,
/// append order is not execution order, so a consumer reading "the last row"
/// would attribute a result to the wrong node (R13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    /// The join key to the run this node ran within.
    pub run_id: String,
    /// The join key to the node that produced this record.
    pub node_id: String,
    /// Whether the node's own work succeeded. A non-zero shell exit or an
    /// `mcp` tool error both set this `false` (R12) — even when
    /// `allow_failure` let the run continue past it (R11).
    pub success: bool,
    /// The shell exit code, when this was a `shell` node whose process ran
    /// to completion.
    pub exit_code: Option<i32>,
    /// Raw output captured from the node's own work.
    pub output: String,
    /// Set when `success` is `false`: why.
    pub error: Option<String>,
    /// Wall-clock duration of the node's own work, in milliseconds.
    pub duration_ms: u64,
}

/// What a run produced, in full — never reported as a completed run when it
/// is not (R11): a consumer acts on this, so `stopped_early` and
/// `failed_node` are load-bearing, not advisory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    /// The join key stamped onto every record in this run.
    pub run_id: String,
    /// `true` when a non-`allow_failure` node failed and stopped the run
    /// before every node got a chance to run. **Never blanket-set**: a fully
    /// successful run must read `false` (the control
    /// `the_control_a_completed_run_does_not_say_stopped_early`), and a
    /// stopped run must read `true` with `failed_node` named (R11).
    pub stopped_early: bool,
    /// The id of the node whose failure stopped the run, when `stopped_early`
    /// is `true`. `None` when the run completed (or when nothing has failed
    /// yet, which — once a run returns — means every node that failed did so
    /// with `allow_failure`).
    pub failed_node: Option<String>,
    /// The effective fan-out bound this run used — reported, never assumed
    /// (R9).
    pub worker_limit: usize,
    /// One record per node that actually ran. A node skipped by a fail-fast
    /// stop (R11: "siblings not yet started are not started") has no record
    /// here at all — it never ran.
    pub records: Vec<NodeRecord>,
}
