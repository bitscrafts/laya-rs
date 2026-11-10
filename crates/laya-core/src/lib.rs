//! `laya-core` — the pure decision layer.
//!
//! Nothing here does I/O. This crate owns the logic that must be *reviewable and
//! testable without a model*:
//!
//! * [`policy`] — thresholds, confidence bands and composite weights. **All of it
//!   lives in the caller**, never in the model. A band boundary is a number in a
//!   config file a human reads in one line.
//! * [`readiness`] — whether Laya is actually running, and what every tool must do
//!   when it is not. A decision layer that cannot decide must not run, and an
//!   unavailable gate must never mean permission.
//! * [`queue`] — the bounded priority queue that gives a blocking `gate` precedence
//!   over bulk work, so a hook is never stuck behind a file scan.
//! * [`route_payload`] — the shape a graph's `route` node consumes, so
//!   `route on = "$.payload.label"` is all a graph author writes.
//!
//! The split is deliberate and comes from the reference design's own lab: contracts are
//! tested **offline** against a deterministic mock (`just test`), and a separate
//! suite checks **meaning** against the live model (`just test-live`). Anything
//! that needs the model to test does not belong in this crate.

#![forbid(unsafe_code)]

pub mod agentic;
pub mod ask_file;
pub mod ask_files;
pub mod compact;
pub mod policy;
pub mod queue;
pub mod readiness;
pub mod route_payload;

pub use compact::{CompactAdvice, CompactError, CompactInput, CompactThresholds, CompactUrgency};
pub use policy::{Band, GateDecision, Policy, PolicyError};
pub use queue::{EnqueueError, Enqueued, Priority, PriorityQueue, QueueStats};
pub use readiness::{
    READINESS_TTL_MS, Readiness, ReadinessCache, ReadinessFailure, WhenNotReady, gate_when_down,
    judge, when_not_ready,
};
pub use route_payload::DecisionPayload;
