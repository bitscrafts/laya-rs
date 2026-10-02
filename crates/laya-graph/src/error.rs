//! Every refusal `laya-graph` can make.
//!
//! A refusal that says only "invalid flow" is not acceptable: the message is
//! the product. Every variant here names the node id, the dangling reference,
//! or the offending text, so a refusal is actionable without re-reading the
//! source.

use thiserror::Error;

/// Every way a flow document, or an `on` expression within it, is refused.
///
/// Nothing here is a warning: every variant means the flow did not load.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FlowError {
    /// The flow's JSON did not parse at all.
    #[error("flow JSON did not parse: {0}")]
    Json(String),

    /// Two nodes declared the same `id`.
    #[error("duplicate node id `{id}`")]
    DuplicateNodeId {
        /// The id that was declared twice.
        id: String,
    },

    /// A node's `depends_on` names an id no node in the flow declares.
    #[error("node `{id}` depends_on unknown node `{missing}`")]
    DanglingDependsOn {
        /// The node whose `depends_on` is dangling.
        id: String,
        /// The id it named that does not exist.
        missing: String,
    },

    /// A node's `kind` is not one of the four v1 supports: `shell`, `mcp`,
    /// `decide`, or the internal `route`. This is also how the deliberately
    /// deferred `agent` and `quorum` kinds are refused.
    #[error("node `{id}` has unsupported kind `{kind}`")]
    UnknownKind {
        /// The node with the unsupported kind.
        id: String,
        /// The kind it named.
        kind: String,
    },

    /// A `decide` node's `ports` do not cover every declared option, and it
    /// has no `_` fallback to catch the rest.
    #[error("decide node `{id}` has no route for options {missing:?}, and no `_` fallback")]
    IncompletePorts {
        /// The decide node.
        id: String,
        /// The options with no port and no fallback to catch them.
        missing: Vec<String>,
    },

    /// `retry.attempts` was set without `retry.backoff_ms`, or the reverse —
    /// half a retry policy is a retry storm waiting to happen.
    #[error(
        "node `{id}` has half a retry policy: attempts and backoff_ms must both be set, or neither"
    )]
    HalfRetryPolicy {
        /// The node with the incomplete retry policy.
        id: String,
    },

    /// `escalate_on_near_tie: true` with no `near_tie_port` to send the tie
    /// to.
    #[error("decide node `{id}` sets escalate_on_near_tie with no near_tie_port")]
    MissingNearTiePort {
        /// The decide node.
        id: String,
    },

    /// `inputs` or a `$.inputs.*` reference. Nothing binds or substitutes
    /// them: measured 2026-10-02, `$.inputs.doc` reached the head as literal
    /// text.
    #[error(
        "{location} uses `inputs`, which is not implemented (until a spec binds `inputs` and substitutes `$.inputs.*`; none has landed) — `$.inputs.*` is NOT substituted; the prompt is literal text sent to the head. Remove `inputs` and write the text into `prompt` directly"
    )]
    InputsUnsupported {
        /// Where it was found (the flow, or a node id).
        location: String,
    },

    /// A `decide` node's `on` is neither `label` nor `band`.
    #[error("decide node `{id}` has on = `{on}`; expected `label` or `band` (omit `on` for label)")]
    BadOn {
        /// The decide node.
        id: String,
        /// The value it gave.
        on: String,
    },

    /// A `decide` node's `question_type` is neither `choice` nor `noul` (spec 16).
    #[error(
        "decide node `{id}` has question_type = `{value}`; expected `choice` or `noul` (omit it for choice)"
    )]
    BadQuestionType {
        /// The decide node.
        id: String,
        /// The value it gave.
        value: String,
    },

    /// A `noul` decide that also declares `options` (spec 16): a yes/no question has none.
    #[error(
        "decide node `{id}` is a noul but declares options; a noul answers yes/no and takes no options          (use question_type `choice` for options)"
    )]
    NoulWithOptions {
        /// The decide node.
        id: String,
    },

    /// A `{{target}}` in a decide's `prompt` names no node in the flow.
    #[error(
        "decide node `{id}` prompt references `{{{{{target}}}}}`, which is not a node in this flow. Name an existing node in `depends_on`; a literal `{{{{` is not allowed in a prompt"
    )]
    MissingPlaceholderTarget {
        /// The decide node.
        id: String,
        /// The id the placeholder named.
        target: String,
    },

    /// A `{{target}}` names a real node that is not in this decide's `depends_on`.
    #[error(
        "decide node `{id}` prompt references `{{{{{target}}}}}`, but `{target}` is not in its `depends_on` — it would not have run yet. Add `{target}` to `depends_on`"
    )]
    PlaceholderNotDependency {
        /// The decide node.
        id: String,
        /// The id the placeholder named.
        target: String,
    },

    /// A decide's prompt references the decide itself.
    #[error(
        "decide node `{id}` prompt references itself (`{{{{{id}}}}}`), whose output cannot exist before it runs"
    )]
    PlaceholderSelf {
        /// The decide node.
        id: String,
    },

    /// A `decide` port key is not a value the route can produce.
    #[error(
        "decide node `{id}` has port key `{key}`, which is not one of the {on} values {expected:?} or `_` — a mistyped key is hidden by the `_` catch-all. Rename it to one of those{hint}"
    )]
    UnknownPortKey {
        /// The decide node.
        id: String,
        /// The offending key.
        key: String,
        /// What the routing matches: `label` or `band`.
        on: String,
        /// The keys that are accepted (besides `_`).
        expected: Vec<String>,
        /// A rendered ", did you mean `x`?" or empty.
        hint: String,
    },

    /// A `decide` port targets a node that does not exist.
    #[error(
        "decide node `{id}` port `{port}` targets `{target}`, which is not a node in this flow — it would run nothing. Name an existing node id"
    )]
    PortTargetMissing {
        /// The decide node.
        id: String,
        /// The port key.
        port: String,
        /// The target it named.
        target: String,
    },

    /// A field the node's kind needs is absent.
    #[error("{kind} node `{id}` requires `{field}`")]
    MissingField {
        /// The node.
        id: String,
        /// Its kind.
        kind: String,
        /// The required field.
        field: String,
    },

    /// A `check` has no `_` catch-all port (spec 25, R2).
    #[error(
        "check node `{id}` needs a `_` catch-all port, as a decide does: say where the run goes when neither `pass` nor `fail` is declared"
    )]
    CheckNeedsFallback {
        /// The node.
        id: String,
    },

    /// A `check` port key other than `pass`, `fail` or `_` (spec 25).
    #[error(
        "check node `{id}` has port `{key}`; a check's exit status can only select `pass` (exit 0), `fail` (non-zero) or `_`"
    )]
    BadCheckPort {
        /// The node.
        id: String,
        /// The offending key.
        key: String,
    },

    /// `allow_failure` on a `check` (spec 25, R2).
    #[error(
        "check node `{id}` sets `allow_failure`, which a check does not take: its non-zero exit is its answer, not a failure. Remove it"
    )]
    CheckAllowFailure {
        /// The node.
        id: String,
    },

    /// `version` is not 1.
    #[error("flow `version` is {version}; this engine reads only version 1")]
    BadVersion {
        /// The version found.
        version: u32,
    },

    /// A node id contains `/`, which is reserved for synthetic ids.
    #[error(
        "node id `{id}` contains `/`, which collides with the synthetic `<id>/route` and subflow-prefixed ids. Use another character"
    )]
    BadNodeId {
        /// The id.
        id: String,
    },

    /// A declared field that does nothing is refused until it is implemented.
    #[error("node `{id}` sets `{field}`, which is not implemented ({until}). {instead}")]
    NotImplemented {
        /// The node.
        id: String,
        /// The field.
        field: String,
        /// When the refusal is lifted.
        until: String,
        /// What to do instead.
        instead: String,
    },

    /// A `subflow` node reached a loader that has no library to resolve it.
    #[error(
        "node `{id}` is a `subflow` using `{uses}`, and `subflow` needs a library, which the CLI does not load. Inline the child's nodes into this flow"
    )]
    SubflowNeedsLibrary {
        /// The subflow node.
        id: String,
        /// The `uses` reference.
        uses: String,
    },

    /// A cycle in `depends_on`. Detection always returns; it never hangs.
    #[error("dependency cycle: {chain}")]
    Cycle {
        /// The cycle, rendered as `a -> b -> a`.
        chain: String,
    },

    /// A `route.on` (or lowered `decide.on`) expression is not member access
    /// on a field the route payload actually carries.
    ///
    /// This is the determinism guarantee: the grammar is exactly
    /// `$.payload.<field>`, so the engine can never evaluate a predicate —
    /// only select among ports an author already drew.
    #[error("`on` expression `{0}` is not member access on a field the route payload carries")]
    BadOnExpr(String),

    /// A `subflow` node's `uses` reference (by name, chasing through any
    /// intermediate subflows) eventually names itself. Expansion is refused
    /// rather than recursed, because an expansion cycle is infinite
    /// expansion with nothing to interrupt it — a hang, not a crash.
    #[error("subflow expansion cycle: {}", .chain.join(" -> "))]
    ExpansionCycle {
        /// The reference chain that closes the cycle, e.g. `["x", "y", "x"]`.
        chain: Vec<String>,
    },

    /// A `subflow` node's child declares an input in its own `inputs` list
    /// that the parent's `with` map never binds. Refused at load, not
    /// discovered at run time.
    #[error(
        "subflow `{subflow_id}` does not bind argument `{argument}`, which its child declares as an input"
    )]
    UnboundArgument {
        /// The id of the `subflow` node whose `with` map is incomplete.
        subflow_id: String,
        /// The child's declared input that nobody bound.
        argument: String,
    },

    /// A `subflow` node's declared `content_hash` does not match the
    /// blake3 hash of the child's actual definition — the child was edited
    /// in place after the parent pinned it.
    #[error(
        "subflow `{subflow_id}` declares content_hash `{declared}` but the child's actual hash is `{actual}`"
    )]
    HashMismatch {
        /// The id of the `subflow` node with the mismatched hash.
        subflow_id: String,
        /// The hash the parent declared.
        declared: String,
        /// The hash the child's actual definition hashes to.
        actual: String,
    },
}

/// Every way running a lowered [`crate::graph::Graph`] can be refused before
/// — or independently of — any node's own result.
///
/// A node's exit code, an `mcp` tool error, and a stopped run are never an
/// `Err` here: they are DATA on [`crate::run::NodeRecord`] and
/// [`crate::run::RunReport`] (R11, R12), because a consumer needs the full
/// report even when the run failed. `RunError` is reserved for conditions the
/// scheduler itself cannot proceed from at all.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunError {
    /// `limit` was zero. Refused rather than silently clamped to 1: a
    /// zero-permit fan-out would deadlock the ready set before a single node
    /// could ever run (R9).
    #[error("the concurrency limit must be at least 1, got {limit}")]
    BadLimit {
        /// The limit that was rejected.
        limit: usize,
    },

    /// An edge names a node id the graph does not declare.
    ///
    /// Defensive: a graph produced by [`crate::lower::lower`] from a
    /// [`crate::validate::validate`]d flow cannot have one — but `Graph`'s
    /// fields are public, so nothing stops a hand-built `Graph` from being
    /// malformed, and a silently-dropped node is exactly the "looks complete"
    /// failure mode this part exists to prevent.
    #[error("edge names node `{node_id}`, which the graph does not declare")]
    UnknownNode {
        /// The dangling id.
        node_id: String,
    },

    /// A node's own scheduling task could not be joined — an executor bug
    /// (e.g. a panic in the scheduler's own spawned task), never a node's
    /// reported exit code or tool error. Those stay DATA on its
    /// [`crate::run::NodeRecord`] (R12) and surface through
    /// [`crate::run::RunReport::stopped_early`] (R11), never as `Err`.
    #[error("a node's task could not be joined: {reason}")]
    NodeTaskFailed {
        /// What `tokio` reported about the join failure.
        reason: String,
    },

    /// The graph contains a cycle, so some nodes can never become ready.
    /// Refused up front rather than returning the runnable part as `Ok`: a
    /// partial run that reads as complete is the failure mode R11 names.
    #[error("the graph has a dependency cycle; these nodes can never run: {nodes:?}")]
    Cycle {
        /// The ids left over once every runnable node was ordered, sorted.
        nodes: Vec<String>,
    },

    /// The dependency table the scheduler was about to rely on has no entry for some node the
    /// graph declares (spec 23, R1). Every consumer — the cycle guard included — trusts that
    /// table, so a table that is not about the graph is refused by name, never run on.
    #[error(
        "the dependency table is inconsistent with the graph; it has no entry for: {missing:?}"
    )]
    InconsistentTable {
        /// The declared node ids the table lacks, sorted.
        missing: Vec<String>,
    },

    /// The run ended with nodes neither run nor skipped, and nothing had stopped it (spec 23,
    /// R2). A report built from this would read as complete; naming the leftovers is the point.
    #[error(
        "the scheduler stalled; unsettled nodes: {unsettled:?}; edges never resolved: {unresolved_edges:?}"
    )]
    Stalled {
        /// The ids that were neither run nor skipped, sorted.
        unsettled: Vec<String>,
        /// The `from -> to` edges into those nodes that never resolved, sorted.
        unresolved_edges: Vec<String>,
    },

    /// A node was settled (ran, or was skipped) more than once (spec 23, R2).
    #[error("node `{node_id}` was settled twice")]
    SettledTwice {
        /// The node.
        node_id: String,
    },
}
