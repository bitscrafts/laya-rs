//! `laya` — the terminal runner, and the readiness doctor.
//!
//! `laya doctor` is the end-to-end exercise of the readiness path: it probes Laya
//! and **exits non-zero** when not ready, so a hook, a cron job or a human can ask
//! "is Laya up?" without parsing prose. The answer is an exit code.
//!
//! Usage:
//!
//! ```text
//! laya doctor [--url URL]     readiness check; exit 0 ready / 1 not ready
//! ```
//!
//! `decide` and `ledger` are not implemented (see the crate docs).

#![forbid(unsafe_code)]

pub mod decide;
pub mod embedder;
pub mod graph;
pub mod ledger;
pub mod mcp_client;

pub use decide::{DecideArgs, DecideOutput, decide};
pub use embedder::{HttpEmbedder, parse_embedding};
pub use graph::graph;
pub use laya_backbone::{DEFAULT_ENDPOINT, PROBE_TIMEOUT_MS};
pub use laya_core::readiness::{Readiness, ReadinessFailure};
pub use mcp_client::{DEFAULT_CALL_TIMEOUT, LiveMcpCaller, resolve_mcp_binary};
