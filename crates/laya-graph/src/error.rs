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
}
