//! `laya-store` — the embedded decision ledger.
//!
//! # What this is for
//!
//! Every decision lands in an embedded **redb** store, so that a cost and latency habit is
//! *visible* rather than assumed, a routing decision can be joined to the run that consumed
//! it, and a threshold change can be tested after the fact against recorded decisions.
//!
//! It is emphatically **not** a place to put state or secrets.
//!
//! # The single-writer design (blocker B4)
//!
//! The first draft had every consumer (Hermes, pi, an editor, a hook) spawn its own `laya-mcp`
//! against **one** embedded store. That collides with redb's model: redb is an embedded
//! **single-process** database, and a hook firing while a session holds the store is the
//! *normal* case, not an edge case.
//!
//! ```text
//!   laya-mcp (per consumer)            laya-cli
//!   read-only open                     `laya ledger compact`
//!   + its OWN segment file   ────────► = THE ONLY WRITER
//!   ledger-<pid>.jsonl                 merges segments into ledger.redb
//! ```
//!
//! A server process **never needs write access** to the store, so the collision cannot occur.
//! That is not an assertion here: [`append_segment`] takes a *directory*, not a store handle,
//! so it is structurally incapable of opening the database. The two-process test proves the
//! typed error a second writer really gets.
//!
//! # Append-only, including outcomes
//!
//! An outcome is **its own record**, tagged [`Record::Outcome`], not an edit of a decision.
//! That keeps the store append-only (so a segment merge cannot lose or rewrite history) and it
//! is what makes calibration *possible later*: `calibrate` needs `(confidence, correct)`
//! pairs, and nothing else in this system records whether a decision was right.
//!
//! # Compaction MOVES segments; it never deletes them
//!
//! Ingested segments are moved into an `ingested/` subdirectory rather than removed. Merging
//! is therefore idempotent and re-runnable, a crashed compaction loses nothing, and the raw
//! evidence stays on disk for a human. (This workspace never hard-deletes; a compaction that
//! deleted would be the one command capable of destroying the ledger's own audit trail.)

#![forbid(unsafe_code)]

pub mod calibrate;

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use redb::{ReadableDatabase as _, ReadableTable as _};
use serde::{Deserialize, Serialize};
use std::io::Write as _;

/// The store file name, relative to the configured data directory.
pub const STORE_FILE: &str = "ledger.redb";

/// Segment files are named with this prefix, so compaction can find them without a registry.
pub const SEGMENT_PREFIX: &str = "ledger-";

/// Segment file suffix.
pub const SEGMENT_SUFFIX: &str = ".jsonl";

/// Ingested segments are moved here, never deleted.
pub const INGESTED_DIR: &str = "ingested";

/// The one table: a monotonic sequence number to the row's JSON text.
///
/// The key is a sequence rather than a hash so ordering is free and `tail` is a range scan.
/// Deduplication on re-compaction uses [`Record::identity`], not the key.
pub const ROWS: redb::TableDefinition<'static, u64, &'static str> =
    redb::TableDefinition::new("rows");

/// The next sequence number, so compaction continues rather than restarting at zero.
pub const META: redb::TableDefinition<'static, &'static str, u64> =
    redb::TableDefinition::new("meta");

/// The key in [`META`] holding the next sequence number.
pub const NEXT_SEQ: &str = "next_seq";

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One recorded decision.
///
/// Field names are the contract — the ledger doc lists them, and a name change is a schema
/// change. Deliberately absent: the state itself (only [`DecisionRow::state_sha256`]), any
/// credential, and `act_probability` (the head is inert on it, and surfacing it invites
/// someone to gate on it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRow {
    /// A human-readable stamp. **Not RFC 3339** — the server emits `epoch+{secs}`, which this
    /// comment used to claim was RFC 3339. The wire format is left alone and the claim is fixed.
    pub ts: String,
    /// Monotonic nanoseconds, measured from THIS process's start. **Not comparable across
    /// processes**, so it is not an ordering key.
    pub ts_nanos: u64,
    /// Epoch nanoseconds — comparable across every writer. **This is the ordering key.**
    ///
    /// Merging segments by `ts_nanos` interleaves rows from different processes arbitrarily.
    /// Measured on the live store: server rows carried 65..171 (process-relative) while CLI
    /// outcome rows carried epoch nanos near 1.79e18 — a ~10^16 gap, so every outcome sorted
    /// after every decision whatever the real time. `ts` is second-resolution and cannot break
    /// the tie.
    #[serde(default)]
    pub wall_nanos: u64,
    /// Which tool produced it (`decide`, `gate`, `ask_file`, …).
    pub tool: String,
    /// The join key to the consumer's own run, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The join key to the consumer's own node, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Which question this answers.
    pub question_id: String,
    /// One of the three question types: `noul`, `choice`, `score`.
    pub question_type: String,
    /// The answer as a routing label — the field a graph `route` node matches on.
    pub label: String,
    /// Laya's calibrated confidence, or `None` when the payload carried none.
    ///
    /// **Never defaulted to 0.0.** A missing confidence recorded as 0.0 is indistinguishable
    /// from "maximally unsure" and poisons every calibration metric built on the column.
    #[serde(default)]
    pub confidence: Option<f64>,
    /// `low` / `mid` / `high`, computed by us, not by the model.
    pub band: String,
    /// top1 − top2. A near tie is where a route port can flip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// Whether [`DecisionRow::margin`] fell under the policy floor.
    #[serde(default)]
    pub near_tie: bool,
    /// The full distribution, so saturation can be re-derived rather than trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<serde_json::Value>,
    /// Whether one option held more than the saturation threshold.
    pub saturated: bool,
    /// The pinned Laya version. **Provenance.**
    pub laya_version: String,
    /// Hash of the decided state. **Never the state itself** — a state can hold a file, and a
    /// file can hold a secret.
    pub state_sha256: String,
    /// Hash of the decision head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha256: Option<String>,
    /// Hash of the backbone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backbone_sha256: Option<String>,
    /// Whether the head's temperature clamp fired; confidence from such a bucket is
    /// **uncalibrated**. `None` until a source that actually knows supplies it — it used to be
    /// copied from `saturated`, a different fact, which made a missing value look known.
    #[serde(default)]
    pub bucket_clamped: Option<bool>,
    /// Wall-clock latency, measured on the **real** path.
    pub latency_ms: u64,
    /// Whether the action this row judged was actually carried out. For a `gate` row this
    /// separates "allowed" from "allowed and then executed", which is the difference between a
    /// permission and an event.
    ///
    /// `None` for a tool that has no such notion (`decide`) and for every row written before the
    /// field existed, so "not applicable" is not recorded as "did not run".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed: Option<bool>,
    /// The final `allow` / `ask` / `deny` of a `gate` or `ask` row (spec 11, R2). The row's
    /// `label` carries the same word for a gate row, but an `ask` row's labels are its model
    /// answers, so the verdict needs a field of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_decision: Option<String>,
    /// What the model alone said (`yes` / `no` / `unknown`), before the guardrails downgraded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_decision: Option<String>,
    /// How many destructive-command signals the guardrail found in the judged command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_signals: Option<u32>,
    /// Hash of the command a `gate` or `ask` row judged. **Never the command itself** (rule 8):
    /// a command can carry a token, and a ledger is read by people who must not see one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_sha256: Option<String>,
    /// For a `noul` question, the raw belief. Copied from the payload so a noul's probability is
    /// recoverable from the row (the ledger doc used to promise this field and the row lacked it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    /// For a `score` question, the position on the rubric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// An outcome for an earlier decision.
///
/// This is the record that does not exist anywhere else, and `calibrate` cannot mean anything
/// without it: ECE needs `(confidence, correct)` pairs, and the ledger otherwise records only
/// what was decided, never whether it was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeRow {
    /// RFC 3339 timestamp, UTC.
    pub ts: String,
    /// Monotonic nanoseconds from THIS process's start, as in [`DecisionRow::ts_nanos`]. Not
    /// comparable across processes.
    pub ts_nanos: u64,
    /// Epoch nanoseconds — the key that orders across every writer. See [`DecisionRow::wall_nanos`].
    #[serde(default)]
    pub wall_nanos: u64,
    /// The [`Record::identity`] of the decision being judged. A reference, so the store stays
    /// append-only.
    pub decision: String,
    /// Whether the decision turned out to be right.
    pub correct: bool,
    /// Who decided that — `test`, `human`, `later-run`. **Required**: an outcome with no
    /// author cannot be weighed against another.
    pub source: String,
    /// Free-form context: which test, which override, what the human said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A record in the ledger. Tagged, so a segment file is self-describing.
// The variants differ a lot in size (a DecisionRow carries ~15 fields). Boxing the large
// one would add an allocation to every decision's hot path to save stack in an enum that
// is built one at a time, so the difference is allowed and explained rather than
// silenced.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A decision.
    Decision(DecisionRow),
    /// An outcome for a decision.
    Outcome(OutcomeRow),
}

impl Record {
    /// A stable identity for this record, used to make compaction idempotent.
    ///
    /// Timestamp plus the identifying fields, joined with a separator that cannot occur in any
    /// of them (a unit separator, not `:` — a node id may contain a colon). Re-compacting the
    /// same segment therefore recognises its own rows instead of duplicating them, which is
    /// what makes "merge then move aside" safe to run twice.
    #[must_use]
    pub fn identity(&self) -> String {
        match self {
            // `wall_nanos` is part of the identity because `ts_nanos` is process-relative: two
            // servers each answering the same question at the same offset into their own lives
            // would otherwise produce the SAME identity, and the second row would be dropped as
            // a "duplicate" of a decision that was in fact a different one.
            Self::Decision(d) => format!(
                "d\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                d.ts_nanos, d.wall_nanos, d.tool, d.question_id, d.label
            ),
            Self::Outcome(o) => format!(
                "o\u{1f}{}\u{1f}{}\u{1f}{}",
                o.ts_nanos, o.wall_nanos, o.decision
            ),
        }
    }

    /// The comparable ordering key. See [`DecisionRow::wall_nanos`].
    #[must_use]
    pub const fn wall_nanos(&self) -> u64 {
        match self {
            Self::Decision(d) => d.wall_nanos,
            Self::Outcome(o) => o.wall_nanos,
        }
    }

    /// When this record happened, in THIS process's monotonic nanoseconds. Not comparable
    /// across processes; [`Record::wall_nanos`] is what orders.
    #[must_use]
    pub const fn ts_nanos(&self) -> u64 {
        match self {
            Self::Decision(d) => d.ts_nanos,
            Self::Outcome(o) => o.ts_nanos,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors — every failure a caller can branch on is TYPED
// ---------------------------------------------------------------------------

/// Why a ledger operation failed.
///
/// The variant that matters most is [`StoreError::AlreadyOpen`]: a second writer must fail
/// **cleanly**, with something a caller can branch on, rather than panic or corrupt. redb gives
/// `DatabaseError::DatabaseAlreadyOpen`; this surfaces it as a typed value rather than letting
/// it escape as an opaque error string.
#[derive(Debug)]
pub enum StoreError {
    /// Another process (or another `Database` handle) holds the store. Not a crash — a
    /// condition to report so the caller can use its segment file instead.
    AlreadyOpen {
        /// The store path that is locked.
        path: PathBuf,
    },
    /// The redb layer failed.
    Redb(redb::Error),
    /// An I/O failure outside redb.
    Io {
        /// What was being done.
        what: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A segment file holds a line that is not a valid record. Reported with the file and line
    /// number, because a silently skipped line is a silently lost decision.
    BadRecord {
        /// The segment file.
        file: PathBuf,
        /// 1-based line number.
        line: usize,
        /// Why it did not parse.
        why: String,
    },
    /// An outcome names a decision that is not in the store. Refused rather than stored: an
    /// outcome pointing at nothing cannot be joined, so it would be invisible data.
    UnknownDecision {
        /// The identity that was referenced.
        decision: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOpen { path } => write!(
                f,
                "the ledger store at {} is already open for writing. redb is single-writer: \
                 use `laya ledger compact` in ONE process, and let every server write to its \
                 own segment file instead",
                path.display()
            ),
            Self::Redb(e) => write!(f, "the ledger store failed: {e}"),
            Self::Io { what, source } => write!(f, "while {what}: {source}"),
            Self::BadRecord { file, line, why } => write!(
                f,
                "{}:{line} is not a valid ledger record: {why}. Refusing to skip it silently — \
                 a skipped line is a lost decision",
                file.display()
            ),
            Self::UnknownDecision { decision } => write!(
                f,
                "no decision `{decision}` in the store, so this outcome could not be joined to \
                 anything. Compact the segments that contain the decision first"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<redb::Error> for StoreError {
    fn from(e: redb::Error) -> Self {
        // The one redb error with a caller-visible meaning gets its own variant, because a
        // second writer is a CONDITION to report, not a fault to log.
        if matches!(e, redb::Error::DatabaseAlreadyOpen) {
            return Self::AlreadyOpen {
                path: PathBuf::from("<the store is held by another process>"),
            };
        }
        Self::Redb(e)
    }
}

// redb's concrete error types each convert into its umbrella `Error`, so these forward to the
// single conversion above rather than restating the `DatabaseAlreadyOpen` rule five times.
// Generated per type rather than by a blanket impl: a blanket impl over an unconstrained type
// parameter would collide with the reflexive `From<T> for T`.
macro_rules! forward_redb_error {
    ($($t:ty),+ $(,)?) => {
        $(
            impl From<$t> for StoreError {
                fn from(e: $t) -> Self {
                    Self::from(redb::Error::from(e))
                }
            }
        )+
    };
}

forward_redb_error!(
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
    redb::CompactionError,
);

/// The result type for this crate.
pub type Result<T> = std::result::Result<T, StoreError>;

// ---------------------------------------------------------------------------
// Segments — the server's ONLY write, and it cannot touch the store
// ---------------------------------------------------------------------------

/// A segment file's path for a given process id.
#[must_use]
pub fn segment_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{pid}{SEGMENT_SUFFIX}"))
}

/// Every segment file in a directory, sorted so compaction is deterministic.
///
/// `ingested/` is deliberately NOT searched: an already-ingested segment must not be merged
/// twice.
///
/// # Errors
/// [`StoreError::Io`] if the directory cannot be read.
pub fn segments(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir).map_err(|e| StoreError::Io {
        what: format!("reading the segment directory {}", dir.display()),
        source: e,
    })?;
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| StoreError::Io {
            what: format!("reading an entry of {}", dir.display()),
            source: e,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_file() && name.starts_with(SEGMENT_PREFIX) && name.ends_with(SEGMENT_SUFFIX) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Append one record to this process's segment file.
///
/// **This is the only write a server process ever performs.** Note the signature takes a
/// *directory*: it has no way to open the store, so the collision in blocker B4 is structurally
/// impossible rather than merely avoided. One line, flushed before returning, so a crash loses
/// at most the in-flight line.
///
/// # Errors
/// [`StoreError::Io`] if the directory or the segment file cannot be written.
pub fn append_segment(dir: &Path, record: &Record) -> Result<PathBuf> {
    append_segment_for(dir, std::process::id(), record)
}

/// Append one record to a **named** process's segment file.
///
/// [`append_segment`] uses this process's id. This form exists so the multi-process case — the
/// one that motivated the whole design — can be exercised: several segment files in one
/// directory, merged by one writer. Without it, a test can only ever create the single-process
/// case, which is exactly the case that never caused the problem.
///
/// # Errors
/// [`StoreError::Io`] if the directory or the segment file cannot be written.
pub fn append_segment_for(dir: &Path, pid: u32, record: &Record) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| StoreError::Io {
        what: format!("creating the segment directory {}", dir.display()),
        source: e,
    })?;
    let path = segment_path(dir, pid);
    let line = serde_json::to_string(record).map_err(|e| StoreError::Io {
        what: "serialising a ledger record".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| StoreError::Io {
            what: format!("opening the segment file {}", path.display()),
            source: e,
        })?;
    writeln!(file, "{line}").map_err(|e| StoreError::Io {
        what: format!("appending to {}", path.display()),
        source: e,
    })?;
    file.flush().map_err(|e| StoreError::Io {
        what: format!("flushing {}", path.display()),
        source: e,
    })?;
    Ok(path)
}

/// Read every record in a segment file.
///
/// A malformed line is an **error naming the file and line**, never a skip: a skipped line is a
/// decision silently missing from the ledger, and a ledger that quietly loses rows is worse
/// than one that refuses.
///
/// # Errors
/// [`StoreError::BadRecord`] for a line that is not a record; [`StoreError::Io`] for I/O.
pub fn read_segment(path: &Path) -> Result<Vec<Record>> {
    let text = std::fs::read_to_string(path).map_err(|e| StoreError::Io {
        what: format!("reading the segment {}", path.display()),
        source: e,
    })?;
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(line) {
            Ok(record) => out.push(record),
            Err(e) => {
                return Err(StoreError::BadRecord {
                    file: path.to_path_buf(),
                    line: index + 1,
                    why: e.to_string(),
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// A handle on the ledger store.
///
/// Only [`Ledger::open_writer`] can write, and the name says so at the call site.
pub struct Ledger {
    db: redb::Database,
    path: PathBuf,
}

impl fmt::Debug for Ledger {
    /// redb's `Database` is not `Debug`, so this prints what a failing assertion actually needs
    /// — which file was open — rather than a handle.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactReport {
    /// How many segment files were read.
    pub segments_read: usize,
    /// How many records were seen across them.
    pub records_seen: usize,
    /// How many were **newly** inserted (a re-run inserts zero: compaction is idempotent).
    pub records_inserted: usize,
    /// How many were already present, recognised by [`Record::identity`].
    pub records_duplicate: usize,
    /// Where the ingested segments were moved to.
    pub ingested_into: PathBuf,
}

impl Ledger {
    /// Open the store **for writing**. At most one of these may exist per store, process-wide.
    ///
    /// # Errors
    /// [`StoreError::AlreadyOpen`] when another handle (in this or any other process) holds it.
    /// That is the blocker-B4 condition, surfaced as a typed error rather than a panic.
    pub fn open_writer(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
                what: format!("creating the store directory {}", parent.display()),
                source: e,
            })?;
        }
        match redb::Database::create(path) {
            Ok(db) => Ok(Self {
                db,
                path: path.to_path_buf(),
            }),
            Err(redb::DatabaseError::DatabaseAlreadyOpen) => Err(StoreError::AlreadyOpen {
                path: path.to_path_buf(),
            }),
            Err(e) => Err(StoreError::from(redb::Error::from(e))),
        }
    }

    /// Read-only queries against the store.
    ///
    /// This is what a **server** process is allowed to do (F8). It is a separate type from
    /// [`Ledger`] on purpose: redb's read-only handle cannot begin a write transaction at all,
    /// so "a server never writes the store" is a property of the TYPE rather than a convention
    /// a reviewer has to go and check.
    ///
    /// Creates the store when it is missing.
    ///
    /// A read that failed because nothing had been recorded yet would be the same defect as a
    /// reader that reports zeros as if they were measurements — a fresh install has to produce
    /// an EMPTY answer, not an error. The store is created once, empty, and every read after
    /// that is genuinely read-only: redb's read-only handle cannot begin a write transaction,
    /// so "a reader never writes" is still a property of the type rather than a convention.
    /// Nothing is created when the store already exists, so calling this against a live store
    /// cannot take the writer's lock or touch its bytes.
    ///
    /// # Errors
    /// [`StoreError::Redb`] if the store exists and cannot be opened.
    pub fn read_only(path: &Path) -> Result<LedgerReader> {
        if !path.exists() {
            // `open_writer` creates it; the handle is dropped immediately, so this is not a
            // writer that outlives the call.
            let _ = Self::open_writer(path)?;
        }
        match redb::Builder::new().open_read_only(path) {
            Ok(db) => Ok(LedgerReader {
                db,
                path: path.to_path_buf(),
            }),
            Err(e) => Err(StoreError::from(redb::Error::from(e))),
        }
    }

    /// [`Ledger::read_only`], retrying for up to `wait` while a writer holds the store.
    ///
    /// **MEASURED 2026-10-02: redb's read-only handle cannot coexist with a writer** — opening one
    /// while `compact` holds the store fails with [`StoreError::AlreadyOpen`], exactly as a second
    /// writer does. So "read-only" does not by itself let an operator ask for numbers during a
    /// compaction. What it does buy is that a reader never WRITES; the lock window is the writer's
    /// short transaction, and a bounded retry rides it out. A writer that never lets go still ends
    /// in `AlreadyOpen` after `wait`, so a stuck compaction is reported instead of waited on.
    ///
    /// # Errors
    /// [`StoreError::AlreadyOpen`] when the store is still held after `wait`; otherwise as
    /// [`Ledger::read_only`].
    pub fn read_only_waiting(path: &Path, wait: std::time::Duration) -> Result<LedgerReader> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            match Self::read_only(path) {
                Err(StoreError::AlreadyOpen { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                other => return other,
            }
        }
    }

    /// The path this handle was opened on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert a record unless an identical one is already present.
    ///
    /// Returns `true` when it was inserted. Idempotence is by [`Record::identity`], which is
    /// what lets compaction run twice without duplicating a single row.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a transaction failure.
    pub fn insert_if_new(&self, record: &Record, seq: u64) -> Result<bool> {
        let identity = record.identity();
        let existing = self.identities()?;
        if existing.contains(&identity) {
            return Ok(false);
        }
        let write = self.db.begin_write().map_err(StoreError::from)?;
        {
            let mut table = write.open_table(ROWS).map_err(StoreError::from)?;
            let json = serde_json::to_string(record).map_err(|e| StoreError::Io {
                what: "serialising a ledger record".to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            })?;
            table.insert(seq, json.as_str()).map_err(StoreError::from)?;
            let mut meta = write.open_table(META).map_err(StoreError::from)?;
            meta.insert(NEXT_SEQ, seq + 1).map_err(StoreError::from)?;
        }
        write.commit().map_err(StoreError::from)?;
        Ok(true)
    }

    /// Every record identity currently in the store.
    ///
    /// Read in ONE read transaction with its own table handle, so the table cannot be dropped
    /// mid-scan.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn identities(&self) -> Result<BTreeSet<String>> {
        let read = self.db.begin_read().map_err(StoreError::from)?;
        let mut out = BTreeSet::new();
        // A first run has no table yet; redb reports that as `TableDoesNotExist`, which for a
        // read means "empty", not "broken".
        match read.open_table(ROWS) {
            Ok(table) => {
                for entry in table.iter().map_err(StoreError::from)? {
                    let (_, value) = entry.map_err(StoreError::from)?;
                    if let Ok(record) = serde_json::from_str::<Record>(value.value()) {
                        out.insert(record.identity());
                    }
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(StoreError::from(redb::Error::from(e))),
        }
        Ok(out)
    }

    /// The next sequence number to use.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn next_seq(&self) -> Result<u64> {
        let read = self.db.begin_read().map_err(StoreError::from)?;
        match read.open_table(META) {
            Ok(table) => Ok(table
                .get(NEXT_SEQ)
                .map_err(StoreError::from)?
                .map_or(0, |v| v.value())),
            Err(redb::TableError::TableDoesNotExist(_)) => Ok(0),
            Err(e) => Err(StoreError::from(redb::Error::from(e))),
        }
    }

    /// Every record in the store, oldest first.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn records(&self) -> Result<Vec<Record>> {
        let read = self.db.begin_read().map_err(StoreError::from)?;
        let mut out = Vec::new();
        match read.open_table(ROWS) {
            Ok(table) => {
                for entry in table.iter().map_err(StoreError::from)? {
                    let (_, value) = entry.map_err(StoreError::from)?;
                    if let Ok(record) = serde_json::from_str::<Record>(value.value()) {
                        out.push(record);
                    }
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(StoreError::from(redb::Error::from(e))),
        }
        Ok(out)
    }

    /// How many records the store holds.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn len(&self) -> Result<usize> {
        Ok(self.records()?.len())
    }

    /// Whether the store holds no records.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Summarise the ledger: counts, latency percentiles and the saturation rate.
    ///
    /// The **saturation rate** is the metric to watch: it is the number that tells us whether
    /// the model's confidence is worth gating on at all.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn stats(&self) -> Result<Stats> {
        Ok(summarise(&self.records()?))
    }

    /// The most recent `n` records, newest first.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn tail(&self, n: usize) -> Result<Vec<Record>> {
        let mut records = self.records()?;
        records.reverse();
        records.truncate(n);
        Ok(records)
    }

    /// Record the outcome of an earlier decision.
    ///
    /// Refuses an outcome whose decision is not in the store: an outcome pointing at nothing
    /// cannot be joined, so it would be invisible data masquerading as evidence.
    ///
    /// # Errors
    /// [`StoreError::UnknownDecision`] when the reference does not resolve.
    pub fn record_outcome(&self, outcome: &OutcomeRow) -> Result<()> {
        let known = self.identities()?;
        if !known.contains(&outcome.decision) {
            return Err(StoreError::UnknownDecision {
                decision: outcome.decision.clone(),
            });
        }
        let seq = self.next_seq()?;
        let _ = self.insert_if_new(&Record::Outcome(outcome.clone()), seq)?;
        Ok(())
    }
}

/// Read-only access to the ledger.
///
/// Separate from [`Ledger`] because the CAPABILITY is the point: this holds a redb
/// `ReadOnlyDatabase`, which has no `begin_write`. A server therefore cannot write the store,
/// and no reviewer has to check that it merely does not.
pub struct LedgerReader {
    db: redb::ReadOnlyDatabase,
    path: PathBuf,
}

impl fmt::Debug for LedgerReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LedgerReader")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl LedgerReader {
    /// The path this reader was opened on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every record in the store, oldest first.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn records(&self) -> Result<Vec<Record>> {
        let read = self.db.begin_read().map_err(StoreError::from)?;
        let mut out = Vec::new();
        match read.open_table(ROWS) {
            Ok(table) => {
                for entry in table.iter().map_err(StoreError::from)? {
                    let (_, value) = entry.map_err(StoreError::from)?;
                    if let Ok(record) = serde_json::from_str::<Record>(value.value()) {
                        out.push(record);
                    }
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(StoreError::from(redb::Error::from(e))),
        }
        Ok(out)
    }

    /// How many records the store holds.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn len(&self) -> Result<usize> {
        Ok(self.records()?.len())
    }

    /// Whether the store holds no records.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// A summary: counts, latency percentiles and the saturation rate.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn stats(&self) -> Result<Stats> {
        Ok(summarise(&self.records()?))
    }

    /// The most recent `n` records, newest first.
    ///
    /// # Errors
    /// [`StoreError::Redb`] on a read failure.
    pub fn tail(&self, n: usize) -> Result<Vec<Record>> {
        let mut records = self.records()?;
        records.reverse();
        records.truncate(n);
        Ok(records)
    }
}

/// Compute a [`Stats`] from records.
///
/// Shared by the writer and the reader, so the two can never report different numbers for the
/// same store — a second implementation of a summary is a second answer waiting to disagree.
///
fn summarise(records: &[Record]) -> Stats {
    let mut decisions: Vec<&DecisionRow> = Vec::new();
    let mut outcomes = 0_usize;
    for r in records {
        match r {
            Record::Decision(d) => decisions.push(d),
            Record::Outcome(_) => outcomes += 1,
        }
    }
    let mut latencies: Vec<u64> = decisions.iter().map(|d| d.latency_ms).collect();
    latencies.sort_unstable();
    let saturated = decisions.iter().filter(|d| d.saturated).count();
    let near_ties = decisions.iter().filter(|d| d.near_tie).count();
    let mut by_band = std::collections::BTreeMap::new();
    for d in &decisions {
        *by_band.entry(d.band.clone()).or_insert(0_usize) += 1;
    }
    // Per tool, because a blended percentile hides the tool that is actually slow.
    let mut by_tool: std::collections::BTreeMap<String, Vec<&DecisionRow>> =
        std::collections::BTreeMap::new();
    for d in &decisions {
        by_tool.entry(d.tool.clone()).or_default().push(d);
    }
    let by_tool = by_tool
        .into_iter()
        .map(|(tool, rows)| {
            let mut lats: Vec<u64> = rows.iter().map(|d| d.latency_ms).collect();
            lats.sort_unstable();
            // A `gate` verdict is its `label`; `executed` is whether the command ran. Both are
            // read off the row, so a refusal is counted exactly like an allow.
            let mut verdicts = std::collections::BTreeMap::new();
            let mut executed = 0_usize;
            for d in &rows {
                if d.tool == "gate" || d.tool == "ask" {
                    // The verdict is `gate_decision` when the row has one, else its label (a
                    // gate row's label IS the verdict; rows from before the field are such).
                    let verdict = d.gate_decision.clone().unwrap_or_else(|| d.label.clone());
                    *verdicts.entry(verdict).or_insert(0_usize) += 1;
                    if d.executed == Some(true) {
                        executed += 1;
                    }
                }
            }
            (
                tool,
                ToolStats {
                    decisions: rows.len(),
                    latency_p50_ms: percentile(&lats, 50),
                    latency_p95_ms: percentile(&lats, 95),
                    latency_p99_ms: percentile(&lats, 99),
                    latency_max_ms: lats.last().copied(),
                    verdicts,
                    executed,
                },
            )
        })
        .collect();

    Stats {
        decisions: decisions.len(),
        outcomes,
        saturated,
        near_ties,
        by_band,
        latency_p50_ms: percentile(&latencies, 50),
        latency_p95_ms: percentile(&latencies, 95),
        latency_max_ms: latencies.last().copied(),
        by_tool,
    }
}

/// A summary of the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    /// How many decisions are recorded.
    pub decisions: usize,
    /// How many outcomes are recorded. If this is zero, the ledger cannot calibrate anything.
    pub outcomes: usize,
    /// How many decisions were saturated (one option held nearly all the mass).
    pub saturated: usize,
    /// How many decisions were near ties.
    pub near_ties: usize,
    /// Counts per confidence band.
    pub by_band: std::collections::BTreeMap<String, usize>,
    /// Median latency, in milliseconds.
    pub latency_p50_ms: Option<u64>,
    /// 95th-percentile latency, in milliseconds.
    pub latency_p95_ms: Option<u64>,
    /// Worst observed latency, in milliseconds.
    pub latency_max_ms: Option<u64>,
    /// Per tool, the same numbers — the point of the exercise.
    ///
    /// A single blended p50 answers nothing: `decide` and `gate` are different costs, and an
    /// average of them moves only when the mix changes. Keyed by tool name.
    pub by_tool: std::collections::BTreeMap<String, ToolStats>,
}

/// Per-tool aggregates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStats {
    /// How many rows this tool produced.
    pub decisions: usize,
    /// Median, 95th percentile and worst latency for this tool.
    pub latency_p50_ms: Option<u64>,
    /// 95th percentile latency for this tool.
    pub latency_p95_ms: Option<u64>,
    /// 99th percentile latency for this tool.
    pub latency_p99_ms: Option<u64>,
    /// Worst latency for this tool.
    pub latency_max_ms: Option<u64>,
    /// For `gate`/`ask`: how many times the gate allowed, asked, denied, or was unparseable.
    pub verdicts: std::collections::BTreeMap<String, usize>,
    /// For `gate`/`ask`: how many verdicts were actually executed.
    pub executed: usize,
}

/// Nearest-rank percentile over a sorted slice. `None` for an empty slice, because a percentile
/// of nothing is not zero — it is unknown.
fn percentile(sorted: &[u64], pct: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    // Nearest-rank: ceil(pct/100 * n), clamped into range. Integer arithmetic so the answer is
    // reproducible across platforms.
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    let index = (rank - 1).min(sorted.len() - 1);
    sorted.get(index).copied()
}

/// Merge every segment in `dir` into the store at `store`, then MOVE the segments aside.
///
/// Idempotent: re-running inserts nothing, because records are recognised by
/// [`Record::identity`]. Segments are moved into `dir/ingested/`, never deleted, so a crashed
/// compaction loses nothing and the raw evidence survives for a human.
///
/// # Errors
/// [`StoreError::AlreadyOpen`] if another writer holds the store; [`StoreError::BadRecord`] if a
/// segment holds a malformed line.
pub fn compact(dir: &Path, store: &Path) -> Result<CompactReport> {
    let ledger = Ledger::open_writer(store)?;
    let files = segments(dir)?;
    let ingested_into = dir.join(INGESTED_DIR);
    std::fs::create_dir_all(&ingested_into).map_err(|e| StoreError::Io {
        what: format!("creating {}", ingested_into.display()),
        source: e,
    })?;

    let mut seq = ledger.next_seq()?;
    let mut records_seen = 0_usize;
    let mut inserted = 0_usize;
    let mut duplicate = 0_usize;
    let mut parsed: Vec<Vec<Record>> = Vec::with_capacity(files.len());
    for file in &files {
        let records = read_segment(file)?;
        records_seen += records.len();
        parsed.push(records);
    }

    // Sort by monotonic time so the sequence numbers reflect real order even when segments came
    // from several processes.
    let mut all: Vec<Record> = parsed.into_iter().flatten().collect();
    all.sort_by_key(Record::wall_nanos);

    for record in &all {
        if ledger.insert_if_new(record, seq)? {
            seq += 1;
            inserted += 1;
        } else {
            duplicate += 1;
        }
    }

    // Only now, with every record committed, move the segments aside. A failure before this
    // point leaves them in place to be retried.
    for file in &files {
        let name = file.file_name().unwrap_or_default();
        let dest = ingested_into.join(name);
        // A same-named segment from an earlier ingest must not be silently overwritten.
        let dest = if dest.exists() {
            ingested_into.join(format!(
                "{}.{}",
                name.to_string_lossy(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ))
        } else {
            dest
        };
        std::fs::rename(file, &dest).map_err(|e| StoreError::Io {
            what: format!("moving {} into {}", file.display(), ingested_into.display()),
            source: e,
        })?;
    }

    Ok(CompactReport {
        segments_read: files.len(),
        records_seen,
        records_inserted: inserted,
        records_duplicate: duplicate,
        ingested_into,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    /// A directory that removes itself.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let p = std::env::temp_dir().join(format!("laya-store-{tag}-{nanos}"));
            std::fs::create_dir_all(&p).expect("mkdir");
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A row whose two clocks differ, which is the only fixture that can expose R8.
    ///
    /// `decision()` sets both clocks to the same number, so any ordering assertion it supports
    /// passes whichever key the sort uses -- a check that cannot fail. Two processes measure
    /// `ts_nanos` from their own start, so a row late in one process's life can precede a row
    /// early in another's.
    fn row(tool: &str, label: &str, ts_nanos: u64, wall_nanos: u64, latency_ms: u64) -> Record {
        let mut r = decision(label, ts_nanos);
        if let Record::Decision(d) = &mut r {
            d.tool = tool.to_string();
            d.wall_nanos = wall_nanos;
            d.latency_ms = latency_ms;
        }
        r
    }

    fn decision(label: &str, nanos: u64) -> Record {
        Record::Decision(DecisionRow {
            ts: "2026-09-30T00:00:00Z".to_string(),
            ts_nanos: nanos,
            wall_nanos: nanos,
            tool: "decide".to_string(),
            run_id: Some("run-1".to_string()),
            node_id: Some("triage".to_string()),
            question_id: "kind".to_string(),
            question_type: "choice".to_string(),
            label: label.to_string(),
            confidence: Some(0.93),
            band: "high".to_string(),
            margin: Some(0.89),
            near_tie: false,
            probabilities: None,
            saturated: false,
            laya_version: "0.3.5".to_string(),
            state_sha256: "abc".to_string(),
            head_sha256: None,
            backbone_sha256: None,
            bucket_clamped: Some(false),
            latency_ms: 240,
            executed: None,
            gate_decision: None,
            model_decision: None,
            destructive_signals: None,
            command_sha256: None,
            p: None,
            score: None,
        })
    }

    // ---- the record shape --------------------------------------------------

    #[test]
    fn a_record_is_tagged_so_a_segment_file_is_self_describing() {
        let json = serde_json::to_string(&decision("billing", 1)).expect("serialise");
        assert!(json.contains("\"kind\":\"decision\""), "{json}");
        // And it round-trips, which is the only thing that makes a segment readable later.
        let back: Record = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, decision("billing", 1));
    }

    #[test]
    fn latency_ms_is_required_because_the_budget_claim_depends_on_it() {
        // Dropping the field must be a PARSE error, not a silent zero: NF3 says the latency
        // claim is withdrawn unless measured on the real path, and an unrecorded latency would
        // quietly become "0 ms, well within budget".
        let json = serde_json::to_string(&decision("billing", 1)).expect("serialise");
        let broken = json.replace("\"latency_ms\":240", "\"latency_ms\":null");
        assert!(
            serde_json::from_str::<Record>(&broken).is_err(),
            "a null latency must not parse"
        );
    }

    #[test]
    fn identity_is_stable_and_distinguishes_records() {
        let a = decision("billing", 10);
        let b = decision("billing", 10);
        let c = decision("auth", 10);
        assert_eq!(a.identity(), b.identity(), "same fields, same identity");
        assert_ne!(
            a.identity(),
            c.identity(),
            "a different label is a different row"
        );
    }

    #[test]
    fn identity_separator_cannot_be_confused_by_a_colon_in_a_node_id() {
        // The separator is a unit separator precisely because node ids contain colons; with a
        // colon separator these two would collide and compaction would drop one.
        let mut a = decision("x", 1);
        let mut b = decision("x", 1);
        if let (Record::Decision(da), Record::Decision(db)) = (&mut a, &mut b) {
            da.node_id = Some("a:b".to_string());
            da.tool = "t".to_string();
            db.node_id = Some("a".to_string());
            db.tool = "b:t".to_string();
        }
        assert_ne!(
            a.identity(),
            b.identity(),
            "ids must not collide across fields"
        );
    }

    // ---- segments: the server's only write ---------------------------------

    #[test]
    fn append_then_read_round_trips_a_record() {
        let tmp = Tmp::new("append");
        let path = append_segment(tmp.path(), &decision("billing", 5)).expect("append");
        assert!(path.exists(), "the segment must exist");
        let read = read_segment(&path).expect("read");
        assert_eq!(read, vec![decision("billing", 5)]);
    }

    #[test]
    fn append_accumulates_lines_rather_than_overwriting() {
        // A segment is append-only: a second decision must not erase the first.
        let tmp = Tmp::new("accumulate");
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("first");
        let path = append_segment(tmp.path(), &decision("auth", 2)).expect("second");
        let read = read_segment(&path).expect("read");
        assert_eq!(read.len(), 2, "both records must be present");
    }

    #[test]
    fn append_creates_the_directory_when_it_does_not_exist() {
        // The very first decision on a fresh machine must not need manual setup.
        let tmp = Tmp::new("mkdir");
        let nested = tmp.path().join("does/not/exist/yet");
        let _ = append_segment(&nested, &decision("billing", 1)).expect("append");
        assert!(nested.is_dir());
    }

    #[test]
    fn a_malformed_line_is_an_error_naming_the_file_and_line_not_a_skip() {
        // The load-bearing one. A skipped line is a decision silently missing from the ledger,
        // and a ledger that quietly loses rows is worse than one that refuses.
        let tmp = Tmp::new("badline");
        let path = segment_path(tmp.path(), 1);
        // Line 1 is a REAL record, so the only bad line is line 2. Writing a bare
        // `{"kind":"decision"}` here would fail on line 1 instead (a Decision needs its
        // fields), and the test would be asserting the wrong line.
        let good = serde_json::to_string(&decision("billing", 1)).expect("serialise");
        std::fs::write(&path, format!("{good}\nnot json at all\n")).expect("write");
        match read_segment(&path) {
            Err(StoreError::BadRecord { line, .. }) => assert_eq!(line, 2, "the bad line is 2"),
            other => panic!("expected BadRecord on line 2, got {other:?}"),
        }
        let msg = read_segment(&path).unwrap_err().to_string();
        assert!(
            msg.contains("Refusing to skip"),
            "the message must explain: {msg}"
        );
    }

    #[test]
    fn segments_lists_only_segment_files_and_ignores_ingested() {
        let tmp = Tmp::new("listing");
        let _ = append_segment(tmp.path(), &decision("a", 1)).expect("seg");
        std::fs::write(tmp.path().join("unrelated.txt"), "x").expect("write");
        std::fs::create_dir_all(tmp.path().join(INGESTED_DIR)).expect("mkdir");
        std::fs::write(
            tmp.path().join(INGESTED_DIR).join("ledger-9.jsonl"),
            "{\"kind\":\"decision\"}\n",
        )
        .expect("write");
        let found = segments(tmp.path()).expect("list");
        assert_eq!(found.len(), 1, "exactly one live segment: {found:?}");
    }

    // ---- the store ---------------------------------------------------------

    #[test]
    fn a_fresh_store_is_empty_and_has_no_rows_table_yet() {
        // redb reports a missing table on first read; that is "empty", not "broken".
        let tmp = Tmp::new("empty");
        let ledger = Ledger::open_writer(&tmp.path().join(STORE_FILE)).expect("open");
        assert!(ledger.is_empty().expect("len"));
        assert_eq!(ledger.next_seq().expect("seq"), 0);
    }

    #[test]
    fn two_writers_on_one_store_is_a_typed_error_not_a_panic() {
        // Blocker B4, proven rather than assumed.
        let tmp = Tmp::new("conflict");
        let store = tmp.path().join(STORE_FILE);
        let _first = Ledger::open_writer(&store).expect("the first writer must succeed");
        match Ledger::open_writer(&store) {
            Err(StoreError::AlreadyOpen { .. }) => {}
            other => panic!("expected AlreadyOpen, got {other:?}"),
        }
        let msg = Ledger::open_writer(&store).unwrap_err().to_string();
        assert!(
            msg.contains("single-writer"),
            "the message must name the rule: {msg}"
        );
        assert!(msg.contains("segment"), "and point at the fix: {msg}");
    }

    // ---- compaction --------------------------------------------------------

    #[test]
    fn compact_merges_segments_into_the_store() {
        let tmp = Tmp::new("compact");
        // Two SEGMENTS, as two processes would produce — which is the case the design is for.
        // (Two calls from one process share a pid, and therefore a single file: that is the
        // design, not a bug.)
        let _ = append_segment_for(tmp.path(), 101, &decision("billing", 1)).expect("a");
        let _ = append_segment_for(tmp.path(), 202, &decision("auth", 2)).expect("b");
        let report = compact(tmp.path(), &tmp.path().join(STORE_FILE)).expect("compact");
        assert_eq!(report.segments_read, 2, "two processes, two files");
        assert_eq!(report.records_seen, 2);
        assert_eq!(report.records_inserted, 2);
        let ledger = Ledger::open_writer(&tmp.path().join(STORE_FILE)).expect("reopen");
        assert_eq!(
            ledger.len().expect("len"),
            2,
            "row count == sum of segments"
        );
    }

    #[test]
    fn one_process_writes_one_segment_file_however_many_records() {
        // The design: a segment is per-PROCESS, not per-record. Two calls from this process
        // therefore share one file. Pinned because a test that assumed otherwise would fail
        // for the wrong reason (as this one first did).
        let tmp = Tmp::new("onefile");
        let a = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let b = append_segment(tmp.path(), &decision("auth", 2)).expect("b");
        assert_eq!(a, b, "one process, one segment file");
        assert_eq!(segments(tmp.path()).expect("list").len(), 1);
        assert_eq!(
            read_segment(&a).expect("read").len(),
            2,
            "both records in it"
        );
    }

    #[test]
    fn compact_merges_across_many_processes_segments() {
        // The case the single-writer design exists for: N consumers, one writer. Five
        // segments must merge into one store with every row intact.
        let tmp = Tmp::new("many");
        for pid in 1..=5_u32 {
            let _ = append_segment_for(tmp.path(), pid, &decision("label", u64::from(pid)))
                .expect("append");
        }
        let report = compact(tmp.path(), &tmp.path().join(STORE_FILE)).expect("compact");
        assert_eq!(report.segments_read, 5);
        let ledger = Ledger::open_writer(&tmp.path().join(STORE_FILE)).expect("reopen");
        assert_eq!(
            ledger.len().expect("len"),
            5,
            "every row survived the merge"
        );
    }

    #[test]
    fn compact_is_idempotent_because_records_are_recognised_by_identity() {
        // The reason compaction can be re-run safely: the second run must insert nothing.
        let tmp = Tmp::new("idempotent");
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("first");
        // Put an identical record back in a fresh segment and compact again.
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("again");
        let report = compact(tmp.path(), &store).expect("second");
        assert_eq!(report.records_seen, 1);
        assert_eq!(report.records_inserted, 0, "nothing new");
        assert_eq!(report.records_duplicate, 1, "recognised as the same record");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        assert_eq!(ledger.len().expect("len"), 1, "still exactly one row");
    }

    #[test]
    fn compact_moves_segments_aside_and_never_deletes_them() {
        // This workspace never hard-deletes. A compaction that deleted would be the one command
        // able to destroy the ledger's own audit trail.
        let tmp = Tmp::new("moves");
        let seg = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let report = compact(tmp.path(), &tmp.path().join(STORE_FILE)).expect("compact");
        assert!(!seg.exists(), "the live path is vacated");
        let moved = report.ingested_into.join(seg.file_name().unwrap());
        assert!(moved.is_file(), "the bytes survive at {}", moved.display());
        assert!(
            segments(tmp.path()).expect("list").is_empty(),
            "no live segments left"
        );
    }

    #[test]
    fn compact_orders_by_monotonic_time_across_segments() {
        // Segments come from several processes, so file order says nothing about real order.
        let tmp = Tmp::new("order");
        let _ = append_segment(tmp.path(), &row("decide", "later", 200, 200, 1)).expect("a");
        let _ = append_segment(tmp.path(), &row("decide", "earlier", 100, 100, 1)).expect("b");
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("compact");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        let records = ledger.records().expect("records");
        assert_eq!(
            records[0].wall_nanos(),
            100,
            "the earlier record comes first"
        );
        assert_eq!(records[1].wall_nanos(), 200);
    }

    #[test]
    fn merged_rows_sort_by_wall_time_not_process_relative_nanos() {
        // R8. Two processes each measure `ts_nanos` from their OWN start, so the two keys
        // disagree about order by construction. Process 101 wrote at wall 1000 but only 5ns into
        // its life; process 102 wrote at wall 500, 900ns into its life.
        //
        // Sorted on `ts_nanos` the store reads [A, B] -- the row from wall 1000 placed BEFORE the
        // row from wall 500. Sorted on `wall_nanos` it reads [B, A], which is what happened.
        // Measured live before the fix: server rows 65..171 against outcome rows near 1.79e18.
        let tmp = Tmp::new("wall-order");
        let _ = append_segment_for(tmp.path(), 101, &row("decide", "A", 5, 1_000, 1))
            .expect("segment A");
        let _ = append_segment_for(tmp.path(), 102, &row("decide", "B", 900, 500, 1))
            .expect("segment B");
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("compact");

        let ledger = Ledger::open_writer(&store).expect("reopen");
        let records = ledger.records().expect("records");
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].wall_nanos(),
            500,
            "the row written first in real time comes first, whatever its process-relative value"
        );
        assert_eq!(records[1].wall_nanos(), 1_000);
        // And the control: on the old key the order really is inverted, which is why this test
        // can fail. If a future change made `ts_nanos` comparable this assertion would break and
        // the test would have to be revisited -- deliberately, not silently.
        assert!(
            records[0].ts_nanos() > records[1].ts_nanos(),
            "the old key disagrees -- so this test is not vacuous"
        );
    }

    #[test]
    fn an_absent_confidence_is_none_not_zero() {
        // R3, second half. `unwrap_or(0.0)` recorded a missing confidence as 0.0, which reads as
        // "maximally unsure" and poisons every calibration metric built on the column.
        let missing: DecisionRow = serde_json::from_value(serde_json::json!({
            "ts": "epoch+1", "ts_nanos": 1, "tool": "decide", "question_id": "q",
            "question_type": "noul", "label": "yes", "band": "low", "near_tie": false,
            "saturated": false, "laya_version": "0.3.5", "state_sha256": "x", "latency_ms": 1
        }))
        .expect("an old row without the newer fields must still deserialise");
        assert_eq!(missing.confidence, None, "absent is absent, not zero");
        assert_eq!(
            missing.bucket_clamped, None,
            "an unknown clamp state is not `false`"
        );
        assert_eq!(missing.wall_nanos, 0, "an old row has no wall clock");
        assert_eq!(missing.executed, None, "not applicable is not `false`");
    }

    #[test]
    fn stats_reports_per_tool_latency_and_gate_decisions() {
        // R7. A single blended p50 answers nothing: `decide` and `gate` are different costs, and
        // the average moves only when the mix changes.
        let tmp = Tmp::new("per-tool");
        let _ = append_segment(tmp.path(), &row("decide", "billing", 1, 1, 10)).expect("d1");
        let _ = append_segment(tmp.path(), &row("decide", "auth", 2, 2, 20)).expect("d2");
        let _ = append_segment(tmp.path(), &row("gate", "allow", 3, 3, 30)).expect("g1");
        let _ = append_segment(tmp.path(), &row("gate", "ask", 4, 4, 40)).expect("g2");
        let _ = append_segment(tmp.path(), &row("gate", "deny", 5, 5, 50)).expect("g3");
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("compact");

        let stats = Ledger::open_writer(&store)
            .expect("reopen")
            .stats()
            .expect("stats");

        let decide = stats
            .by_tool
            .get("decide")
            .expect("decide must be separate");
        assert_eq!(decide.decisions, 2);
        assert_eq!(decide.latency_p50_ms, Some(10));
        assert_eq!(decide.latency_max_ms, Some(20));
        assert!(decide.verdicts.is_empty(), "decide has no verdicts");

        let gate = stats.by_tool.get("gate").expect("gate must be separate");
        assert_eq!(gate.decisions, 3);
        assert_eq!(gate.latency_max_ms, Some(50));
        assert_eq!(gate.verdicts.get("allow"), Some(&1));
        assert_eq!(gate.verdicts.get("ask"), Some(&1));
        assert_eq!(
            gate.verdicts.get("deny"),
            Some(&1),
            "a refusal is counted, not omitted -- the half an incident review wants"
        );
        // The blended view is still there, and still different from either tool's own numbers.
        assert_eq!(stats.decisions, 5);
        assert_eq!(stats.latency_p50_ms, Some(30));
    }

    #[test]
    fn the_documented_fields_exist_on_the_row() {
        // R9. The ledger doc tabulated `p` and `score`, which the row never had -- a reader
        // auditing the ledger would hunt for columns that were never built.
        let doc = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../_docs/05-operations/DECISION-LEDGER.md");
        let text = std::fs::read_to_string(&doc).expect("the ledger doc must be readable");
        let table = text
            .split("### What is deliberately absent")
            .next()
            .expect("the schema table");
        // EVERY optional field must be present in this comparison. The row type skips a `None`
        // field when serialising, so a sparse row would make the check pass by omission -- it
        // would "confirm" a documented field that the struct no longer has.
        let full = serde_json::json!({
            "ts": "epoch+1", "ts_nanos": 1, "wall_nanos": 1, "tool": "decide",
            "run_id": "r", "node_id": "n", "question_id": "q", "question_type": "noul",
            "label": "yes", "confidence": 0.5, "band": "low", "margin": 0.1, "near_tie": false,
            "probabilities": {"yes": 0.5, "no": 0.5}, "saturated": false, "laya_version": "0.3.5",
            "state_sha256": "x", "head_sha256": "h", "backbone_sha256": "b",
            "bucket_clamped": false, "latency_ms": 1, "executed": true,
            "gate_decision": "allow", "model_decision": "yes", "destructive_signals": 0,
            "command_sha256": "c", "p": 0.5, "score": 1.0
        });
        let parsed: DecisionRow =
            serde_json::from_value(full).expect("a fully-populated row must deserialise");
        let row = serde_json::to_value(parsed).expect("serialise");
        let keys: Vec<&String> = row.as_object().expect("an object").keys().collect();

        // Every field the table presents as real must exist. A struck-through entry is a claim
        // that has been withdrawn, so it is skipped rather than asserted.
        let mut checked = 0;
        for line in table.lines().filter(|l| l.starts_with("| `")) {
            if line.contains("~~") {
                continue;
            }
            // Only the FIRST column is the field list; the second column is prose and may
            // itself quote a value in backticks. Taking every backticked token on the line
            // reads `epoch+{secs}` as a field name and fails on the doc's own wording.
            let field_cell = line.split('|').nth(1).unwrap_or_default();
            for field in field_cell.split('`').skip(1).step_by(2).filter(|f| {
                !f.is_empty() && f.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }) {
                assert!(
                    keys.iter().any(|k| k.as_str() == field),
                    "DECISION-LEDGER.md documents `{field}`, which DecisionRow does not have"
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 10,
            "the table must have been parsed: {checked} fields"
        );
    }

    #[test]
    fn compact_continues_the_sequence_rather_than_restarting_it() {
        // If seq restarted at 0, a second compaction would OVERWRITE the first batch's rows.
        let tmp = Tmp::new("seq");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("first", 1)).expect("a");
        let _ = compact(tmp.path(), &store).expect("first compaction");
        let _ = append_segment(tmp.path(), &decision("second", 2)).expect("b");
        let _ = compact(tmp.path(), &store).expect("second compaction");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        assert_eq!(ledger.len().expect("len"), 2, "the first row must survive");
        assert_eq!(ledger.next_seq().expect("seq"), 2, "sequence continued");
    }

    #[test]
    fn compact_with_no_segments_is_a_no_op_not_an_error() {
        // Called on a schedule, with nothing to do, it must succeed quietly.
        let tmp = Tmp::new("noop");
        let report = compact(tmp.path(), &tmp.path().join(STORE_FILE)).expect("compact");
        assert_eq!(report.segments_read, 0);
        assert_eq!(report.records_inserted, 0);
    }

    // ---- outcomes: the path that makes calibrate possible -------------------

    #[test]
    fn an_outcome_is_a_separate_record_so_the_store_stays_append_only() {
        let tmp = Tmp::new("outcome");
        let store = tmp.path().join(STORE_FILE);
        let d = decision("billing", 1);
        let _ = append_segment(tmp.path(), &d).expect("a");
        let _ = compact(tmp.path(), &store).expect("compact");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        let outcome = OutcomeRow {
            ts: "2026-09-30T01:00:00Z".to_string(),
            ts_nanos: 2,
            wall_nanos: 1,
            decision: d.identity(),
            correct: true,
            source: "test".to_string(),
            note: Some("the proration test passed".to_string()),
        };
        ledger.record_outcome(&outcome).expect("record");
        let records = ledger.records().expect("records");
        assert_eq!(
            records.len(),
            2,
            "the decision is untouched; the outcome is a new row"
        );
        assert!(matches!(records[0], Record::Decision(_)));
        assert!(matches!(records[1], Record::Outcome(_)));
    }

    #[test]
    fn an_outcome_for_an_unknown_decision_is_refused() {
        // An outcome pointing at nothing cannot be joined, so it would be invisible data
        // masquerading as evidence.
        let tmp = Tmp::new("unknown");
        let ledger = Ledger::open_writer(&tmp.path().join(STORE_FILE)).expect("open");
        let orphan = OutcomeRow {
            ts: "2026-09-30T01:00:00Z".to_string(),
            ts_nanos: 2,
            wall_nanos: 1,
            decision: "d\u{1f}999\u{1f}decide\u{1f}q\u{1f}x".to_string(),
            correct: true,
            source: "test".to_string(),
            note: None,
        };
        match ledger.record_outcome(&orphan) {
            Err(StoreError::UnknownDecision { .. }) => {}
            other => panic!("expected UnknownDecision, got {other:?}"),
        }
    }

    // ---- the read-only handle (F8: a server cannot write the store) ---------

    #[test]
    fn a_reader_can_read_a_store_a_writer_created() {
        let tmp = Tmp::new("reader");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let _ = compact(tmp.path(), &store).expect("compact");
        // The writer is dropped here, as a compaction process would exit.
        let reader = Ledger::read_only(&store).expect("read-only open");
        assert_eq!(reader.len().expect("len"), 1);
        assert_eq!(reader.stats().expect("stats").decisions, 1);
        assert_eq!(reader.path(), store);
    }

    #[test]
    fn a_reader_agrees_with_the_writer_about_stats() {
        // A second implementation of a summary is a second answer waiting to disagree, so both
        // go through `summarise`. This pins that they cannot drift.
        let tmp = Tmp::new("agree");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let _ = append_segment(tmp.path(), &decision("auth", 2)).expect("b");
        let _ = compact(tmp.path(), &store).expect("compact");
        let reader = Ledger::read_only(&store).expect("open");
        let reader_stats = reader.stats().expect("r");
        drop(reader); // release before taking the write handle
        let writer = Ledger::open_writer(&store).expect("open");
        assert_eq!(reader_stats, writer.stats().expect("w"));
    }

    #[test]
    fn a_held_reader_blocks_a_writer_so_a_server_must_not_hold_one() {
        // MEASURED 2026-09-30, and the opposite of what I first assumed. redb's read-only
        // handle takes the file lock too, so a reader held open makes `compact` fail with
        // `AlreadyOpen`.
        //
        // That is a real operational constraint, not a curiosity: a `laya-mcp` process that
        // opened the store read-only ONCE at startup and kept it would make it IMPOSSIBLE to
        // ever compact the ledger. So the rule is:
        //
        //     NEVER HOLD A READER. Open read-only, read, and DROP -- per call.
        //
        // This test exists so nobody "tidies up" the per-call open into a cached handle.
        let tmp = Tmp::new("locks");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let _ = compact(tmp.path(), &store).expect("first");

        let held = Ledger::read_only(&store).expect("reader");
        match Ledger::open_writer(&store) {
            Err(StoreError::AlreadyOpen { .. }) => {}
            other => panic!("a held reader was expected to block the writer, got {other:?}"),
        }

        // And dropping it frees the store again, which is what makes per-call opening correct.
        drop(held);
        assert!(
            Ledger::open_writer(&store).is_ok(),
            "dropping the reader must release the store"
        );
    }

    #[test]
    fn stats_opens_the_store_read_only() {
        // R7. A read must not take the exclusive write lock. Proven on the TYPE and on the
        // behaviour: `LedgerReader` has no write method at all, and while a reader is held a
        // second READER is not refused by it -- but a WRITER is, which is how we know the reader
        // is really the thing that was opened.
        let tmp = Tmp::new("ro-stats");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &row("gate", "allow", 1, 1, 10)).expect("a");
        let _ = compact(tmp.path(), &store).expect("compact");

        let reader = Ledger::read_only(&store).expect("read-only");
        assert_eq!(reader.stats().expect("stats").decisions, 1);
        // The control: it IS a reader, so it excludes a writer (measured, see
        // `a_held_reader_blocks_a_writer_so_a_server_must_not_hold_one`).
        assert!(matches!(
            Ledger::open_writer(&store),
            Err(StoreError::AlreadyOpen { .. })
        ));
    }

    #[test]
    fn a_reader_waits_out_a_short_compaction_instead_of_failing_on_the_lock() {
        // R7's operator story. A `compact` holds the writer for a moment; `stats` arriving in
        // that moment must get numbers, not `AlreadyOpen`.
        let tmp = Tmp::new("ro-wait");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &row("gate", "allow", 1, 1, 10)).expect("a");
        let _ = compact(tmp.path(), &store).expect("compact");

        let writer = Ledger::open_writer(&store).expect("the compactor's handle");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(writer);
        });
        let started = std::time::Instant::now();
        let reader = Ledger::read_only_waiting(&store, std::time::Duration::from_secs(5))
            .expect("a reader must outlast a short compaction");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(250),
            "it really did have to wait: {:?}",
            started.elapsed()
        );
        assert_eq!(reader.stats().expect("stats").decisions, 1);
        release.join().expect("join");
    }

    #[test]
    fn the_control_a_writer_that_never_lets_go_is_reported_not_waited_on() {
        // The near miss for the test above: a retry that never gave up would hang `stats` behind
        // a stuck compaction. After the bound it must say so, as the typed error.
        let tmp = Tmp::new("ro-stuck");
        let store = tmp.path().join(STORE_FILE);
        let _held = Ledger::open_writer(&store).expect("writer");
        let started = std::time::Instant::now();
        let r = Ledger::read_only_waiting(&store, std::time::Duration::from_millis(200));
        assert!(
            matches!(r, Err(StoreError::AlreadyOpen { .. })),
            "{:?}",
            r.map(|_| ())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn an_old_segment_without_the_new_fields_still_compacts() {
        // N2. A segment line as today's schema writes it: none of `gate_decision`,
        // `model_decision`, `destructive_signals`, `command_sha256`, `p`, `score`, no `wall_nanos`,
        // and `executed` as a plain bool.
        let tmp = Tmp::new("n2");
        let line = r#"{"kind":"decision","ts":"epoch+1","ts_nanos":65,"tool":"gate","question_id":"q","question_type":"noul","label":"allow","confidence":0.5,"band":"mid","near_tie":false,"saturated":false,"laya_version":"0.3.5","state_sha256":"x","bucket_clamped":false,"latency_ms":3,"executed":true}"#;
        std::fs::write(tmp.path().join("ledger-1.jsonl"), format!("{line}\n")).expect("write");
        let store = tmp.path().join(STORE_FILE);
        let report = compact(tmp.path(), &store).expect("an old segment must compact");
        assert_eq!(report.records_inserted, 1);
        let rows = Ledger::open_writer(&store)
            .expect("reopen")
            .records()
            .expect("records");
        let Record::Decision(d) = &rows[0] else {
            panic!("a decision")
        };
        assert_eq!(d.executed, Some(true), "an old bool reads as `Some`");
        assert_eq!(d.gate_decision, None);
        assert_eq!(d.model_decision, None);
        assert_eq!(d.destructive_signals, None);
        assert_eq!(d.command_sha256, None);
        assert_eq!((d.p, d.score), (None, None));
    }

    #[test]
    fn two_servers_answering_at_the_same_process_offset_are_both_kept() {
        // R8's hazard in the identity, not only the sort. `ts_nanos` is process-relative, so two
        // servers can each answer the same question with the same label 65 ns into their lives.
        // On the old identity the second was dropped as a "duplicate"; it is a different decision.
        let tmp = Tmp::new("ident");
        let _ = append_segment_for(tmp.path(), 1, &row("gate", "allow", 65, 1_000, 1)).expect("a");
        let _ = append_segment_for(tmp.path(), 2, &row("gate", "allow", 65, 2_000, 1)).expect("b");
        let store = tmp.path().join(STORE_FILE);
        let report = compact(tmp.path(), &store).expect("compact");
        assert_eq!(report.records_inserted, 2, "{report:?}");
        // The control: the SAME row twice is still a duplicate, so idempotence is intact.
        let _ = append_segment_for(tmp.path(), 3, &row("gate", "allow", 65, 2_000, 1)).expect("c");
        let again = compact(tmp.path(), &store).expect("recompact");
        assert_eq!(again.records_inserted, 0, "{again:?}");
        assert_eq!(again.records_duplicate, 1, "{again:?}");
    }

    #[test]
    fn stats_counts_executed_and_reports_p99_per_tool() {
        // R7: the executed count and p99, on rows shaped as the server now writes them.
        let tmp = Tmp::new("exec");
        for (i, (verdict, ran)) in [("allow", true), ("allow", false), ("deny", false)]
            .into_iter()
            .enumerate()
        {
            let mut r = row("ask", "yes", i as u64, i as u64, 10 * (i as u64 + 1));
            if let Record::Decision(d) = &mut r {
                d.gate_decision = Some(verdict.to_string());
                d.executed = Some(ran);
            }
            let _ = append_segment(tmp.path(), &r).expect("append");
        }
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("compact");
        let stats = Ledger::read_only(&store)
            .expect("ro")
            .stats()
            .expect("stats");
        let ask = stats.by_tool.get("ask").expect("ask");
        assert_eq!(
            ask.verdicts.get("allow"),
            Some(&2),
            "the verdict is gate_decision"
        );
        assert_eq!(ask.verdicts.get("yes"), None, "not the model's label");
        assert_eq!(ask.executed, 1, "only the row that actually ran");
        assert_eq!(ask.latency_p99_ms, Some(30));
    }

    #[test]
    fn tail_on_a_reader_returns_the_newest_first() {
        let tmp = Tmp::new("rtail");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("old", 1)).expect("a");
        let _ = append_segment(tmp.path(), &decision("new", 2)).expect("b");
        let _ = compact(tmp.path(), &store).expect("compact");
        let reader = Ledger::read_only(&store).expect("open");
        let recent = reader.tail(1).expect("tail");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].ts_nanos(), 2);
    }

    // ---- stats -------------------------------------------------------------

    #[test]
    fn stats_reports_zero_outcomes_so_the_calibration_gap_is_visible() {
        // The honest number: with no outcomes, nothing here can be calibrated, and the stats
        // say so rather than implying a calibration that does not exist.
        let tmp = Tmp::new("stats");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("billing", 1)).expect("a");
        let _ = append_segment(tmp.path(), &decision("auth", 2)).expect("b");
        let _ = compact(tmp.path(), &store).expect("compact");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        let s = ledger.stats().expect("stats");
        assert_eq!(s.decisions, 2);
        assert_eq!(s.outcomes, 0, "no outcomes yet — calibrate cannot run");
        assert_eq!(s.by_band.get("high"), Some(&2));
        assert_eq!(s.latency_p50_ms, Some(240));
        assert_eq!(s.latency_max_ms, Some(240));
    }

    #[test]
    fn a_percentile_of_nothing_is_none_not_zero() {
        // Zero would read as "instant", which is a measurement we never made.
        assert_eq!(percentile(&[], 50), None);
        assert_eq!(percentile(&[10], 50), Some(10));
        assert_eq!(percentile(&[10], 95), Some(10));
        assert_eq!(percentile(&[1, 2, 3, 4], 50), Some(2));
        assert_eq!(percentile(&[1, 2, 3, 4], 95), Some(4));
    }

    #[test]
    fn tail_returns_the_newest_first() {
        let tmp = Tmp::new("tail");
        let store = tmp.path().join(STORE_FILE);
        let _ = append_segment(tmp.path(), &decision("old", 1)).expect("a");
        let _ = append_segment(tmp.path(), &decision("new", 2)).expect("b");
        let _ = compact(tmp.path(), &store).expect("compact");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        let recent = ledger.tail(1).expect("tail");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].ts_nanos(), 2, "newest first");
    }
}
