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
    /// RFC 3339 timestamp, UTC.
    pub ts: String,
    /// Monotonic nanoseconds — the ordering key for compaction, immune to clock steps.
    pub ts_nanos: u64,
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
    /// Laya's calibrated confidence.
    pub confidence: f64,
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
    /// True when the head's temperature clamp fired; confidence from such a bucket is
    /// **uncalibrated**.
    #[serde(default)]
    pub bucket_clamped: bool,
    /// Wall-clock latency, measured on the **real** path.
    pub latency_ms: u64,
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
    /// Monotonic nanoseconds, as in [`DecisionRow::ts_nanos`].
    pub ts_nanos: u64,
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
            Self::Decision(d) => format!(
                "d\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                d.ts_nanos, d.tool, d.question_id, d.label
            ),
            Self::Outcome(o) => format!("o\u{1f}{}\u{1f}{}", o.ts_nanos, o.decision),
        }
    }

    /// When this record happened, in monotonic nanoseconds — the ordering key.
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
    /// # Errors
    /// [`StoreError::Redb`] if the store does not exist or cannot be opened.
    pub fn read_only(path: &Path) -> Result<LedgerReader> {
        match redb::Builder::new().open_read_only(path) {
            Ok(db) => Ok(LedgerReader {
                db,
                path: path.to_path_buf(),
            }),
            Err(e) => Err(StoreError::from(redb::Error::from(e))),
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
    Stats {
        decisions: decisions.len(),
        outcomes,
        saturated,
        near_ties,
        by_band,
        latency_p50_ms: percentile(&latencies, 50),
        latency_p95_ms: percentile(&latencies, 95),
        latency_max_ms: latencies.last().copied(),
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
    all.sort_by_key(Record::ts_nanos);

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

    fn decision(label: &str, nanos: u64) -> Record {
        Record::Decision(DecisionRow {
            ts: "2026-09-30T00:00:00Z".to_string(),
            ts_nanos: nanos,
            tool: "decide".to_string(),
            run_id: Some("run-1".to_string()),
            node_id: Some("triage".to_string()),
            question_id: "kind".to_string(),
            question_type: "choice".to_string(),
            label: label.to_string(),
            confidence: 0.93,
            band: "high".to_string(),
            margin: Some(0.89),
            near_tie: false,
            probabilities: None,
            saturated: false,
            laya_version: "0.3.5".to_string(),
            state_sha256: "abc".to_string(),
            head_sha256: None,
            backbone_sha256: None,
            bucket_clamped: false,
            latency_ms: 240,
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
        let _ = append_segment(tmp.path(), &decision("later", 200)).expect("a");
        let _ = append_segment(tmp.path(), &decision("earlier", 100)).expect("b");
        let store = tmp.path().join(STORE_FILE);
        let _ = compact(tmp.path(), &store).expect("compact");
        let ledger = Ledger::open_writer(&store).expect("reopen");
        let records = ledger.records().expect("records");
        assert_eq!(records[0].ts_nanos(), 100, "the earlier record comes first");
        assert_eq!(records[1].ts_nanos(), 200);
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
