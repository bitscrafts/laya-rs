//! `laya-store` — the embedded decision ledger.
//!
//! **STATUS: NOT STARTED.** Spec
//! [`specs/04-mcp-server-and-ledger.md`](../../../specs/04-mcp-server-and-ledger.md)
//! and `_docs/05-operations/DECISION-LEDGER.md` hold the contract.
//!
//! ## The single-writer design (blocker B4)
//!
//! The first draft had every consumer (Hermes, pi, Claude Code, a hook) spawn its
//! own `laya-mcp` against **one** embedded store. That collides with redb's model:
//! redb is an embedded **single-process** database, and a hook firing while an agent
//! session holds the store is the *normal* case, not an edge case.
//!
//! So:
//!
//! ```text
//!   laya-mcp (per consumer)            laya-cli
//!   read-only open                     `laya ledger compact`
//!   + its OWN segment file   ────────► = THE ONLY WRITER
//!   ledger-<pid>.jsonl                 merges segments into ledger.redb
//! ```
//!
//! A server process **never needs write access** to the store, so the collision
//! cannot occur. This must be **proven, not assumed** — spec 04 includes a
//! two-process test asserting a second writer fails with a typed error rather than a
//! panic or corruption.
//!
//! ## What a row records
//!
//! Every decision, as a JSON document. Full field list in
//! `_docs/05-operations/DECISION-LEDGER.md`; the load-bearing ones here:
//!
//! * `margin` (top1 − top2) — added by blocker **B1**, because a near-tie is where a
//!   port can flip;
//! * `state_sha256` — **never the state itself**, because a state can contain file
//!   contents and a file can contain a secret;
//! * `laya_version`, `head_sha256`, `backbone_sha256` — provenance, so a swapped
//!   artifact fails loudly instead of deciding silently;
//! * `bucket_clamped` — from blocker **B2**; confidence from a clamped bucket is
//!   uncalibrated.
//!
//! ## Pinned version
//!
//! `redb = 4.2` — chosen deliberately rather than inherited: the alternative pins we
//! compared against sit on different majors, which differ in lock and repair
//! behaviour, so the major was picked for this project rather than copied. No other
//! project here is a dependency, and none is named in this crate.

#![forbid(unsafe_code)]

/// The store file name, relative to the configured data directory.
pub const STORE_FILE: &str = "ledger.redb";

/// Segment files are named with this prefix, so compaction can find them without
/// tracking them in a registry.
pub const SEGMENT_PREFIX: &str = "ledger-";

/// Segment file suffix.
pub const SEGMENT_SUFFIX: &str = ".jsonl";
