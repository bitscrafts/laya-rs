//! The flow document: what a person writes.
//!
//! JSON, versioned. Parsing produces this typed model and nothing downstream
//! re-reads the raw JSON — `validate` and `lower` both work from [`Flow`]
//! alone. A node's kind-specific fields are optional on [`Node`] rather than
//! split into a tagged enum, because an unrecognised `kind` (an unsupported
//! value, or a typo) must still parse successfully: refusing it with a named
//! [`crate::FlowError::UnknownKind`] is `validate`'s job, not a raw
//! `serde_json` error from the parse step.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::FlowError;

/// A flow document.
///
/// `deny_unknown_fields` (spec 20): a mistyped key is refused by name rather
/// than silently ignored — a node written `depend_on` used to validate and run
/// with no dependency at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    /// The flow's name.
    pub flow: String,
    /// The document format version.
    pub version: u32,
    /// A human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared inputs. **Refused by `validate` (spec 20):** nothing binds or
    /// substitutes them — the field only exists so the refusal can name it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// Default node timeout, in seconds, inherited by every node that does
    /// not declare its own. `None` means no flow-wide deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Every node in the flow.
    pub nodes: Vec<Node>,
}

impl Flow {
    /// Parse a flow document from its canonical JSON text.
    ///
    /// # Errors
    ///
    /// Returns [`FlowError::Json`] when the text is not valid JSON, or does
    /// not match the flow document shape.
    pub fn parse(text: &str) -> Result<Self, FlowError> {
        serde_json::from_str(text).map_err(|e| refine_parse_error(&e))
    }
}

/// Turn a serde error into a [`FlowError::Json`]; an unknown-field refusal gains
/// the nearest known key (`depend_on` -> "did you mean `depends_on`?").
pub(crate) fn refine_parse_error(e: &serde_json::Error) -> FlowError {
    let msg = e.to_string();
    FlowError::Json(match suggestion(&msg) {
        Some(near) => format!("{msg} — did you mean `{near}`?"),
        None => msg,
    })
}

/// Parse serde's "unknown field `x`, expected one of `a`, `b`" text and return
/// the expected key closest to `x`, if any is close enough to be a typo.
fn suggestion(msg: &str) -> Option<String> {
    let rest = msg.split_once("unknown field `")?.1;
    let (key, rest) = rest.split_once('`')?;
    let expected = rest.split_once("expected ")?.1;
    let candidates: Vec<&str> = expected.split('`').skip(1).step_by(2).collect();
    nearest_key(key, &candidates)
}

/// The candidate within edit distance 3 (and under the key's own length) of
/// `key`, nearest first; `None` when nothing is plausibly a typo of it.
pub(crate) fn nearest_key(key: &str, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .map(|c| (edit_distance(key, c), *c))
        .filter(|(d, _)| *d <= 3 && *d < key.chars().count())
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.to_string())
}

/// Levenshtein distance, two rows.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != *cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            cur.push(sub.min(del).min(ins));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// A node's kind.
///
/// The closed set v1 implements is `shell`, `mcp`, `decide`, and the internal
/// `route` produced only by lowering (never authored by hand). Anything else
/// — including the deliberately deferred `agent` and `quorum` — parses into
/// [`Kind::Other`] so the refusal happens in `validate`, named, rather than as
/// a raw parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Runs a shell command; its exit code is the result.
    Shell,
    /// Calls an MCP tool by name.
    Mcp,
    /// Asks Laya a question and routes on the answer. Lowered away entirely
    /// before execution — see [`crate::lower`].
    Decide,
    /// Matches `on` against `rules`, first match wins. Produced only by
    /// lowering a `decide` node; never written by a flow author.
    Route,
    /// Any value that is not one of the four above, captured for the
    /// refusal message `validate` produces.
    Other(String),
}

impl Kind {
    /// The wire name for this kind.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Shell => "shell",
            Self::Mcp => "mcp",
            Self::Decide => "decide",
            Self::Route => "route",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl Serialize for Kind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "shell" => Self::Shell,
            "mcp" => Self::Mcp,
            "decide" => Self::Decide,
            "route" => Self::Route,
            _ => Self::Other(raw),
        })
    }
}

/// `{attempts, backoff_ms}`. Either both are set, or neither — `validate`
/// refuses half a policy, since that is a retry storm waiting to happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Retry {
    /// How many attempts to make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// How long to wait between attempts, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
}

/// One node in the flow.
///
/// Kind-specific fields are optional and simply unused by kinds that do not
/// need them; `validate` is what enforces which fields a given `kind`
/// requires.
///
/// `deny_unknown_fields` (spec 20): see [`Flow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Unique within the flow.
    pub id: String,
    /// What this node does.
    pub kind: Kind,
    /// Node ids that must finish before this one starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Timeout, in seconds. Refused on a `decide` until spec 17.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// The retry policy, if any. **Refused by `validate` (spec 20):** it was
    /// declared and inert; the field exists so the refusal can name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// A failure here does not fail the run.
    #[serde(default)]
    pub allow_failure: bool,
    /// Whether this node edits a repository.
    #[serde(default)]
    pub writes: bool,

    /// `shell`: the command line to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,

    /// `mcp`: the tool name to call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// `mcp`: the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,

    /// `decide`: the question to ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// `decide`: the options Laya may answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// `decide`: the state to hand Laya. **Literal text** — nothing is
    /// substituted into it.
    ///
    /// *Withdrawn 2026-10-02 (spec 20):* this comment used to give
    /// `$.inputs.issue` as an example. That feature does not exist: measured,
    /// `$.inputs.doc` is sent to the head as the literal string and the head
    /// answers it (band low, confidence 0.3985). `validate` now refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `decide`: what the routing matches — `label` (default) or `band`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    /// `decide`: answer (or `_`) to target node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<BTreeMap<String, String>>,
    /// `decide`: send a near tie to `near_tie_port` instead of its label.
    #[serde(default)]
    pub escalate_on_near_tie: bool,
    /// `decide`: where a near tie goes. Required when
    /// `escalate_on_near_tie` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_tie_port: Option<String>,
}

impl Node {
    /// A node with only `id` and `kind` set; every other field at its
    /// default. Convenient for building nodes of a given kind one field at a
    /// time with struct-update syntax.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: Kind) -> Self {
        Self {
            id: id.into(),
            kind,
            depends_on: Vec::new(),
            timeout: None,
            retry: None,
            allow_failure: false,
            writes: false,
            run: None,
            tool: None,
            arguments: None,
            question: None,
            options: None,
            prompt: None,
            on: None,
            ports: None,
            escalate_on_near_tie: false,
            near_tie_port: None,
        }
    }
}
