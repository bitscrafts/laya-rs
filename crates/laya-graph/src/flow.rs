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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flow {
    /// The flow's name.
    pub flow: String,
    /// The document format version.
    pub version: u32,
    /// A human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The inputs this flow expects.
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
        serde_json::from_str(text).map_err(|e| FlowError::Json(e.to_string()))
    }
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Unique within the flow.
    pub id: String,
    /// What this node does.
    pub kind: Kind,
    /// Node ids that must finish before this one starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Timeout, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// The retry policy, if any.
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
    /// `decide`: the state to hand Laya, e.g. `$.inputs.issue`.
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
