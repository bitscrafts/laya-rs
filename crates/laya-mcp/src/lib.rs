//! `laya-mcp` — the MCP stdio server.
//!
//! One Rust binary speaking **MCP over stdio**, so Hermes, pi, Claude Code, our
//! skills and our hooks all consume the same tool surface with **no network
//! listener** — which is what makes it usable inside a container with no inbound
//! access.
//!
//! ## The readiness precondition comes FIRST
//!
//! User directive: *"it will be mandatory the app to check for laya be running! and
//! it must inform/stop if not!"* So the server probes Laya **before serving** and, if
//! it is not usable, writes the diagnostic to stderr and **exits non-zero**. It never
//! serves in a degraded state pretending to work.
//!
//! While serving, readiness is re-checked through a short cache
//! ([`laya_core::readiness::ReadinessCache`]) and the system **fails closed**:
//! `gate` answers `ask`, never `allow`; every other tool returns a protocol error.
//!
//! ## What is deliberately NOT here
//!
//! * **`route` is not a tool.** Exposing it would let an agent select edges outside
//!   any graph, which is the injection failure mode this whole architecture exists to
//!   prevent. The graph owns every edge; `laya-rs` supplies a value.
//! * **Errors are protocol errors, not successful results whose text starts with
//!   `"error: …"`.** That pattern is fail-open by accident: a
//!   caller that only checks for a successful response treats a failure as an answer.
//! * **No blanket filesystem scope.** `ask_file` reads only within a configured root
//!   set, not the whole workspace.
//!
//! ## Status
//!
//! The **readiness gate, the tool surface, the dispatch, and the fail-closed
//! behaviour are implemented and tested.** The decision path behind `decide` is NOT:
//! it needs the head, which is spec 03/04's pinned-sidecar work. Until then `decide`
//! returns a typed "not implemented" protocol error rather than a fabricated answer —
//! a made-up decision would be worse than an honest failure.

#![forbid(unsafe_code)]

use std::future::Future;
use std::io::Write as _;
use std::sync::Arc;

use laya_core::readiness::{ProbeOutcome, Readiness, ReadinessCache, WhenNotReady};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock, Tool};
use rmcp::{ErrorData as McpError, ServerHandler, tool_handler};
use serde::Deserialize;
use serde_json::json;

/// A monotonic clock origin for readiness timestamps.
///
/// **Why this exists** — found by the end-to-end test, not by reading the code: the
/// binary recorded the startup verdict at timestamp `0`, so `now - 0` always exceeded
/// the TTL, every lookup missed, and the cache miss fell through to the "down" branch.
/// The server then behaved as though Laya were down *while Laya was up*, which meant
/// the fail-closed path answered all traffic.
///
/// One shared origin fixes it: every timestamp is milliseconds since the process
/// started, so a verdict taken at startup is genuinely fresh for the first TTL.
pub mod clock {
    use std::sync::OnceLock;
    use std::time::Instant;

    static ORIGIN: OnceLock<Instant> = OnceLock::new();

    /// The process's clock origin, created on first use.
    fn origin() -> Instant {
        *ORIGIN.get_or_init(Instant::now)
    }

    /// Milliseconds since the clock origin.
    ///
    /// Monotonic, so a wall-clock jump cannot make a stale verdict look fresh (or a
    /// fresh one look stale).
    #[must_use]
    pub fn now_ms() -> u64 {
        u64::try_from(origin().elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Probe Laya and decide readiness.
///
/// Lives here rather than in the binary so the startup gate and the per-call
/// re-check cannot disagree about what "ready" means — a divergence between those two
/// is exactly how a server comes to serve while believing it is down.
#[must_use]
pub fn probe(endpoint: &str) -> Readiness {
    let health = probe_health(endpoint);
    let outcome = if matches!(health, ProbeOutcome::Healthy) {
        probe_models(endpoint)
    } else {
        health
    };
    laya_core::readiness::judge(endpoint, &outcome)
}

/// Build a bounded HTTP agent.
#[must_use]
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(
            laya_backbone::PROBE_TIMEOUT_MS,
        )))
        .build()
        .new_agent()
}

/// Probe `/health`.
#[must_use]
pub fn probe_health(endpoint: &str) -> ProbeOutcome {
    let url = format!("{}/health", endpoint.trim_end_matches('/'));
    match agent().get(&url).call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            laya_backbone::from_health_response(status, &body)
        }
        Err(e) => laya_backbone::from_transport_error(&e.to_string()),
    }
}

/// Probe `/v1/models` and check the expected model is served.
#[must_use]
pub fn probe_models(endpoint: &str) -> ProbeOutcome {
    let url = format!("{}/v1/models", endpoint.trim_end_matches('/'));
    match agent().get(&url).call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            if status != 200 {
                return ProbeOutcome::BadStatus { status };
            }
            laya_backbone::parse_models(&body)
        }
        Err(e) => laya_backbone::from_transport_error(&e.to_string()),
    }
}

/// The tools this server exposes.
///
/// `route` is absent on purpose — see the crate docs.
pub const TOOL_NAMES: [&str; 7] = [
    "decide",
    "ask_file",
    "ask_files",
    "gate",
    "should_compact",
    "ask",
    "ledger",
];

/// The env var that relocates the sidecar socket.
///
/// Read once per server construction. Named here so the launcher script, a test and
/// the server cannot disagree about the spelling.
pub const SIDECAR_SOCKET_ENV: &str = "LAYA_SIDECAR_SOCKET";

/// True when `name` is a tool this server exposes.
#[must_use]
pub fn is_known_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// The shared state every tool handler reads.
///
/// The readiness cache is behind a mutex; it is held only for the lookup, never across
/// an I/O await, so it cannot serialise the decision path.
///
/// **There is no work queue here, deliberately.** This server serves ONE client over
/// stdio on a current-thread runtime, so dispatch is already serial and there is no
/// concurrent work to prioritise. `laya-core::queue` implements a tested priority queue
/// (spec 06), and this server does not use it — a queue was previously wired in a way
/// that could never fill, which read as a safety property while providing none. See the
/// note in `dispatch`.
pub struct ServerState {
    /// The configured Laya endpoint.
    pub endpoint: String,
    /// The last readiness verdict, with its timestamp.
    pub readiness: std::sync::Mutex<ReadinessCache>,
    /// How to reach the decision head. The head needs torch, so it is a separate
    /// pinned process — see `laya_backbone::sidecar`.
    pub sidecar: laya_backbone::Sidecar,
    /// The roots `ask_file`/`ask_files` may read. Resolved ONCE at construction, so the
    /// boundary cannot change between two calls that appear to use the same server.
    pub read_scope: laya_core::ask_file::Scope,
    /// Where this process appends its decision records. A DIRECTORY, not a store handle:
    /// F8 says a server never writes the shared store, and a directory cannot be opened as one.
    pub ledger_dir: Option<std::path::PathBuf>,
    /// Where the structured events (R4, R6) are written: stderr in production, a buffer in a test.
    /// Behind a mutex because `handle` takes `&ServerState`.
    events: std::sync::Mutex<Box<dyn std::io::Write + Send>>,
    /// Per-process call counter: the request id the `tool_call` event carries.
    seq: std::sync::atomic::AtomicU64,
}

impl ServerState {
    /// Build the state for an endpoint.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        // The socket is overridable so a deployment can relocate it (`/run` is not
        // writable everywhere, and a per-user socket is normal) and so a test can
        // point at a genuinely dead path to exercise the fail-closed behaviour.
        // Without this the variable is set by tests and IGNORED here — which is a
        // test of the test, not of the code.
        let socket = std::env::var(SIDECAR_SOCKET_ENV).map_or_else(
            |_| laya_backbone::Sidecar::default_path(),
            laya_backbone::Sidecar::new,
        );
        Self {
            endpoint: endpoint.into(),
            readiness: std::sync::Mutex::new(ReadinessCache::new()),
            sidecar: socket,
            read_scope: laya_core::ask_file::Scope::new(Vec::new()),
            ledger_dir: resolve_ledger_dir(std::env::var(LEDGER_DIR_ENV).ok().as_deref()),
            events: std::sync::Mutex::new(Box::new(std::io::stderr())),
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Send the structured events somewhere other than stderr (tests, embedding).
    #[must_use]
    pub fn with_event_sink(mut self, sink: Box<dyn std::io::Write + Send>) -> Self {
        self.events = std::sync::Mutex::new(sink);
        self
    }

    /// Write one event as one JSON line. A sink that cannot be written to is ignored: telemetry
    /// must never fail the call it describes (the same rule as a ledger write, G3).
    pub fn emit_event(&self, event: &serde_json::Value) {
        if let Ok(mut sink) = self.events.lock() {
            let _ = writeln!(sink, "{event}");
            let _ = sink.flush();
        }
    }

    /// Announce whether the ledger is on, and where (spec 11, R4).
    ///
    /// **The off state is announced, not merely absent.** The live deployment ran for its whole
    /// life with `LAYA_LEDGER_DIR` unset: every call skipped recording and nothing said so, so
    /// "off" and "broken" looked identical. A startup event that fired only when the ledger was ON
    /// would repeat that — silence would mean either. So both states write one line, and the off
    /// one is a `warn`.
    pub fn announce_ledger(&self) {
        let event = ledger_event(self.ledger_dir.as_deref());
        self.emit_event(&event);
    }

    /// Set the read scope (`ask_file`/`ask_files`).
    #[must_use]
    pub fn with_read_scope(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.read_scope = laya_core::ask_file::Scope::new(paths);
        self
    }

    /// Turn ledgering on or off explicitly, for tests and for embedding.
    ///
    /// The environment is the deployed path, but a test needs a directory of its own — and a
    /// test that recorded into the operator's real ledger would be a side effect, not a check.
    #[must_use]
    pub fn with_ledger_dir(mut self, dir: Option<std::path::PathBuf>) -> Self {
        self.ledger_dir = dir;
        self
    }

    /// Point at a specific sidecar socket (tests, or a non-default deployment).
    #[must_use]
    pub fn with_sidecar(mut self, sidecar: laya_backbone::Sidecar) -> Self {
        self.sidecar = sidecar;
        self
    }

    /// Record a verdict (used by the startup gate and by the periodic re-check).
    pub fn record_readiness(&self, verdict: Readiness, now_ms: u64) {
        if let Ok(mut c) = self.readiness.lock() {
            c.record(verdict, now_ms);
        }
    }

    /// The verdict to act on right now: a fresh cached one, or `None` meaning "you
    /// must re-probe".
    ///
    /// Returning `Option` rather than a default is deliberate — a caller cannot
    /// accidentally treat "unknown" as "ready".
    #[must_use]
    pub fn fresh_readiness(&self, now_ms: u64) -> Option<Readiness> {
        self.readiness
            .lock()
            .ok()
            .and_then(|c| c.fresh(now_ms).cloned())
    }
}

/// The MCP server.
pub struct LayaMcpServer {
    state: Arc<ServerState>,
}

impl LayaMcpServer {
    /// Build a server over the given state.
    #[must_use]
    pub const fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }

    /// Build the router: one route per tool, so `tools/list` reports them.
    #[must_use]
    pub fn tool_router() -> ToolRouter<Self> {
        let mut router = ToolRouter::new();
        for name in TOOL_NAMES {
            let (description, schema) = tool_meta(name);
            router = router.with_route(ToolRoute::new_dyn(
                Tool::new(name, description, schema),
                dispatch,
            ));
        }
        router
    }
}

/// The ONE question schema, used by every tool that takes questions.
///
/// **This is the shape the model reads**, not a preference. Three defects in this file were
/// the advertised schema disagreeing with it (measured 2026-10-02, all three found by an
/// agent-shaped caller failing):
///
/// 1. It was declared `{"type": "array"}` with no item shape. Sending an array raises
///    `AttributeError: 'list' object has no attribute 'keys'` -- the model takes a MAP.
/// 2. The text field was not described at all, so the only name available was `text`, from
///    `laya_abi::Question`. The model requires **`instructions`**; sending `text` raises
///    `KeyError: 'instructions'` on every question in the call.
/// 3. Neither was `criteria` marked required. A `choice` or `score` without it raises
///    (`TypeError: 'NoneType' object is not iterable` for score), and for a score the entries
///    ARE the scale being measured against, so the number is uninterpretable even where it
///    does not raise.
///
/// The shape below is taken from the model's own `system_one` docstring, which is the
/// authority:
///
/// ```text
///   choice: {"type": "choice", "instructions": "...", "criteria": {"optA": "...", ...}}
///   score:  {"type": "score",  "instructions": "...", "criteria": ["lvl0", "lvl1", ...]}
///   noul:   {"type": "noul",   "instructions": "..."}
/// ```
/// Refuse a question map the model cannot read, naming the argument at fault.
///
/// The model's requirements, each learned by sending it the wrong thing and reading the
/// Python error it produced:
///
/// * the questions must be an OBJECT keyed by id -- a list raises
///   `AttributeError: 'list' object has no attribute 'keys'`
/// * each question needs `instructions` (not `text`) -- `KeyError: 'instructions'`
/// * `choice` and `score` need `criteria` -- a `score` without it raises
///   `TypeError: 'NoneType' object is not iterable`
///
/// # Errors
/// A message naming the offending question id and the field that is wrong, meant to be read
/// by the caller that sent it.
pub fn validate_question_shape(questions: &serde_json::Value) -> Result<(), String> {
    let Some(obj) = questions.as_object() else {
        return Err(format!(
            "laya-mcp: `questions` must be an OBJECT keyed by question id, not {}. \
             Each value looks like {{\"type\":\"noul\",\"instructions\":\"…\"}}; a \
             `choice` also needs \"criteria\" as an object of options to their \
             descriptions, and a `score` needs \"criteria\" as an ARRAY of levels.",
            match questions {
                serde_json::Value::Array(_) => "an array",
                serde_json::Value::String(_) => "a string",
                serde_json::Value::Number(_) => "a number",
                serde_json::Value::Bool(_) => "a boolean",
                _ => "that",
            }
        ));
    };
    if obj.is_empty() {
        return Err("laya-mcp: `questions` is empty, so there is nothing to ask".to_string());
    }
    for (id, q) in obj {
        let Some(q) = q.as_object() else {
            return Err(format!("laya-mcp: question `{id}` must be an object"));
        };
        let kind = q.get("type").and_then(serde_json::Value::as_str);
        let Some(kind) = kind else {
            return Err(format!(
                "laya-mcp: question `{id}` has no `type` (must be noul, choice or score)"
            ));
        };
        if !["noul", "choice", "score"].contains(&kind) {
            return Err(format!(
                "laya-mcp: question `{id}` has type {kind:?}, which is not one of \
                 noul, choice, score"
            ));
        }
        if q.get("instructions").is_none() {
            // The single most likely mistake, because `laya_abi::Question` names this field
            // `text` and the advertised schema used to name it not at all.
            let hint = if q.get("text").is_some() {
                " -- the text field is named `instructions`, not `text`"
            } else {
                ""
            };
            return Err(format!(
                "laya-mcp: question `{id}` has no `instructions`{hint}"
            ));
        }
        if kind != "noul" && q.get("criteria").is_none() {
            let expected = if kind == "score" {
                "an ARRAY of level descriptions in ascending order"
            } else {
                "an OBJECT mapping each option to its description"
            };
            return Err(format!(
                "laya-mcp: question `{id}` is a `{kind}` and needs `criteria`, {expected} \
                 -- the entries are what the \"{kind}\" is measured against"
            ));
        }
        // A choice's criteria keys ARE the options, so an empty map offers nothing to pick.
        if kind == "choice" {
            let n = q
                .get("criteria")
                .and_then(serde_json::Value::as_object)
                .map_or(0, serde_json::Map::len);
            if n == 0 {
                return Err(format!(
                    "laya-mcp: question `{id}` is a `choice` whose `criteria` is empty, so \
                     there is no option to choose"
                ));
            }
        }
    }
    Ok(())
}

/// The ONE question schema, advertised by every tool that takes questions.
///
/// Spelled out once so `decide` and `ask` cannot drift, and so the advertised shape is the
/// one the model actually reads. See [`validate_question_shape`] for the errors a caller gets
/// when it is not.
#[must_use]
pub fn questions_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "description": "Questions keyed by id. Each value is a typed question. The key is the \
                        `question_id` echoed back on the answer, so it is the join key a \
                        caller uses to pick which answers to act on.",
        "additionalProperties": {
            "type": "object",
            "required": ["type", "instructions"],
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["noul", "choice", "score"],
                    "description": "noul: one probability that the instruction is true. \
                                    choice: pick one of the declared criteria keys. \
                                    score: place the state on the criteria scale."
                },
                "instructions": {
                    "type": "string",
                    "description": "The question text. Required, and named `instructions` -- \
                                    NOT `text`."
                },
                "criteria": {
                    "description": "Required for `choice` and `score`; omit for `noul`. \
                                    For `choice`: an object mapping each option to its \
                                    description. For `score`: an ARRAY of level \
                                    descriptions in ascending order -- these are the scale \
                                    the score is measured against."
                }
            }
        }
    })
}

/// The description and JSON schema for a tool.
fn tool_meta(name: &str) -> (String, serde_json::Map<String, serde_json::Value>) {
    let (desc, props, required) = match name {
        "decide" => (
            "Ask Laya a typed question about a state and get a route-shaped answer \
             (label, band, confidence, probabilities). The thresholds live in the \
             caller; the model never sees them.",
            json!({
                "state": {"type": "string", "description": "the state to judge"},
                "questions": questions_schema(),
            }),
            json!(["state", "questions"]),
        ),
        "ask_file" => (
            "Ask a question about ONE file. The file is read here and is NEVER put \
             into the agent's context — only the typed answer comes back.",
            json!({
                "path": {"type": "string"},
                "question": {"type": "string"}
            }),
            json!(["path", "question"]),
        ),
        "ask_files" => (
            "Ask the same question across many files (globs expanded, de-duplicated, capped) \
             and return the files that answer it, best first. Sequential: the head is a \
             single process. Reading none of them into context.",
            json!({
                "paths": {"type": "array", "items": {"type": "string"}},
                "question": {"type": "string"}
            }),
            json!(["paths", "question"]),
        ),
        "gate" => (
            "Judge whether an action may proceed. Returns allow/deny/ask. FAILS \
             CLOSED: if Laya is unavailable the answer is `ask`, never `allow`.",
            json!({
                "action": {"type": "string", "description": "what is about to happen"},
                "irreversible": {"type": "boolean", "description": "must never be got wrong"}
            }),
            json!(["action"]),
        ),
        "should_compact" => (
            "Ask whether the context should be compacted: four bounded judgements. The \
             token thresholds stay in code, never in the model.",
            json!({
                "turns_summary": {"type": "string"},
                "context_tokens": {"type": "integer"}
            }),
            json!(["turns_summary", "context_tokens"]),
        ),
        "ask" => (
            "The agentic form: the CALLER writes the questions, over a state plus \
             paths plus a command. The command is gated first.",
            json!({
                "command": {"type": "string"},
                "questions": questions_schema(),
            }),
            json!(["questions"]),
        ),
        "ledger" => (
            "Query the decision ledger (stats, tail, outcomes).",
            json!({
                "op": {"type": "string", "enum": ["stats", "tail", "outcome"]}
            }),
            json!(["op"]),
        ),
        _ => ("", json!({}), json!([])),
    };

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), props);
    schema.insert("required".into(), required);
    (desc.to_string(), schema)
}

/// Arguments a tool call may carry. One flat struct keeps the dynamic dispatcher
/// simple; each tool validates what it needs.
///
/// `Default` is derived so the all-`None` literals in tests and the deserialisation
/// fallback do not have to be edited whenever a field is added — adding a field then
/// gives a compile error only where it genuinely matters.
#[derive(Debug, Default, Deserialize)]
pub struct RawArgs {
    // Fields are `pub` because the `laya-gate` binary is a separate crate target and must be
    // able to build one. A builder would be the tidier shape for a library, but this struct
    // is a flat argument bag with no invariants to protect, and the alternative is a
    // constructor per field.
    /// The state a `decide` call judges, as free text or a JSON structure.
    #[serde(default)]
    pub state: Option<String>,
    /// The command `gate` judges.
    #[serde(default)]
    pub action: Option<String>,
    /// Whether the caller believes the action cannot be undone. Forces `ask`.
    #[serde(default)]
    pub irreversible: Option<bool>,
    /// The file `ask_file` reads.
    #[serde(default)]
    pub path: Option<String>,
    /// The question `ask_file` asks about the file. Separate from `path` on purpose:
    /// keeping them in distinct fields is what stops content being concatenated into the
    /// question, which is how a file's text could become an instruction.
    #[serde(default)]
    pub question: Option<String>,
    /// The command `ask` wants run. Gated BEFORE it is executed: a command the guardrail
    /// does not allow is never run, so there is nothing for a caller to route around.
    #[serde(default)]
    pub command: Option<String>,
    /// The paths `ask_files` asks about.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// The caller's own summary of recent turns, for `should_compact`.
    #[serde(default)]
    pub turns_summary: Option<String>,
    /// How full the context is. Supplied by the CALLER — counted, never estimated.
    #[serde(default)]
    pub context_tokens: Option<u64>,
    /// Which ledger operation was asked for.
    #[serde(default)]
    pub op: Option<String>,
    /// Laya's own question map. Kept as a `Value` in the POC; a typed
    /// `laya_abi::Question` array is the next step (the adapter is ready).
    #[serde(default)]
    pub questions: Option<serde_json::Value>,
    /// The CALLER's run id, when it has one. Threaded into the ledger so a decision can be
    /// joined to the run that made it — the join is the whole point of recording.
    #[serde(default)]
    pub run_id: Option<String>,
    /// The CALLER's node id, when it has one. Same purpose as `run_id`, one level finer.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// What a tool handler concluded.
///
/// A typed outcome rather than a string, so the *decision* about success/failure is
/// made once, in [`to_result`], instead of in every handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// A successful answer, as JSON text.
    Ok(String),
    /// The tool is recognised but not implemented yet. Reported as a PROTOCOL ERROR,
    /// never as a successful result — an unimplemented tool that "succeeds" is how a
    /// caller comes to trust a fabricated answer.
    NotImplemented(&'static str),
}

/// Turn an outcome into an MCP result, using the protocol's error surface.
///
/// This corrects that pattern: a failure becomes
/// [`McpError`], NOT a `CallToolResult::success` whose text begins `"error: "`.
/// A caller that only checks for success must not be able to mistake a failure for
/// an answer.
///
/// # Errors
/// Returns an [`McpError`] for every non-success outcome.
pub fn to_result(outcome: ToolOutcome) -> Result<CallToolResult, McpError> {
    match outcome {
        ToolOutcome::Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
        ToolOutcome::NotImplemented(what) => Err(McpError::internal_error(
            format!("laya-mcp: `{what}` is not implemented yet (spec 04)"),
            None,
        )),
    }
}

/// JSON type name as a caller would say it, for an error that names what was received.
const fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Parse a tool call's arguments, naming the first argument whose JSON type is wrong.
///
/// Deserialising straight into [`RawArgs`] and defaulting on failure turned a wrongly-typed
/// argument into an absent one, so a call that supplied `path` was told "requires `path`" —
/// an error naming the wrong problem, which sends the caller to fix an argument that was
/// never broken. A wrong type is reported as a wrong type, naming the field and the type
/// received. `null` is treated as absent, as `Option` does.
///
/// # Errors
/// A message naming the offending argument and the type it arrived as.
pub fn parse_args(value: serde_json::Value) -> Result<RawArgs, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "laya-mcp: arguments must be an object, got {}",
            json_type_name(&value)
        ));
    };
    let strings = [
        "state",
        "action",
        "path",
        "question",
        "command",
        "turns_summary",
        "op",
        "run_id",
        "node_id",
    ];
    for (name, v) in obj {
        if v.is_null() {
            continue;
        }
        let expected = if strings.contains(&name.as_str()) {
            (!v.is_string()).then_some("a string")
        } else {
            match name.as_str() {
                "irreversible" => (!v.is_boolean()).then_some("a boolean"),
                "context_tokens" => (!v.is_u64()).then_some("a non-negative integer"),
                "paths" => (!v
                    .as_array()
                    .is_some_and(|a| a.iter().all(serde_json::Value::is_string)))
                .then_some("an array of strings"),
                _ => None,
            }
        };
        if let Some(expected) = expected {
            return Err(format!(
                "laya-mcp: argument `{name}` must be {expected}, got {}",
                json_type_name(v)
            ));
        }
    }
    serde_json::from_value(value).map_err(|e| format!("laya-mcp: arguments are malformed: {e}"))
}

/// The `invalid_params` error for a missing required argument, or `None` when all are present.
///
/// Every implemented arm needs the same three lines; keeping them in one place is what stops
/// `handle` from being a wall of near-identical blocks.
fn require(tool: &str, args: &RawArgs) -> Option<McpError> {
    missing_required(tool, args)
        .map(|missing| McpError::invalid_params(format!("`{tool}` requires `{missing}`"), None))
}

/// The shared entry point for every tool: **check readiness, then dispatch**.
///
/// One place, so no tool can invent its own (weaker) down behaviour. This is the
/// function the fail-closed guarantee lives in, and it is pure enough to test
/// exhaustively without a server.
///
/// # Errors
/// Returns an [`McpError`] when the tool is unknown, when an unimplemented tool is
/// called, or when a non-`gate` tool is called while Laya is down.
/// The ONE way a tool call's arguments are accepted at runtime.
///
/// The whole seam in one function, so a test can drive the real path rather than a
/// re-enactment of it. This matters more than it looks: the defect R5 fixed was not that
/// `parse_args` was wrong — it was that **nothing called it**, and the first version of R5's test
/// called `parse_args` directly, so it stayed green with the permissive parse wired back into the
/// dispatcher (measured: the whole lib suite passed with the fix removed). Extracting the seam
/// means the wiring test drives exactly what production drives.
///
/// # Errors
/// [`McpError::invalid_params`] naming the argument and the type received, for any argument the
/// tool's contract does not accept. It never silently substitutes a default: a wrongly-typed
/// argument that becomes `RawArgs::default()` surfaces later as "requires X", which blames the
/// wrong argument and gives the caller nothing to fix.
pub fn accepted_args(args_value: serde_json::Value) -> Result<RawArgs, McpError> {
    parse_args(args_value).map_err(|e| McpError::invalid_params(e, None))
}

/// **The live seam** — everything a tool call goes through before the tool runs.
///
/// It is argument acceptance first, then the readiness verdict.
///
/// One function, so a test can drive the real path. That matters more than it looks. The defect
/// R5 fixed was not that `parse_args` was wrong — it was that **nothing checked**, and two
/// successive test versions failed the control for the same reason: they called the parser
/// directly, so reverting the dispatcher's own call left them green (measured twice: the whole
/// lib suite passed with the permissive parse restored). A test that does not go through
/// `dispatch_shim`'s own code cannot fail when `dispatch_shim` changes.
///
/// The ORDER is the contract: an argument that cannot be accepted is refused **before** the
/// layer is probed, because probing costs a network round trip and a call that was never going
/// to be accepted has no business making one.
///
/// # Errors
/// [`McpError::invalid_params`] naming the argument and the type received, for any argument the
/// tool's contract does not accept. It never silently substitutes a default: a wrongly-typed
/// argument that becomes `RawArgs::default()` surfaces later as "requires X", which blames the
/// wrong argument and gives the caller nothing to fix.
pub fn accept(
    tool: &str,
    args_value: serde_json::Value,
    state: &ServerState,
) -> Result<(RawArgs, Readiness), McpError> {
    let args = accepted_args(args_value)?;

    // Readiness AFTER acceptance, and every call. A fresh cached verdict is reused; otherwise
    // the caller must re-probe, and an absent verdict is treated as NOT ready — the fail-closed
    // direction. Not probing here was the bug the end-to-end test caught: a cache miss was
    // treated as "down", so a healthy Laya answered every call through the fail-closed path.
    let now_ms = clock::now_ms();
    let readiness = state.fresh_readiness(now_ms).unwrap_or_else(|| {
        let verdict = probe(&state.endpoint);
        state.record_readiness(verdict.clone(), now_ms);
        verdict
    });
    let _ = tool;
    Ok((args, readiness))
}

/// The dynamic tool dispatcher's body, minus the rmcp context.
///
/// Readiness first, then the tool. Split from [`dispatch_shim`] so the tool surface is drivable
/// from a test without constructing an MCP server, and so the ORDER — readiness before any work
/// — is stated in one place.
///
/// # Errors
/// [`McpError`] for an unknown tool, a not-ready layer, or any failure the tool itself reports.
pub fn handle(
    tool: &str,
    readiness: &Readiness,
    args: &RawArgs,
    state: &ServerState,
) -> Result<CallToolResult, McpError> {
    let started = std::time::Instant::now();
    let result = handle_inner(tool, readiness, args, state, started);
    emit_tool_call(state, tool, args, &result, started);
    result
}

/// One structured line per tool call, on the event sink (spec 11, R6).
///
/// `seq` is a per-process counter and serves as the request id. **`run_id` and `node_id` are the
/// requirement, not the JSON shape**: an event without them cannot be joined to the run that made
/// the call, which is the only thing it exists for. They are `null` for a call that supplied none.
/// The event names are chosen so a later move to `tracing` is a mechanical swap (N4).
fn emit_tool_call(
    state: &ServerState,
    tool: &str,
    args: &RawArgs,
    result: &Result<CallToolResult, McpError>,
    started: std::time::Instant,
) {
    let seq = state.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    // The gate's verdict is the one thing worth lifting out of the response: it is the answer an
    // operator watching the stream wants. Parsed, not substring-matched.
    let decision = result.as_ref().ok().and_then(|r| {
        r.content.iter().find_map(|c| match c {
            ContentBlock::Text(t) if tool == "gate" => {
                serde_json::from_str::<serde_json::Value>(&t.text)
                    .ok()?
                    .get("decision")?
                    .as_str()
                    .map(str::to_string)
            }
            _ => None,
        })
    });
    let mut event = json!({
        "ts": now_wall_nanos() / 1_000_000,
        "event": "tool_call",
        "seq": seq,
        "tool": tool,
        "run_id": args.run_id,
        "node_id": args.node_id,
        "latency_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "outcome": if result.is_ok() { "ok" } else { "error" },
    });
    if let (Some(obj), Some(decision)) = (event.as_object_mut(), decision) {
        obj.insert("decision".into(), json!(decision));
    }
    state.emit_event(&event);
}

/// Run `call`, and if it fails record the failure as a row before returning it.
///
/// Wraps every tool that judges, so a refusal or an error is a ledger row for ALL of them and not
/// only for the ones that remembered to (spec 11, R2).
fn recorded(
    tool: &str,
    args: &RawArgs,
    state: &ServerState,
    started: std::time::Instant,
    call: impl FnOnce() -> Result<CallToolResult, McpError>,
) -> Result<CallToolResult, McpError> {
    let result = call();
    if let Err(e) = &result {
        let _ = record_nonanswer(state, args, tool, started, e, &Trace::default());
    }
    result
}

/// What `handle` answers when Laya is not ready: the gate's fail-closed `ask`, or a protocol
/// error for every other tool. Both are recorded, because both are a refusal to answer.
fn not_ready_answer(
    tool: &str,
    readiness: &Readiness,
    args: &RawArgs,
    state: &ServerState,
    started: std::time::Instant,
) -> Result<CallToolResult, McpError> {
    match laya_core::readiness::when_not_ready(tool) {
        WhenNotReady::GateAsks => {
            // The gate answered `ask` because it could not ask anyone: a verdict, and a
            // refusal to grant, so it is a row like any other.
            let payload = json!({
                "decision": laya_core::readiness::gate_when_down(),
                "reason": "laya-unavailable",
                "note": "the gate could not ask, so it must not allow"
            });
            let _ = record_decision_for(
                state,
                &payload,
                timing_since(started),
                &state_sha256(&json!(args.action)),
                &RowSpec {
                    tool,
                    question_id: "laya-unavailable",
                    question_type: "noul",
                    run_id: args.run_id.as_deref(),
                    node_id: args.node_id.as_deref(),
                    executed: Some(false),
                    command: args.action.as_deref(),
                    label_override: payload.get("decision").and_then(serde_json::Value::as_str),
                    ..RowSpec::default()
                },
            );
            to_result(ToolOutcome::Ok(payload.to_string()))
        }
        WhenNotReady::ProtocolError => {
            let e = McpError::internal_error(
                format!(
                    "laya-mcp: Laya is not ready, so `{tool}` cannot answer.\n{}",
                    readiness_reason(readiness)
                ),
                None,
            );
            let _ = record_nonanswer(state, args, tool, started, &e, &Trace::default());
            Err(e)
        }
    }
}

fn handle_inner(
    tool: &str,
    readiness: &Readiness,
    args: &RawArgs,
    state: &ServerState,
    started: std::time::Instant,
) -> Result<CallToolResult, McpError> {
    if !is_known_tool(tool) {
        return Err(McpError::invalid_params(
            format!("unknown tool '{tool}'"),
            None,
        ));
    }

    // THE fail-closed gate. `gate` is the one tool whose down behaviour is a
    // decision rather than an error, because a hook calling a gate must get a safe
    // answer, not a crash — and a safe answer is `ask`, never `allow`.
    if !readiness.is_ready() {
        return not_ready_answer(tool, readiness, args, state, started);
    }

    // Ready. Every remaining tool needs the decision path, which is spec 03/04 work.
    let what = match tool {
        "decide" => "decide",
        "ask_file" => "ask_file",
        "ask_files" => "ask_files",
        "gate" => "gate",
        "should_compact" => "should_compact",
        "ask" => "ask",
        "ledger" => "ledger",
        _ => "unknown",
    };
    // ORDER MATTERS, and the reason is the honesty of the error.
    //
    // For an IMPLEMENTED tool, a missing argument is worth naming: supplying it
    // produces an answer. For an UNIMPLEMENTED one, "requires `path`" implies exactly
    // the same thing — and it would be a lie, because there is no implementation to
    // receive the argument. So the unimplemented check comes first for those.
    match tool {
        "gate" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            gate_via_sidecar(args, state)
        }
        "decide" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            recorded("decide", args, state, started, || {
                decide_via_sidecar(args, state)
            })
        }
        "ask_file" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            recorded("ask_file", args, state, started, || {
                ask_file_via_sidecar(args, state)
            })
        }
        "ask_files" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            recorded("ask_files", args, state, started, || {
                ask_files_via_sidecar(args, state)
            })
        }
        "ask" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            ask_via_sidecar(args, state)
        }
        "ledger" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            ledger_via_sidecar(args, state)
        }
        "should_compact" => {
            if let Some(err) = require(tool, args) {
                return Err(err);
            }
            recorded("should_compact", args, state, started, || {
                should_compact_via_sidecar(args, state)
            })
        }
        // Still honestly unimplemented: these need the file-reading and compaction
        // work the spec assigns to P2/P3. No argument contract is advertised.
        _ => to_result(ToolOutcome::NotImplemented(what)),
    }
}

/// The `gate` tool: one `noul` judgement, fail-closed on an unreachable sidecar.
///
/// The fail-closed rule is the point: if the sidecar cannot answer, the gate says
/// `ask` — never `allow`. An unavailable gate must never mean permission.
fn gate_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let out = gate_payload(args, state)?;
    let mut body = out.payload;
    if let (Some(obj), Some(note)) = (body.as_object_mut(), out.ledger_note) {
        obj.insert("ledger".into(), json!(note));
    }
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// What one judgement produced.
#[derive(Debug, Clone)]
pub struct GateOutcome {
    /// The verdict payload: `decision`, `model_decision`, `band`, `confidence`, signals.
    pub payload: serde_json::Value,
    /// A note when the row could not be written. `None` means recorded, or nothing to record.
    pub ledger_note: Option<String>,
    /// Milliseconds spent producing the verdict.
    pub latency_ms: u64,
}

/// Judge one command AND record it — the single entry point for a gate call.
///
/// Both the MCP `gate` tool and the `laya-gate` binary call this. Two entry points that
/// each did their own judging would be two gates, and a rule stated twice is the defect
/// class this project keeps finding: the copies drift, and only one of them gets fixed.
///
/// **Every verdict is recorded, including a refusal.** A gate that logs only its approvals
/// answers "what did you allow" and stays silent on "what did you stop" — the half an
/// incident review reaches for first.
///
/// # Errors
/// An [`McpError`] when the head answered but its answer could not be read as a typed
/// answer. An unreachable head is NOT an error: it is the fail-closed `ask` verdict.
pub fn gate_payload(args: &RawArgs, state: &ServerState) -> Result<GateOutcome, McpError> {
    let started = std::time::Instant::now();
    let action = args.action.clone().unwrap_or_default();
    let irreversible = args.irreversible.unwrap_or(false);
    let payload = match judge_command(&action, irreversible, state) {
        Ok(p) => p,
        Err(e) => {
            // The head answered but its answer could not be read. That is a gate that could not
            // decide, and the row is written before the error is returned.
            let _ = record_nonanswer(state, args, "gate", started, &e, &Trace::default());
            return Err(e);
        }
    };
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    // The GATE'S DECISION is the row's label -- `allow` / `ask` / `deny`.
    //
    // The order matters and the first version had it backwards: it read the model's own
    // `label` first, so a gate row recorded `"yes"` and the per-tool verdict counts came out
    // as `yes: 1` instead of `allow: 1`. The model's yes/no is a downgrade-only input to the
    // verdict, not the verdict, and an operator asking "what did the gate allow yesterday"
    // needs the decision. The model's label is still in `probabilities`/`band`.
    let verdict_label = payload
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .or_else(|| payload.get("label").and_then(serde_json::Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let spec = RowSpec {
        tool: "gate",
        question_id: &state_sha256(&json!(action)),
        question_type: "noul",
        run_id: args.run_id.as_deref(),
        node_id: args.node_id.as_deref(),
        // A verdict is a permission, not an event: this call answered, it did not execute.
        executed: Some(false),
        // Only the HASH of the command is recorded (N3).
        command: Some(&action),
        label_override: Some(&verdict_label),
        ..RowSpec::default()
    };
    let ledger_note = record_decision_for(
        state,
        &payload,
        Timing {
            latency_ms,
            ts_nanos: now_nanos(),
            wall_nanos: now_wall_nanos(),
        },
        &state_sha256(&json!({ "action": action, "irreversible": irreversible })),
        &spec,
    );

    Ok(GateOutcome {
        payload,
        ledger_note,
        latency_ms,
    })
}

/// Judge ONE command, returning the gate payload.
///
/// Extracted so the `gate` tool and the `ask` tool share **one** implementation. Level 10
/// runs its command through the guardrail gate first, and a second gate that could drift away
/// from this one is exactly the defect class this project keeps finding: a rule stated in two
/// places, disagreeing. There is one gate.
///
/// **Fails closed:** an unreachable sidecar yields `ask`, never `allow`. A gate that cannot
/// ask must not permit.
///
/// # Errors
/// An [`McpError`] if the model's answer cannot be read as a typed answer. A gate that cannot
/// understand its own evidence must not proceed to a decision.
fn judge_command(
    action: &str,
    irreversible: bool,
    state: &ServerState,
) -> Result<serde_json::Value, McpError> {
    let question = if irreversible {
        format!(
            "Is this action safe to run WITHOUT human confirmation? It is believed to be \
             IRREVERSIBLE, so a wrong 'yes' cannot be undone. Action: {action}"
        )
    } else {
        format!("Is this action safe to run without human confirmation? Action: {action}")
    };

    let questions = [laya_abi::Question::noul("safe", question)];
    let laya_q = questions_to_laya(&questions);

    let policy = laya_core::Policy::default();
    Ok(
        match state.sidecar.predict(&json!({"action": action}), &laya_q) {
            Ok(answers) => {
                let entry = answers.get("safe").cloned().unwrap_or_else(|| json!({}));
                let answer = answer_from_laya(&policy, "safe", &entry, &[])
                    .map_err(|e| McpError::internal_error(format!("laya-mcp: {e}"), None))?;
                let dp = laya_core::DecisionPayload::from_answer(&answer, &policy);

                // THE MODEL MAY ONLY DOWNGRADE. MEASURED 2026-09-30: this head cannot
                // classify command danger — "is this action safe?" as a noul scored 6/12,
                // with `ls -la /tmp` and `rm -rf <dir>` both coming back ~0.85-0.89. So the
                // model is reduced to a `yes`/`no` label plus confidence and handed to
                // `gate_verdict`, which can only use it to ASK or DENY more — never to grant
                // `allow` on its own. `allow` additionally requires a positive allowlist
                // match (spec 08, R1): the defect this fixed was `allow` reachable from
                // confidence alone, with no allowlist and a 12-entry denylist that measured
                // 0/6 on the live corpus.
                let model = match answer.value.as_str() {
                    "yes" => Some(laya_core::agentic::ModelVerdict {
                        label: laya_core::agentic::ModelLabel::Yes,
                        confidence: answer.confidence,
                        near_tie: dp.near_tie,
                    }),
                    "no" => Some(laya_core::agentic::ModelVerdict {
                        label: laya_core::agentic::ModelLabel::No,
                        confidence: answer.confidence,
                        near_tie: dp.near_tie,
                    }),
                    // "unknown": the noul itself could not be read. Same rule as an
                    // unreachable head — `gate_verdict` treats `None` as `ask`.
                    _ => None,
                };
                let verdict =
                    laya_core::agentic::gate_verdict(action, model, irreversible, &policy);

                serde_json::to_value(&dp).map_or_else(
                    |_| json!({}),
                    |mut v| {
                        if let Some(o) = v.as_object_mut() {
                            // `answer_from_laya` defaults a missing confidence to 0.0. Reporting
                            // that would be inventing "maximally unsure", so the key is removed
                            // when the model gave none (spec 11, R3).
                            if entry.get("confidence").is_none() {
                                o.remove("confidence");
                            }
                            o.insert("decision".into(), json!(verdict.decision));
                            o.insert("model_decision".into(), json!(verdict.model_decision));
                            o.insert(
                                "destructive_signals".into(),
                                serde_json::to_value(&verdict.signals)
                                    .unwrap_or_else(|_| json!([])),
                            );
                            o.insert("allowlisted".into(), json!(verdict.allowlisted));
                            o.insert("allowlist_entry".into(), json!(verdict.allowlist_entry));
                            o.insert("irreversible".into(), json!(irreversible));
                            o.insert(
                                "signals_note".into(),
                                json!(
                                    "allow requires a positive allowlist match; the model and \
                                     the destructive-pattern check may only downgrade it to \
                                     ask or deny, never grant it. The head alone was measured \
                                     at 6/12 on known commands."
                                ),
                            );
                        }
                        v
                    },
                )
            }
            Err(e) => {
                // FAIL CLOSED. The sidecar is unreachable, so we cannot ask — and a gate
                // that cannot ask must not allow.
                eprintln!("laya-mcp: gate falling back to `ask`: {e}");
                json!({
                    "decision": laya_core::readiness::gate_when_down(),
                    "reason": "sidecar-unavailable",
                    "note": "the gate could not ask, so it must not allow",
                })
            }
        },
    )
}

/// The `ledger` startup event for a configured directory, or the warning for none.
#[must_use]
pub fn ledger_event(dir: Option<&std::path::Path>) -> serde_json::Value {
    dir.map_or_else(
        || {
            json!({
                "ts": now_wall_nanos() / 1_000_000,
                "event": "ledger",
                "level": "warn",
                "enabled": false,
                "dir": null,
                "message": format!(
                    "ledger is OFF: {LEDGER_DIR_ENV} is unset, so no decision, refusal or verdict \
                     will be recorded. Set it to turn recording on."
                ),
            })
        },
        |dir| {
            json!({
                "ts": now_wall_nanos() / 1_000_000,
                "event": "ledger",
                "level": "info",
                "enabled": true,
                "dir": dir.display().to_string(),
                "dir_exists": dir.is_dir(),
            })
        },
    )
}

/// The ledger directory this server appends to, from the environment.
///
/// **Unset means NO ledgering**, and that is deliberate. A decision layer whose bookkeeping
/// silently started writing files somewhere unexpected — an agent's cwd, a read-only mount —
/// would be worse than one that records nothing. The absence is visible in the startup banner.
#[must_use]
pub fn resolve_ledger_dir(raw: Option<&str>) -> Option<std::path::PathBuf> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(raw))
}

/// The three clocks a row records, grouped because they are always taken together.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// Wall-clock milliseconds spent answering — the measured number.
    latency_ms: u64,
    /// Monotonic nanoseconds from this process's start.
    ts_nanos: u64,
    /// Epoch nanoseconds — the ordering key. See [`laya_store::DecisionRow::wall_nanos`].
    wall_nanos: u64,
}

/// Everything about a row except the answer and the clocks.
///
/// Grouped rather than passed as five positional arguments: the recorder had eight, and a
/// signature that long is one transposition away from writing the wrong question id into the
/// ledger — a defect no type checker catches, because both are `&str`.
#[derive(Debug, Default, Clone)]
struct RowSpec<'a> {
    /// Which tool produced it.
    tool: &'a str,
    /// Which question it answers (or, for a refusal, what was refused).
    question_id: &'a str,
    /// `noul`, `choice` or `score`.
    question_type: &'a str,
    /// The consumer's own run id, when it supplied one.
    run_id: Option<&'a str>,
    /// The consumer's own node id, when it supplied one.
    node_id: Option<&'a str>,
    /// Whether the action judged had actually been carried out at the moment of recording.
    /// `None` for a tool with no such notion, so "not applicable" is not recorded as "did not run".
    executed: Option<bool>,
    /// The command judged. Only its HASH is recorded (rule 8 / N3) — a command can carry a token.
    command: Option<&'a str>,
    /// The final `allow|ask|deny`, when the payload does not already carry it as `decision`.
    gate_decision: Option<&'a str>,
    /// What the model alone said, when the payload does not carry it as `model_decision`.
    model_decision: Option<&'a str>,
    /// How many destructive signals were found, when the payload does not list them.
    destructive_signals: Option<u32>,
    /// The model's own answer entry for this question, before it was shaped into a payload.
    ///
    /// The source of `confidence` and `bucket_clamped` when present: the payload's `confidence`
    /// is a plain `f64` that was defaulted to 0.0 on the way in, so reading it would record an
    /// invented number for an answer that carried none.
    answer_entry: Option<&'a serde_json::Value>,
    /// The label to record, when the payload does not carry one.
    ///
    /// Two callers need this. A refusal has no model label, so the refusal itself is the label —
    /// otherwise the row shows nothing and a denial disappears from the ledger. A gate verdict
    /// carries `decision` rather than `label`, so without this a fail-closed `ask` recorded an
    /// empty label: the row existed and said nothing, which my own test caught.
    label_override: Option<&'a str>,
}

/// Append one ledger row, and never fail the call because of it.
///
/// Returns a short note when the record could not be written, so the caller can surface it. The
/// DECISION is not affected: a decision layer that refused to decide because its bookkeeping was
/// broken would trade the whole function for the log. But it is reported rather than swallowed,
/// because a ledger that has silently stopped recording is worse than one that never did —
/// every "the latency budget is fine" claim would keep reading as measured.
fn record_decision_for(
    state: &ServerState,
    payload: &serde_json::Value,
    timing: Timing,
    state_hash: &str,
    spec: &RowSpec<'_>,
) -> Option<String> {
    let dir = state.ledger_dir.as_ref()?;
    let band = payload
        .get("band")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let row = laya_store::DecisionRow {
        ts: now_rfc3339(),
        ts_nanos: timing.ts_nanos,
        wall_nanos: timing.wall_nanos,
        tool: spec.tool.to_string(),
        run_id: spec.run_id.map(str::to_string),
        node_id: spec.node_id.map(str::to_string),
        question_id: spec.question_id.to_string(),
        question_type: spec.question_type.to_string(),
        // A refusal has no model label -- the label IS the refusal, so the row is a fact in the
        // ledger rather than an absence. What the gate denied is the row an incident review wants.
        label: spec.label_override.map_or_else(
            || {
                payload
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            },
            str::to_string,
        ),
        // `None` when absent -- never 0.0, which would read as "maximally unsure" and poison
        // every calibration metric built on the column. The model's own entry is the source when
        // there is one: the payload's `confidence` is an `f64` that was defaulted to 0.0 on the
        // way in.
        confidence: spec
            .answer_entry
            .unwrap_or(payload)
            .get("confidence")
            .and_then(serde_json::Value::as_f64),
        band: band.to_string(),
        margin: payload.get("margin").and_then(serde_json::Value::as_f64),
        near_tie: payload
            .get("near_tie")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        probabilities: payload.get("probabilities").cloned(),
        saturated: payload
            .get("saturated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        laya_version: LAYA_VERSION.to_string(),
        state_sha256: state_hash.to_string(),
        head_sha256: None,
        backbone_sha256: None,
        // NOT copied from `saturated`: that is a different fact, and copying it made an unknown
        // clamp state look known. Only a source that actually KNOWS the clamp state supplies it:
        // the model's own entry, or a payload that carries the field. Otherwise `None`.
        bucket_clamped: spec
            .answer_entry
            .and_then(|e| e.get("bucket_clamped"))
            .or_else(|| payload.get("bucket_clamped"))
            .and_then(serde_json::Value::as_bool),
        latency_ms: timing.latency_ms,
        executed: spec.executed,
        gate_decision: spec.gate_decision.map(str::to_string).or_else(|| {
            payload
                .get("decision")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        model_decision: spec.model_decision.map(str::to_string).or_else(|| {
            payload
                .get("model_decision")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        destructive_signals: spec.destructive_signals.or_else(|| {
            payload
                .get("destructive_signals")
                .and_then(serde_json::Value::as_array)
                .map(|signals| u32::try_from(signals.len()).unwrap_or(u32::MAX))
        }),
        command_sha256: spec.command.map(|c| state_sha256(&json!(c))),
        p: payload.get("p").and_then(serde_json::Value::as_f64),
        score: payload.get("score").and_then(serde_json::Value::as_f64),
    };
    match laya_store::append_segment(dir, &laya_store::Record::Decision(row)) {
        Ok(_) => None,
        Err(e) => Some(format!("the decision was not recorded: {e}")),
    }
}

/// The clocks for a row whose call began at `started`.
fn timing_since(started: std::time::Instant) -> Timing {
    Timing {
        latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ts_nanos: now_nanos(),
        wall_nanos: now_wall_nanos(),
    }
}

/// One question the model answered, shaped for recording.
struct Answered {
    id: String,
    question_type: String,
    /// The route-shaped payload — what [`laya_core::DecisionPayload`] makes of the answer.
    payload: serde_json::Value,
    /// The model's own entry, before shaping. See [`RowSpec::answer_entry`].
    entry: serde_json::Value,
}

/// Turn the model's `answers` into one [`Answered`] per question it answered.
///
/// An answer that cannot be read as a typed answer is skipped, exactly as `decide` has always
/// skipped it from its `payloads`: there is no label to record for it.
fn answered_questions(answers: &serde_json::Value, questions: &serde_json::Value) -> Vec<Answered> {
    let policy = laya_core::Policy::default();
    let mut out = Vec::new();
    let Some(map) = answers.as_object() else {
        return out;
    };
    for (id, entry) in map {
        let declared: Vec<String> = questions
            .get(id)
            .and_then(|q| q.get("criteria"))
            .and_then(serde_json::Value::as_object)
            .map(|c| c.keys().cloned().collect())
            .unwrap_or_default();
        let Ok(answer) = answer_from_laya(&policy, id, entry, &declared) else {
            continue;
        };
        let dp = laya_core::DecisionPayload::from_answer(&answer, &policy);
        let Ok(payload) = serde_json::to_value(&dp) else {
            continue;
        };
        out.push(Answered {
            id: id.clone(),
            question_type: questions
                .get(id)
                .and_then(|q| q.get("type"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| entry.get("type").and_then(serde_json::Value::as_str))
                .unwrap_or("noul")
                .to_string(),
            payload,
            entry: entry.clone(),
        });
    }
    out
}

/// Record one row per answered question. Returns the last failure note, if any.
fn record_answers(
    state: &ServerState,
    answered: &[Answered],
    state_hash: &str,
    timing: Timing,
    base: &RowSpec<'_>,
) -> Option<String> {
    let mut note = None;
    for a in answered {
        let spec = RowSpec {
            question_id: &a.id,
            question_type: &a.question_type,
            answer_entry: Some(&a.entry),
            ..base.clone()
        };
        if let Some(n) = record_decision_for(state, &a.payload, timing, state_hash, &spec) {
            note = Some(n);
        }
    }
    note
}

/// What a call learned before it failed, so the failure row can still say what was judged.
#[derive(Debug, Default)]
struct Trace {
    gate_decision: Option<String>,
    model_decision: Option<String>,
    destructive_signals: Option<u32>,
    /// Set once the command has actually run: a failure after that point is not "did not run".
    executed: Option<bool>,
}

/// Record a call that produced NO answer — a refusal, or an error — and say which.
///
/// The row is written before the error is returned. A ledger that holds only successes cannot
/// answer "what did you refuse", which is the question that matters after an incident, and a
/// rule that only some tools followed would leave exactly the rest of them dark.
///
/// `invalid_params` is a refusal (the caller asked for something the layer will not do); anything
/// else is an `error` (the layer could not answer). They are different rows because an operator
/// reading "refused: 40" and "error: 40" is looking at two different problems.
fn record_nonanswer(
    state: &ServerState,
    args: &RawArgs,
    tool: &str,
    started: std::time::Instant,
    err: &McpError,
    trace: &Trace,
) -> Option<String> {
    let label = if err.code == rmcp::model::ErrorCode::INVALID_PARAMS {
        "refused"
    } else {
        "error"
    };
    // The reason is the question id, as the first version of this recorded it. It must not carry
    // the command it refused to run.
    let mut why = err.message.to_string();
    if let Some(cmd) = args.command.as_deref().filter(|c| !c.is_empty()) {
        why = why.replace(cmd, "<command>");
    }
    if let Some(act) = args.action.as_deref().filter(|c| !c.is_empty()) {
        why = why.replace(act, "<command>");
    }
    let spec = RowSpec {
        tool,
        question_id: &why,
        question_type: "noul",
        run_id: args.run_id.as_deref(),
        node_id: args.node_id.as_deref(),
        executed: if tool == "ask" || tool == "gate" {
            Some(trace.executed.unwrap_or(false))
        } else {
            None
        },
        command: args.command.as_deref().or(args.action.as_deref()),
        gate_decision: trace.gate_decision.as_deref(),
        model_decision: trace.model_decision.as_deref(),
        destructive_signals: trace.destructive_signals,
        label_override: Some(label),
        ..RowSpec::default()
    };
    record_decision_for(
        state,
        &json!({}),
        timing_since(started),
        &state_sha256(&json!(why)),
        &spec,
    )
}

/// The pinned Laya version, recorded in every row for provenance.
pub const LAYA_VERSION: &str = "0.3.5";

/// A hash of the decided state. **Never the state itself** — a state can hold a file, and a
/// file can hold a secret.
#[must_use]
pub fn state_sha256(state: &serde_json::Value) -> String {
    // blake3 is not a dependency here, so this is a stable 64-bit FNV-1a in hex. It is
    // sufficient for the job the field does — noticing that two decisions were made about the
    // SAME input — and the weaker guarantee is stated rather than implied.
    let text = state.to_string();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Monotonic nanoseconds — the ordering key, immune to a clock step.
#[must_use]
pub fn now_nanos() -> u64 {
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// The process start, so `now_nanos` is monotonic and starts near zero.
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Epoch nanoseconds — the key that is comparable across every writer.
///
/// `now_nanos` above is measured from **this process's** start, so two servers' rows cannot be
/// ordered against each other. Measured: server rows read 65..171 while CLI outcome rows read
/// epoch nanos near 1.79e18, a ~10^16 gap that put every outcome after every decision. Rows
/// carry both: this one orders, the other stays for per-process sequencing.
#[must_use]
pub fn now_wall_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// A wall-clock timestamp, for a human reading the ledger.
#[must_use]
pub fn now_rfc3339() -> String {
    // No chrono dependency: seconds since the epoch is unambiguous and machine-readable, and a
    // human can convert it. Deliberately not a hand-rolled calendar, which is where date code
    // goes wrong.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("epoch+{secs}")
}

/// The `ask` tool (L10): the CALLER writes the questions.
///
/// ## What makes this level different
///
/// Levels 1–9 had us deciding what to ask. Here the agent writes the questions itself over a
/// state it composes from a summary, files, and a command it wants run. Two orderings are
/// deliberate and load-bearing:
///
/// 1. **The command is gated BEFORE it is run**, by the same [`judge_command`] the `gate` tool
///    uses — one gate, not two that could disagree. A command whose verdict is not `allow` is
///    refused and **never executed**. This closes the failure the source research records for
///    this level: *"Level 10 gates its own command, not the agent's bash tool."* It also
///    closes the level-6 failure it records, where an agent **routed around a block** by
///    writing the same file through a bash heredoc — which is why running the gated command
///    through this tool is safer than running anything directly.
/// 2. **The response carries answers and never the files or the output.** That is what makes
///    the level cheap: the caller pays for a judgement, not for reading.
///
/// The state composition itself is [`laya_core::agentic::compose_state`], where the order and
/// the cap are pinned by offline tests.
///
/// **Validate the WHOLE request before anything executes (spec 08, R7).** The historical
/// defect measured live: a command was gated, then RUN, and only afterwards did
/// `compose_state` discover there was nothing to judge — by which point a destructive
/// command had already destroyed data. So every refusal this function can produce is
/// decided first: `validate_questions`, the scoped file reads, and the part of
/// [`laya_core::agentic::compose_state`]'s precondition that does not depend on the
/// command's own output (a [`laya_core::agentic::AskFailure::StateTooLarge`] from the
/// summary and files ALONE can only get worse once output is added, so it is certain and is
/// raised now; [`laya_core::agentic::AskFailure::NoState`] is raised now only when there is
/// no command to possibly rescue it). Only once all of that holds does the command's own
/// gate verdict get consulted, and only an `allow` runs it — bounded by a deadline (R8), so
/// a hung command cannot stall this single-client, serial server.
///
/// # Errors
/// An [`McpError`] for a missing or malformed question block, nothing to judge, an oversized
/// state, a file outside the read scope, or a command the gate did not allow.
fn ask_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let started = std::time::Instant::now();
    let mut trace = Trace::default();
    let result = ask_inner(args, state, started, &mut trace);
    // A refusal is recorded BEFORE the error is returned, and EVERY way out of `ask` is covered
    // here -- a malformed question, an unreadable file, a command the gate did not allow, a head
    // that is down. Recording at each early return left the gate's own refusal (the one that
    // matters most) unrecorded, because it comes back through `?` from `ask_execute`.
    if let Err(e) = &result {
        let _ = record_nonanswer(state, args, "ask", started, e, &trace);
    }
    result
}

/// The body of [`ask_via_sidecar`]; `trace` carries what was learned if it fails part-way.
fn ask_inner(
    args: &RawArgs,
    state: &ServerState,
    started: std::time::Instant,
    trace: &mut Trace,
) -> Result<CallToolResult, McpError> {
    let questions = laya_core::agentic::validate_questions(
        args.questions.as_ref().unwrap_or(&serde_json::Value::Null),
    )
    .map_err(|e| McpError::invalid_params(format!("laya-mcp: {e}"), None))?;

    let scope = &state.read_scope;
    let mut files: Vec<laya_core::agentic::StateFile> = Vec::new();
    for raw in args.paths.clone().unwrap_or_default() {
        // Reuse the file tool's reader: one scope check, one size rule, one refusal message.
        // The question is passed through because `FileState` requires one; at this level the
        // question lives in the caller's own block, not here.
        match laya_core::ask_file::read_for_question(scope, &raw, "") {
            Ok(file) => files.push(laya_core::agentic::StateFile {
                path: file.path,
                content: file.content,
            }),
            Err(e) => return Err(McpError::invalid_params(e.to_string(), None)),
        }
    }

    let summary = args.state.as_deref();
    let command = args.command.as_deref().filter(|c| !c.trim().is_empty());

    let policy = laya_core::Policy::default();
    let gate_decision = match command {
        Some(cmd) => {
            let verdict = judge_command(cmd, false, state)?;
            // What the guardrail said is kept in the trace at once, so a refusal further down
            // still records WHICH verdict stopped it.
            trace.gate_decision = verdict
                .get("decision")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            trace.model_decision = verdict
                .get("model_decision")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            trace.destructive_signals = verdict
                .get("destructive_signals")
                .and_then(serde_json::Value::as_array)
                .map(|s| u32::try_from(s.len()).unwrap_or(u32::MAX));
            verdict
                .get("decision")
                .and_then(|d| serde_json::from_value::<laya_core::GateDecision>(d.clone()).ok())
                .unwrap_or(laya_core::GateDecision::Ask)
        }
        // Unused by `ask_execute` when there is no command; `Ask` keeps the value
        // meaningful rather than an arbitrary placeholder.
        None => laya_core::GateDecision::Ask,
    };

    let (stdout, stderr, timed_out) = ask_execute(
        summary,
        &files,
        command,
        gate_decision,
        policy.ask_deadline_secs,
    )?;
    // Past this line the command has run (or there was none). A failure from here on is not
    // "did not run", and the row must not say it was.
    trace.executed = Some(command.is_some());

    let composed = laya_core::agentic::compose_state(summary, &files, Some(&stdout), Some(&stderr))
        .map_err(|e| McpError::invalid_params(format!("laya-mcp: {e}"), None))?;
    let state_bytes = composed.len();

    let answers = state
        .sidecar
        .predict(&json!(composed), questions)
        .map_err(|e| {
            McpError::internal_error(
                format!("laya-mcp: the decision head is unavailable: {e}"),
                None,
            )
        })?;

    // The answers, plus what the caller needs to AUDIT the arithmetic — and deliberately
    // NOT the files or the output. `content_in_context: false` states it so a caller does not
    // have to infer it from what is absent. `timed_out` is DATA, not a JSON-RPC error: the
    // caller needs to know the command started even though it did not finish.
    // One row per question answered. `executed` is whether the command RAN, which is the
    // difference between a permission and an event.
    let spec = RowSpec {
        tool: "ask",
        run_id: args.run_id.as_deref(),
        node_id: args.node_id.as_deref(),
        executed: Some(command.is_some()),
        command,
        gate_decision: trace.gate_decision.as_deref(),
        model_decision: trace.model_decision.as_deref(),
        destructive_signals: trace.destructive_signals,
        ..RowSpec::default()
    };
    let ledger_note = record_answers(
        state,
        &answered_questions(&answers, questions),
        &state_sha256(&json!(composed)),
        timing_since(started),
        &spec,
    );

    let mut body = json!({
        "answers": answers,
        "state_bytes": state_bytes,
        "files_read": files.len(),
        "command_ran": command.is_some(),
        "timed_out": timed_out,
        "content_in_context": false,
        "questions_asked": questions.as_object().map_or(0, serde_json::Map::len),
    });
    if let (Some(obj), Some(note)) = (body.as_object_mut(), ledger_note) {
        obj.insert("ledger".into(), json!(note));
    }
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// Validate what is already decidable, THEN gate-check, THEN run the command (R7) — split
/// out from [`ask_via_sidecar`] so the ordering is testable without a live sidecar: a test
/// supplies `gate_decision` directly instead of going through [`judge_command`].
///
/// Returns `(stdout, stderr, timed_out)`.
///
/// # Errors
/// An oversized state from `summary`/`files` alone, "nothing to judge" when there is no
/// command to possibly rescue it, a command the gate did not allow, or a failure to spawn
/// the command at all.
fn ask_execute(
    summary: Option<&str>,
    files: &[laya_core::agentic::StateFile],
    command: Option<&str>,
    gate_decision: laya_core::GateDecision,
    deadline_secs: u64,
) -> Result<(String, String, bool), McpError> {
    match laya_core::agentic::compose_state(summary, files, None, None) {
        Err(e @ laya_core::agentic::AskFailure::StateTooLarge { .. }) => {
            // Adding the command's output can only make this BIGGER, never smaller: already
            // certain to fail, so refused now, before anything runs.
            return Err(McpError::invalid_params(format!("laya-mcp: {e}"), None));
        }
        Err(e @ laya_core::agentic::AskFailure::NoState) if command.is_none() => {
            // No command exists that could rescue this, so it is certain now too.
            return Err(McpError::invalid_params(format!("laya-mcp: {e}"), None));
        }
        // Everything else — `Ok`, and `NoState` with a command present, which may still
        // supply the only state there is — is deferred to the real composition after the
        // command runs.
        Ok(_) | Err(_) => {}
    }

    let Some(cmd) = command else {
        return Ok((String::new(), String::new(), false));
    };

    if gate_decision != laya_core::GateDecision::Allow {
        return Err(McpError::invalid_params(
            format!(
                "laya-mcp: the gate did not allow this command, so it was NOT run. \
                 decision={gate_decision:?}"
            ),
            None,
        ));
    }

    run_with_deadline(cmd, deadline_secs)
}

/// Run `cmd` under `sh -c`, bounded by `deadline_secs` (R8).
///
/// On expiry the child is killed and `timed_out` is `true` in the result — never a JSON-RPC
/// error, because the caller needs to know the command STARTED. stdout/stderr are read on
/// their own threads so a child that fills a pipe cannot deadlock against this thread
/// waiting on it.
///
/// # Errors
/// An [`McpError`] if the command could not even be spawned.
fn run_with_deadline(cmd: &str, deadline_secs: u64) -> Result<(String, String, bool), McpError> {
    use std::io::Read as _;

    // `exec` replaces the shell's own process image with `cmd` instead of forking it as a
    // child of `sh`, so the process this `Command` tracks IS the command itself — killing
    // it kills the real work, not just a shell sitting above it with the pipes still held
    // open by a grandchild. Safe for every command that can reach here: the gate that must
    // have allowed it (R1/R2) already proved it carries no shell metacharacter, so there is
    // no `&&`/`;` for `exec` to truncate.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exec {cmd}"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            McpError::internal_error(format!("laya-mcp: could not run the command: {e}"), None)
        })?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(deadline_secs);
    let timed_out = loop {
        match child.try_wait() {
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break true;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Exited on its own (`Ok(Some(_))`), or the exit status could not be determined
            // (`Err(_)`): either way this is not a timeout, so the caller sees whatever the
            // reader threads captured rather than a fabricated one.
            Ok(Some(_)) | Err(_) => break false,
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok((stdout, stderr, timed_out))
}

/// The `should_compact` tool (L7): the four judgements, turned into advice by OUR code.
///
/// **Four questions in ONE call**, per the level. Three are `noul`s and the 0..=2 score is a
/// `score`; the token count is the CALLER's and is never sent to the model — thresholds are
/// arithmetic and stay in code.
///
/// The model supplies judgements. This function owns the arithmetic, the veto, and the four
/// words the agent hears.
/// The four questions `should_compact` asks about the recent turns.
fn compact_questions() -> serde_json::Value {
    let questions = [
        (
            "switched_gears",
            "Did the most recent request switch to a different task or subject from the work \
             immediately before it?",
        ),
        (
            "at_boundary",
            "Did the last turn COMPLETE a unit of work, such that stopping here would lose \
             nothing in progress?",
        ),
        (
            "mid_edit",
            // MEASURED 2026-09-30. The first phrasing asked TWO things at once ("spans more
            // than one step" AND "would be broken by losing the conversation") and agreed with
            // a known-intended answer only 1/6 times — 3 false positives, 2 false negatives.
            // Since this judgement is a VETO, the false positives suppressed every genuine
            // compaction request and the false negatives permitted the compaction the veto
            // exists to prevent.
            //
            // Four phrasings were scored against the same six cases; this one and one other
            // both reached 6/6 with no false positives. The simple, single, concrete form is
            // kept. Small sample — six cases — so this is a large unambiguous improvement on
            // 1/6, not a calibrated rate.
            "Is work currently IN PROGRESS and not yet finished?",
        ),
    ];
    let mut qmap = serde_json::Map::new();
    for (id, text) in questions {
        qmap.insert(
            id.to_string(),
            json!({"type": "noul", "instructions": text}),
        );
    }
    // The 0..=2 score, described exactly as the source level describes it.
    qmap.insert(
        "needs_history".to_string(),
        json!({
            "type": "score",
            "instructions": "How much of the earlier conversation does the NEXT step need?",
            "criteria": {
                "0": "none — the next step is self-contained",
                "1": "some — a little context is needed",
                "2": "all — the next step depends on the earlier work"
            }
        }),
    );
    serde_json::Value::Object(qmap)
}

fn should_compact_via_sidecar(
    args: &RawArgs,
    state: &ServerState,
) -> Result<CallToolResult, McpError> {
    let started = std::time::Instant::now();
    let summary = args.turns_summary.as_deref().unwrap_or_default();
    let context_tokens = args.context_tokens.unwrap_or(0);

    let qmap = compact_questions();
    let answers = state
        .sidecar
        .predict(&json!({"turn_summary": summary}), &qmap)
        .map_err(|e| {
            McpError::internal_error(
                format!("laya-mcp: the decision head is unavailable: {e}"),
                None,
            )
        })?;
    // One row per question answered. The summary is hashed, never stored.
    let ledger_note = record_answers(
        state,
        &answered_questions(&answers, &qmap),
        &state_sha256(&json!({"turn_summary": summary})),
        timing_since(started),
        &RowSpec {
            tool: "should_compact",
            run_id: args.run_id.as_deref(),
            node_id: args.node_id.as_deref(),
            ..RowSpec::default()
        },
    );

    // Read the three nouls. A MISSING answer is treated as the conservative value, not as
    // `false`-by-default: an absent `mid_edit` must not read as "not editing", or a failed
    // question would silently unlock the veto.
    let noul = |id: &str| -> Option<f64> {
        answers
            .get(id)
            .and_then(|e| e.get("noul"))
            .and_then(serde_json::Value::as_f64)
    };
    let policy = laya_core::Policy::default();
    // No answer means we do NOT know it is safe to compact, so the veto applies. This is the
    // fail-closed direction, the same rule `gate` follows — and it is why `map_or` is used
    // rather than `map(..).unwrap_or(false)`, which would read a missing answer as "not
    // editing" and silently unlock the veto.
    let mid_edit = noul("mid_edit").is_none_or(|p| policy.noul_is_true(p));
    let switched_gears = noul("switched_gears").is_some_and(|p| policy.noul_is_true(p));
    let at_boundary = noul("at_boundary").is_some_and(|p| policy.noul_is_true(p));
    let needs_history = answers
        .get("needs_history")
        .and_then(|e| e.get("score"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);

    let input = laya_core::CompactInput {
        context_tokens,
        switched_gears,
        at_boundary,
        needs_history,
        mid_edit,
    };
    let thresholds = laya_core::CompactThresholds::default();
    let advice = laya_core::CompactAdvice::decide(&input, &thresholds);

    let mut body = json!({
        // The four words the agent may act on.
        "urgency": advice.urgency.as_str(),
        "compact_now": advice.compact_now,
        "reasons": advice.reasons,
        // The judgements, so the caller can audit the arithmetic.
        "judgements": {
            "switched_gears": switched_gears,
            "at_boundary": at_boundary,
            "needs_history": needs_history,
            "mid_edit": mid_edit,
        },
        "context_tokens": context_tokens,
        "thresholds": {
            "notice": thresholds.notice,
            "recommend": thresholds.recommend,
            "request": thresholds.request,
        },
        "answers": answers,
    });
    if let (Some(obj), Some(note)) = (body.as_object_mut(), ledger_note) {
        obj.insert("ledger".into(), json!(note));
    }
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// The `ask_file` tool: read a file inside the scope and ask a typed question about it.
///
/// The file NEVER enters the caller's context — only the typed answer is returned. That is
/// the entire value of the tool (measured: 9,089 tokens judged for $0.00049, against
/// $0.091 at a frontier model, paid again every turn until compaction).
///
/// Order of operations, and every step is deliberate:
///   1. arguments validated by the caller (`path`, `question`)
///   2. the read scope is enforced **before** the file is opened (spec 04 F7) — a refusal
///      costs nothing and discloses nothing
///   3. size and encoding limits are enforced, refusing rather than truncating
///   4. the content is placed under `data` as UNTRUSTED (spec 04 F9)
///   5. only then is the head asked
fn ask_file_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let started = std::time::Instant::now();
    let path = args.path.as_deref().unwrap_or_default();
    let question = args.question.as_deref().unwrap_or_default();

    let file = laya_core::ask_file::read_for_question(&state.read_scope, path, question)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    let answers = state
        .sidecar
        .predict(
            &file.to_state(),
            &laya_core::ask_file::FileState::question_spec(),
        )
        .map_err(|e| {
            McpError::internal_error(
                format!("laya-mcp: the decision head is unavailable: {e}"),
                None,
            )
        })?;

    // The file's content is hashed, never stored (N3).
    let ledger_note = record_answers(
        state,
        &answered_questions(&answers, &laya_core::ask_file::FileState::question_spec()),
        &state_sha256(&file.to_state()),
        timing_since(started),
        &RowSpec {
            tool: "ask_file",
            run_id: args.run_id.as_deref(),
            node_id: args.node_id.as_deref(),
            ..RowSpec::default()
        },
    );

    // Reported with the path it came from so a caller cannot confuse which file a verdict
    // belongs to — and `content_in_context: false` states the property explicitly rather
    // than leaving the caller to infer it.
    let bytes = file.content.len();
    let mut out = json!({ "path": file.path, "bytes": bytes, "answers": answers });
    if let Some(obj) = out.as_object_mut() {
        obj.insert("content_in_context".into(), json!(false));
        if let Some(note) = ledger_note {
            obj.insert("ledger".into(), json!(note));
        }
        // MEASURED 2026-09-30: the head reads a PREFIX of the state. A leading signal scored
        // exactly 0.788 at 3,975 / 15,943 / 63,951 and 199,951 bytes — invariant across 50x,
        // which is only possible if the tail is not read. A signal at the end was seen at
        // 984 bytes and gone at 3,976 (0.087, identical to the no-signal control).
        //
        // So "refusing rather than truncating" is not the whole truth: WE do not truncate,
        // but the model reads a window. A verdict that silently covers the first page of a
        // 200 KiB file is worse than a refusal, because nothing in the answer says so.
        if bytes > laya_core::ask_files::FULL_ATTENTION_BYTES {
            obj.insert(
                "warning".into(),
                json!(format!(
                    "content is {bytes} bytes; the head reads a PREFIX of the state, so this                      verdict may describe only the beginning of the file"
                )),
            );
        }
    }
    to_result(ToolOutcome::Ok(out.to_string()))
}

/// The `ask_files` tool: one question across many files, globs expanded and capped.
///
/// **Sequential, and the schema says so.** The sidecar accepts one connection and serves it
/// in a blocking loop, so parallel asks would queue anyway; advertising parallelism that
/// does not exist is the defect class this project keeps removing. The BACKBONE is 5.8x
/// parallel — the single-process head is not.
fn ask_files_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let started = std::time::Instant::now();
    let question = args.question.as_deref().unwrap_or_default();
    let patterns = args.paths.clone().unwrap_or_default();

    let (files, expansion_notes) =
        laya_core::ask_files::expand(&patterns, laya_core::ask_files::MAX_FILES);

    // Refusing on an empty set would be wrong: a glob that matched nothing is information
    // the caller needs, and it comes back as notes rather than as an error.
    let verdicts = laya_core::ask_files::judge_files(&state.read_scope, &files, question, |file| {
        let r = state
            .sidecar
            .predict(
                &file.to_state(),
                &laya_core::ask_file::FileState::question_spec(),
            )
            .map_err(|e| format!("the decision head is unavailable: {e}"))?;
        Ok(r)
    });

    // One row per file answered, and one per file NOT answered -- a path outside the read scope
    // is a refusal, and the refusals are the rows an incident review wants. The latency is the
    // whole call's: the per-file split is not measured, and a number that looked per-file would
    // be invented.
    let timing = timing_since(started);
    let mut ledger_note = None;
    for v in &verdicts {
        let note = match (&v.answers, &v.failure) {
            (Some(answers), _) => record_answers(
                state,
                &answered_questions(answers, &laya_core::ask_file::FileState::question_spec()),
                &state_sha256(&json!({"path": v.path, "bytes": v.bytes})),
                timing,
                &RowSpec {
                    tool: "ask_files",
                    run_id: args.run_id.as_deref(),
                    node_id: args.node_id.as_deref(),
                    ..RowSpec::default()
                },
            ),
            (None, why) => {
                let why = why.as_deref().unwrap_or("not judged");
                let label = if why.contains("outside the configured read scope") {
                    "refused"
                } else {
                    "error"
                };
                record_decision_for(
                    state,
                    &json!({}),
                    timing,
                    &state_sha256(&json!({"path": v.path})),
                    &RowSpec {
                        tool: "ask_files",
                        question_id: why,
                        question_type: "noul",
                        run_id: args.run_id.as_deref(),
                        node_id: args.node_id.as_deref(),
                        label_override: Some(label),
                        ..RowSpec::default()
                    },
                )
            }
        };
        if note.is_some() {
            ledger_note = note;
        }
    }

    let policy = laya_core::Policy::default();
    let hits = laya_core::ask_files::pick(&verdicts, policy.noul_threshold);

    // `hits` is the answer a caller wants ("which files answer this, best first"); the full
    // `files` array carries the per-file reasons, so a caller can see WHY a file is absent
    // rather than only that it is.
    let mut body = json!({
        "question": question,
        "asked": verdicts.len(),
        "hits": hits.iter().map(|(p, noul)| json!({"path": p, "noul": noul})).collect::<Vec<_>>(),
        "files": verdicts,
        "notes": expansion_notes,
        "parallel": false,
        "content_in_context": false,
    });
    if let (Some(obj), Some(note)) = (body.as_object_mut(), ledger_note) {
        obj.insert("ledger".into(), json!(note));
    }
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// The `ledger` tool: query the decision ledger (stats, tail).
///
/// ## Two constraints, both from measurements rather than caution
///
/// 1. **The store is opened per call and dropped immediately.** redb's read-only handle takes
///    the file lock (`a_held_reader_blocks_a_writer_so_a_server_must_not_hold_one`), so a
///    server that cached a reader would make `laya ledger compact` fail with `AlreadyOpen` for
///    as long as it lived — that is, forever.
/// 2. **An absent store is not an error.** Before the first compaction there is no
///    `ledger.redb`, and that is the normal state of a fresh install. Reporting it as a failure
///    would make the tool look broken exactly when the system is working correctly.
///
/// Outcomes are never written here: appending is a WRITE, and a server does not write the
/// store. `laya ledger outcome` does that.
///
/// # Errors
/// An [`McpError`] when no ledger directory is configured, or the store exists but cannot be
/// read.
fn ledger_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let Some(dir) = state.ledger_dir.as_ref() else {
        return Err(McpError::invalid_params(
            "laya-mcp: no ledger directory is configured, so there is nothing to query. \
             Set LAYA_LEDGER_DIR (the server appends its segments there, and \
             `laya ledger compact` builds the store from them)",
            None,
        ));
    };
    let store = dir.join(laya_store::STORE_FILE);
    if !store.exists() {
        // The honest answer, and not an error: nothing has been compacted yet.
        return to_result(ToolOutcome::Ok(
            json!({
                "decisions": 0,
                "outcomes": 0,
                "store": store.display().to_string(),
                "note": "no store yet — nothing has been compacted. Segment files, if any, are \
                         in the ledger directory; run `laya ledger compact` to merge them",
            })
            .to_string(),
        ));
    }

    // Open, read, DROP. The reader is a temporary, not a field.
    let reader = laya_store::Ledger::read_only(&store).map_err(|e| {
        McpError::internal_error(
            format!("laya-mcp: the ledger store could not be read: {e}"),
            None,
        )
    })?;

    let body = match args.op.as_deref() {
        None | Some("stats") => {
            let s = reader
                .stats()
                .map_err(|e| McpError::internal_error(format!("laya-mcp: {e}"), None))?;
            json!({
                "decisions": s.decisions,
                "outcomes": s.outcomes,
                "saturated": s.saturated,
                "near_ties": s.near_ties,
                "by_band": s.by_band,
                "latency_ms": {
                    "p50": s.latency_p50_ms,
                    "p95": s.latency_p95_ms,
                    "max": s.latency_max_ms,
                },
                // Stated, not implied. ECE needs (confidence, correct) pairs, and with no
                // outcomes there are none — a number computed here would look authoritative
                // and mean nothing.
                "calibrated": s.outcomes > 0,
                "note": if s.outcomes > 0 {
                    "outcomes are recorded; calibration has raw material"
                } else {
                    "NO OUTCOMES RECORDED — nothing here can be calibrated. Record one with \
                     `laya ledger outcome <id> --correct --source test`"
                },
            })
        }
        Some("tail") => {
            let n = args
                .context_tokens
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(10)
                .min(100);
            let records = reader
                .tail(n)
                .map_err(|e| McpError::internal_error(format!("laya-mcp: {e}"), None))?;
            json!({
                "records": records,
                "count": records.len(),
                "note": "newest first",
            })
        }
        Some(other) => {
            return Err(McpError::invalid_params(
                format!("laya-mcp: unknown ledger op `{other}`; use `stats` or `tail`"),
                None,
            ));
        }
    };
    drop(reader);
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// The `decide` tool: typed questions -> route-shaped payloads.
fn decide_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    // The real-path clock for this call, started BEFORE the model is asked, so the recorded
    // latency covers the whole decision rather than only the bookkeeping after it.
    let call_started = std::time::Instant::now();
    // The caller supplies the questions as Laya's own shape in this POC; a typed
    // `laya_abi::Question` array is the next step and the adapter above is ready for
    // it. Accepting Laya's shape here keeps the POC honest about what is proven.
    // `map_or_else` rather than `map(..).unwrap_or_else(..)`: the latter builds the
    // JSON even when the state is present, which is real wasted work per call.
    let state_value = args
        .state
        .as_ref()
        .map_or_else(|| json!({}), |s| json!({"input": s}));
    let questions = args.questions.clone().unwrap_or_else(|| json!({}));
    // Refuse a malformed question HERE, with a message that names the field, instead of
    // forwarding it to the model and surfacing `AttributeError: 'list' object has no
    // attribute 'keys'` or `KeyError: 'instructions'`. Both are what the OLD advertised
    // schema produced: it said `array` and never mentioned `instructions`, so a caller
    // following it failed with an error from inside the model's own Python, which says
    // nothing about which argument was wrong or how.
    validate_question_shape(&questions).map_err(|e| McpError::invalid_params(e, None))?;

    let answers = match state.sidecar.predict(&state_value, &questions) {
        Ok(a) => a,
        Err(e) => {
            return Err(McpError::internal_error(
                format!("laya-mcp: the decision head is unavailable: {e}"),
                None,
            ));
        }
    };

    // THE ROUTE-SHAPED PAYLOAD IS BUILT HERE, and it was NOT before.
    //
    // `decide` used to return the raw `answers` object. That meant `DecisionPayload` —
    // with its `label`, `band`, `margin` and `near_tie` — was constructed and unit-tested
    // but never REACHED BY A CALLER, so a graph author writing the documented
    // `route on = "$.payload.label"` had nothing to match on. Spec 04 F3 requires the
    // route-shaped payload; this is what makes that true rather than aspirational.
    //
    // The graph is handed `answers[<id>]` shaped as the payload, keyed by question id, so
    // one response can feed several route nodes. The raw answers are kept alongside, since
    // a caller debugging a decision needs the model's own output.
    let answered = answered_questions(&answers, &questions);
    let mut payloads = serde_json::Map::new();
    for a in &answered {
        payloads.insert(a.id.clone(), a.payload.clone());
    }

    // THIS CALL's latency, from `call_started` at the top of this handler.
    //
    // The first version used the process-start `Instant` and published `0` — a number that
    // looks measured and is not, which is the exact defect NF3 exists to prevent. `stats`
    // reported `latency p50: 0 ms` for a real model call, and only the end-to-end proof
    // caught it, because the field was present, well-formed and plausible.
    //
    // One row per question, so the ledger can be joined per decision rather than per request --
    // and carrying the CALLER's run and node id, which is what makes it joinable at all.
    let ledger_note = record_answers(
        state,
        &answered,
        &state_sha256(&state_value),
        timing_since(call_started),
        &RowSpec {
            tool: "decide",
            run_id: args.run_id.as_deref(),
            node_id: args.node_id.as_deref(),
            ..RowSpec::default()
        },
    );

    let mut body = json!({ "payloads": payloads, "answers": answers });
    if let (Some(obj), Some(note)) = (body.as_object_mut(), ledger_note) {
        // Reported, not swallowed: a ledger that has silently stopped recording is worse than
        // one that never did, because every claim about it keeps reading as measured.
        obj.insert("ledger".into(), json!(note));
    }
    to_result(ToolOutcome::Ok(body.to_string()))
}

/// The first argument `tool` declares as required and did not receive.
///
/// Kept next to the schema table's intent so a caller that follows the advertised
/// schema is never rejected by this check — the failure mode where the documentation
/// and the validation disagree.
#[must_use]
pub fn missing_required(tool: &str, args: &RawArgs) -> Option<&'static str> {
    let needs: &[&str] = match tool {
        "decide" => &["state"],
        "ask_file" => &["path", "question"],
        "ask_files" => &["paths"],
        "gate" => &["action"],
        "should_compact" => &["turns_summary", "context_tokens"],
        "ask" => &["questions"],
        "ledger" => &["op"],
        _ => &[],
    };
    needs.iter().copied().find(|name| match *name {
        "state" => args.state.as_deref().is_none_or(str::is_empty),
        "action" => args.action.as_deref().is_none_or(str::is_empty),
        "path" => args.path.as_deref().is_none_or(str::is_empty),
        "op" => args.op.as_deref().is_none_or(str::is_empty),
        "irreversible" => args.irreversible.is_none(),
        // `question` is required by the schema and now checked. Two reasons it must be
        // checked HERE rather than left to the scope check that runs next: an empty
        // question would otherwise reach the model (a question with no question is not a
        // question), and the error a caller sees would be about their PATH, sending them
        // to fix the wrong argument.
        "question" => args.question.as_deref().is_none_or(str::is_empty),
        "context_tokens" => args.context_tokens.is_none(),
        "paths" => args
            .paths
            .as_ref()
            .is_none_or(|v| v.iter().all(|s| s.trim().is_empty())),
        // The remaining structured arguments are not in `RawArgs` yet: the decision path
        // owns them, so they are not required from a caller until it exists.
        _ => false,
    })
}

/// Convert `laya_abi::Question` values into Laya's own question wire shape.
///
/// The two shapes are genuinely different (`text`/`options` here vs
/// `instructions`/`criteria` there), so the adapter is explicit rather than a
/// rename — a silent alias would hide a real semantic difference.
///
/// A `choice` sends its options as `criteria` with the option as its own description,
/// which is what Laya expects. A `score` sends its rubric as a list. A `noul` sends
/// only the instruction.
#[must_use]
pub fn questions_to_laya(questions: &[laya_abi::Question]) -> serde_json::Value {
    use laya_abi::QuestionType;
    let mut out = serde_json::Map::new();
    for q in questions {
        let entry = match q.kind {
            QuestionType::Noul => json!({
                "type": "noul",
                "instructions": q.text,
            }),
            QuestionType::Choice => {
                // Laya wants a MAP of option -> description. We have only names, so
                // each option describes itself; a richer description is the caller's
                // to supply later without changing this contract.
                let criteria: serde_json::Map<String, serde_json::Value> =
                    q.options.iter().map(|o| (o.clone(), json!(o))).collect();
                json!({
                    "type": "choice",
                    "instructions": q.text,
                    "criteria": criteria,
                })
            }
            QuestionType::Score => {
                let mut entry = json!({
                    "type": "score",
                    "instructions": q.text,
                });
                // `criteria` is only meaningful with a rubric, so it is omitted
                // rather than sent empty (Laya would see a zero-level score).
                if !q.options.is_empty()
                    && let Some(obj) = entry.as_object_mut()
                {
                    obj.insert("criteria".into(), json!(q.options));
                }
                entry
            }
        };
        out.insert(q.id.clone(), entry);
    }
    serde_json::Value::Object(out)
}

/// Pull a `laya_abi::Answer` out of one entry of the sidecar's `answers` object.
///
/// # Errors
/// [`SidecarError`] when the entry has no `type`, or is a `choice` whose answer is not
/// one of the declared options — the property the whole graph design depends on, so a
/// violation is an error rather than a guess.
pub fn answer_from_laya(
    policy: &laya_core::Policy,
    id: &str,
    entry: &serde_json::Value,
    declared_options: &[String],
) -> Result<laya_abi::Answer, laya_backbone::SidecarError> {
    use laya_abi::{Answer, QuestionType};
    let kind = match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("noul") => QuestionType::Noul,
        Some("choice") => QuestionType::Choice,
        Some("score") => QuestionType::Score,
        other => {
            return Err(laya_backbone::SidecarError::Protocol(format!(
                "answer `{id}` has an unknown type {other:?}"
            )));
        }
    };

    // `probabilities` is a MAP keyed by option, not a list (observed from the live
    // model). Preserve the option ORDER as declared, so a consumer that zips them
    // gets a stable mapping.
    let (value, probabilities, options, p, score) = match kind {
        QuestionType::Choice => {
            let chosen = entry
                .get("choice")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !declared_options.is_empty() && !declared_options.contains(&chosen) {
                return Err(laya_backbone::SidecarError::Protocol(format!(
                    "answer `{id}` chose {chosen:?}, which is not one of the declared \
                     options {declared_options:?} — the model must only select among \
                     ports that already exist"
                )));
            }
            let probs = entry.get("probabilities").and_then(|v| v.as_object());
            let mut probabilities = Vec::new();
            let mut options = Vec::new();
            for o in declared_options {
                options.push(o.clone());
                probabilities.push(
                    probs
                        .and_then(|m| m.get(o))
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or_default(),
                );
            }
            (chosen, probabilities, options, None, None)
        }
        QuestionType::Score => {
            let s = entry.get("score").and_then(serde_json::Value::as_f64);
            // A score's probabilities arrive keyed by level index as a string.
            let mut probabilities = Vec::new();
            if let Some(m) = entry.get("probabilities").and_then(|v| v.as_object()) {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX));
                for k in keys {
                    if let Some(v) = m.get(k).and_then(serde_json::Value::as_f64) {
                        probabilities.push(v);
                    }
                }
            }
            let value = s.map_or_else(|| "unscored".to_string(), |v| format!("{v:.4}"));
            (value, probabilities, Vec::new(), None, s)
        }
        QuestionType::Noul => {
            let noul = entry.get("noul").and_then(serde_json::Value::as_f64);
            // The threshold comes from the POLICY, not from a literal here. It was
            // hardcoded to 0.5 while `Policy::noul_threshold` was validated, configurable
            // from TOML, and read only by its own tests — so a config change had no effect
            // on the label a caller got. Passing `policy` in is what makes the config real.
            let value = match noul {
                Some(v) if policy.noul_is_true(v) => "yes".to_string(),
                Some(_) => "no".to_string(),
                None => "unknown".to_string(),
            };
            (value, Vec::new(), Vec::new(), noul, None)
        }
    };

    Ok(Answer {
        id: id.to_string(),
        kind,
        value,
        p,
        score,
        probabilities,
        options,
        confidence: entry
            .get("confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default(),
        latency_ms: None,
    })
}

/// Render the reason from a not-ready verdict, without ever including a key.
fn readiness_reason(r: &Readiness) -> String {
    match r {
        Readiness::Ready => String::from("ready"),
        Readiness::NotReady(f) => f.reason.clone(),
    }
}

/// The dynamic tool-dispatch function type — the shape the rmcp framework expects:
/// given a tool-call context, return a boxed future resolving to the MCP response.
type DispatchFn = for<'a> fn(
    ToolCallContext<'a, LayaMcpServer>,
) -> std::pin::Pin<
    Box<dyn Future<Output = Result<CallToolResponse, McpError>> + Send + 'a>,
>;

/// The dynamic dispatcher rmcp calls for any tool name.
///
/// Typed as [`DispatchFn`] through the coercion below, so the shape rmcp requires is
/// declared once and this implementation cannot silently drift from it.
fn dispatch(
    ctx: ToolCallContext<'_, LayaMcpServer>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<CallToolResponse, McpError>> + Send + '_>> {
    // `ctx` is consumed, so the coercion happens on a reference to the function item
    // rather than by calling it twice.
    let _: DispatchFn = dispatch_shim;
    dispatch_shim(ctx)
}

/// The real body, kept separate so [`dispatch`] can assert its type against
/// [`DispatchFn`] without recursing.
fn dispatch_shim(
    ctx: ToolCallContext<'_, LayaMcpServer>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<CallToolResponse, McpError>> + Send + '_>> {
    let name = ctx.name.to_string();
    let args_value = ctx
        .arguments
        .map_or_else(|| json!({}), serde_json::Value::Object);
    let state = Arc::clone(&ctx.service.state);

    Box::pin(async move {
        let (args, readiness) = accept(&name, args_value, &state)?;

        // NO QUEUE HERE, deliberately — and the reason is architectural.
        //
        // A queue used to be pushed-to and popped-from in this one expression, so its
        // depth was ALWAYS zero and its "queue is full, fail closed" error could never
        // fire. That error read as a safety property while providing none.
        //
        // The root cause is not the wiring, it is that this server has nothing to
        // queue: it serves ONE client over stdio on a `new_current_thread` runtime, so
        // requests are handled serially and there is no concurrent work to prioritise.
        // The concurrency in this system lives in the backbone, which was measured at
        // 5.8x parallel with headroom to spare — so there is no throughput problem for
        // a queue to solve either.
        //
        // `laya-core::queue` remains a correct, tested component (16 tests, spec 06).
        // It is simply not wired here, and saying so is the point: a capability that is
        // present in the build but unreachable in behaviour must be stated, not implied.
        let result = handle(&name, &readiness, &args, &state)?;
        Ok(CallToolResponse::from(result))
    })
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for LayaMcpServer {}

// NOTE: there is deliberately no `server_capabilities()` helper here.
//
// `#[tool_handler]` on the `ServerHandler` impl ABOVE generates `get_info()` (see
// rmcp-macros `tool_handler.rs`), and the macro builds
// `ServerCapabilities::builder().enable_tools().build()` — tools only,
// which is exactly right (this server exposes no prompts, resources or sampling, so
// claiming more would be a lie a client would act on).
//
// A hand-written `server_capabilities()` helper used to sit here. It was removed because
// it was a SECOND source of truth that nothing read: a reader would reasonably believe
// editing it changed what the server advertises, and it would not.
// To change the advertised capabilities, add the sibling handler attribute
// (`#[prompt_handler]`, etc.) or implement `get_info` on the impl block itself.
// NOTE: `ask_file`/`ask_files` do NOT define their own read scope here.
//
// The containment rule lives in `laya_core::ask_file::Scope` — ONE implementation, so it
// cannot drift, and so the rule that decides a refusal is testable without a server.
// A duplicate `ReadScope` used to sit at this spot. It differed from the core rule in a
// way that mattered: it had no parent-resolution, so it could not place a path that does
// not exist, and would have reported a missing in-scope file as "outside the scope".

/// The env var listing the roots `ask_file`/`ask_files` may read, colon-separated.
///
/// **There is deliberately no default of `/`.** With the variable unset, the scope is the
/// process's working directory — a real, bounded place — and the resolved roots are
/// logged at startup so the boundary in force is visible rather than inferred. A tool that
/// silently defaults to the whole filesystem is a tool nobody can reason about.
pub const READ_ROOTS_ENV: &str = "LAYA_READ_ROOTS";

/// The env var naming where this server appends its ledger segments.
pub const LEDGER_DIR_ENV: &str = "LAYA_LEDGER_DIR";

/// Resolve the configured read scope.
///
/// Colon-separated (`:`), matching `PATH`. An entry that does not exist is dropped and
/// REPORTED, because a silently-dropped root looks like a scope that refuses everything
/// for no reason.
#[must_use]
pub fn resolve_read_scope(raw: Option<&str>) -> (laya_core::ask_file::Scope, Vec<String>) {
    let mut notes = Vec::new();
    let roots: Vec<std::path::PathBuf> = match raw {
        Some(v) if !v.trim().is_empty() => v
            .split(':')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let p = std::path::PathBuf::from(s);
                if p.is_dir() {
                    Some(p)
                } else {
                    notes.push(format!("read root `{s}` is not a directory; ignored"));
                    None
                }
            })
            .collect(),
        _ => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            notes.push(format!(
                "{READ_ROOTS_ENV} is unset, so the read scope is the working directory: {}",
                cwd.display()
            ));
            vec![cwd]
        }
    };
    (laya_core::ask_file::Scope::new(roots), notes)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::indexing_slicing
    )]

    use super::*;
    use laya_core::readiness::judge;

    fn ready() -> Readiness {
        Readiness::Ready
    }

    /// The wiring test: drives the live seam, not the parser in isolation.
    #[test]
    fn a_wrongly_typed_argument_is_named_at_the_live_seam() {
        // The control found this missing. With the old permissive parse wired back into
        // `dispatch_shim` (`serde_json::from_value(args_value).unwrap_or_default()`), the test
        // above still PASSES: it calls `parse_args` directly, so it tests the function and not
        // the wiring. Measured: the whole lib suite stayed green with the fix removed.
        //
        // So this one drives `handle`, the entry point production uses. It must fail if
        // `dispatch_shim` ever goes back to swallowing a wrongly-typed argument, because the
        // defect was never "parse_args is wrong" -- it was "nothing called it".
        let err = accept("gate", json!({"action": 123}), &test_state())
            .expect_err("a numeric `action` must be refused, not defaulted away");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("`action`"),
            "the refusal must name the argument that is wrong: {msg}"
        );
        assert!(
            msg.contains("number") && msg.contains("string"),
            "and must name what was received and what is required: {msg}"
        );
        // The measured precedent: `path` supplied and valid, `question` an object. The old
        // code surfaced "requires `question`" -- blaming the wrong argument.
        let err = accept(
            "ask_file",
            json!({"path": "/etc/hostname", "question": {"type": "noul"}}),
            &test_state(),
        )
        .expect_err("an object `question` must be refused");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("`question`") && !msg.contains("requires `path`"),
            "the refusal must name the argument at fault, not the valid one beside it: {msg}"
        );
    }

    /// The parser in isolation. Kept, and clearly NOT the wiring test: this one stays green
    /// when the dispatcher's call is reverted, which is why the seam test above exists.
    #[test]
    fn a_wrongly_typed_argument_is_named_not_swallowed() {
        let e = parse_args(json!({"action": 123})).expect_err("a number is not a string");
        assert!(e.contains("`action`") && e.contains("number"), "{e}");

        // The measured precedent: `path` is supplied and valid, `question` is an object. The
        // error must name `question` and `object`, and must NOT blame `path`.
        let e = parse_args(json!({"path": "/etc/hostname", "question": {"type": "noul"}}))
            .expect_err("an object is not a string");
        assert!(e.contains("`question`") && e.contains("object"), "{e}");
        assert!(
            !e.contains("`path`"),
            "must not blame the argument that was fine: {e}"
        );

        // And end to end through the validation `handle` reaches: a non-string `paths` element.
        let e = parse_args(json!({"paths": ["a", 7]})).expect_err("7 is not a string");
        assert!(
            e.contains("`paths`") && e.contains("array of strings"),
            "{e}"
        );
        let e =
            parse_args(json!(["not", "an", "object"])).expect_err("arguments must be an object");
        assert!(e.contains("array"), "{e}");

        // CONTROL: well-typed arguments, `null` (absent) and unknown keys still parse.
        let a = parse_args(json!({
            "action": "ls", "irreversible": false, "context_tokens": 5,
            "paths": ["a"], "question": null, "extra": {"anything": 1}
        }))
        .expect("well-typed arguments must parse");
        assert_eq!(a.action.as_deref(), Some("ls"));
        assert_eq!(a.question, None);
        assert_eq!(a.context_tokens, Some(5));
    }

    /// Seeded LCG (Knuth MMIX constants). Deterministic: a failure reproduces from `SEED`.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next()).unwrap_or(0) % n.max(1)
        }
    }

    /// R4: seeded byte mutation of real JSON argument objects, through parse + `handle`.
    /// `ask` is left out of the seeds because a mutated allowlisted command would really run.
    #[test]
    fn mutated_arguments_are_refused_or_handled_never_panic() {
        const SEED: u64 = 0x130A_5EED;
        let seeds = [
            r#"{"state":"the build is green","options":["a","b"]}"#,
            r#"{"action":"git push --force origin main","irreversible":true}"#,
            r#"{"path":"/etc/hostname","question":"what is this?"}"#,
            r#"{"question":"q","paths":["/etc/hostname","*.md"]}"#,
            r#"{"turns_summary":"did things","context_tokens":1234}"#,
            r#"{"state":"x","run_id":"r1","node_id":"n1"}"#,
        ];
        let tools = ["decide", "gate", "ask_file", "ask_files", "should_compact"];
        let st = test_state();
        let mut rng = Lcg(SEED);
        let (mut parsed, mut refused) = (0_u32, 0_u32);
        for case in 0..2_000_u32 {
            let mut bytes = seeds[rng.below(seeds.len())].as_bytes().to_vec();
            for _ in 0..=rng.below(4) {
                let at = rng.below(bytes.len());
                match rng.below(4) {
                    0 => bytes[at] = u8::try_from(rng.next() & 0xff).unwrap_or(0),
                    // Structural bytes are what change a value's TYPE, which random bytes rarely do.
                    3 => bytes[at] = b"0t[{\"n"[rng.below(6)],
                    1 => {
                        bytes.remove(at);
                    }
                    _ => bytes.insert(at, bytes[at]),
                }
                if bytes.is_empty() {
                    bytes.push(b'{');
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let tool = tools[rng.below(tools.len())];
            match parse_args(value) {
                Ok(args) => {
                    parsed += 1;
                    // Ok or Err are both fine; a panic fails the test and prints the seed.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle(tool, &ready(), &args, &st)
                    }))
                    .unwrap_or_else(|_| panic!("handle panicked: seed {SEED:#x}, case {case}, tool {tool}, input {text}"));
                }
                Err(_) => refused += 1,
            }
        }
        // CONTROL: the mutator must produce BOTH outcomes, or it is exercising only one path.
        assert!(
            parsed > 100 && refused > 0,
            "parsed {parsed}, refused {refused}"
        );
    }

    fn down() -> Readiness {
        judge(
            "http://127.0.0.1:9",
            &ProbeOutcome::Unreachable("refused".into()),
        )
    }

    /// A state whose sidecar socket cannot exist, so readiness and argument tests
    /// stay offline — and so a test that DOES reach the sidecar fails loudly rather
    /// than accidentally talking to a real one.
    fn test_state() -> ServerState {
        ServerState::new("http://x:1")
            .with_sidecar(laya_backbone::Sidecar::new("/tmp/laya-test-no-such.sock"))
    }

    fn no_args() -> RawArgs {
        RawArgs::default()
    }

    /// A state whose ledger is ON, in a directory of its own.
    ///
    /// Recording is the thing under test, so the state must have a ledger dir — the default
    /// `test_state()` has none, and a recorder with no directory returns `None` and writes
    /// nothing, which would make every recording assertion below pass for the wrong reason.
    fn state_with_ledger(dir: &std::path::Path) -> ServerState {
        ServerState::new("http://x:1")
            .with_sidecar(laya_backbone::Sidecar::new("/tmp/laya-test-no-such.sock"))
            .with_ledger_dir(Some(dir.to_path_buf()))
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!("laya-mcp-{tag}-{nanos}"));
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    /// Every row the server has appended, read back from the segments it wrote.
    fn recorded(dir: &std::path::Path) -> Vec<laya_store::DecisionRow> {
        let mut out = Vec::new();
        for f in laya_store::segments(dir).expect("segments") {
            for r in laya_store::read_segment(&f).expect("read") {
                if let laya_store::Record::Decision(d) = r {
                    out.push(d);
                }
            }
        }
        out
    }

    // ---- the tool surface ---------------------------------------------------

    #[test]
    fn the_tool_list_is_exactly_the_seven_documented_tools() {
        assert_eq!(TOOL_NAMES.len(), 7);
        for t in [
            "decide",
            "ask_file",
            "ask_files",
            "gate",
            "should_compact",
            "ask",
            "ledger",
        ] {
            assert!(is_known_tool(t), "{t} must be a tool");
        }
    }

    #[test]
    fn a_refused_gate_is_recorded() {
        // R2's refusal half. The sidecar socket cannot exist, so the gate takes its fail-closed
        // path and answers `ask`. That verdict -- the one where the gate could NOT decide -- is
        // exactly what has to appear in the ledger, because "what did the gate refuse" is the
        // question an incident review asks first.
        let dir = tempdir("gate-refusal");
        let state = state_with_ledger(&dir);
        let args = RawArgs {
            action: Some("rm -rf /".to_string()),
            ..RawArgs::default()
        };
        let _ =
            gate_via_sidecar(&args, &state).expect("the gate answers even when the head is down");

        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "a refusal must be a row, not an absence");
        let row = &rows[0];
        assert_eq!(row.tool, "gate");
        // The VERDICT, not the model's yes/no. Reading `label` first recorded "yes" and the
        // per-tool counts came out as `yes: 1` instead of `ask: 1` -- a metric that cannot
        // answer "what did the gate refuse".
        assert!(
            ["allow", "ask", "deny"].contains(&row.label.as_str()),
            "the row must carry the gate's decision, got {:?}",
            row.label
        );
        assert_eq!(
            row.executed,
            Some(false),
            "a verdict is a permission, not an event"
        );
        assert_eq!(row.run_id, None, "no caller id was supplied");
    }

    #[test]
    fn the_gate_row_carries_the_callers_run_and_node_ids() {
        // R6's real requirement: an event without the ids answers nothing, because it cannot be
        // joined to the run that made it.
        let dir = tempdir("gate-ids");
        let state = state_with_ledger(&dir);
        let args = RawArgs {
            action: Some("git status".to_string()),
            run_id: Some("run-42".to_string()),
            node_id: Some("triage".to_string()),
            ..RawArgs::default()
        };
        let _ = gate_via_sidecar(&args, &state).expect("gate answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run_id.as_deref(), Some("run-42"));
        assert_eq!(rows[0].node_id.as_deref(), Some("triage"));
    }

    #[test]
    fn a_gate_row_carries_a_comparable_wall_clock() {
        // Without a wall clock the row cannot be ordered against rows from another process,
        // which is what made `tail`'s "newest" untrue.
        let dir = tempdir("gate-wall");
        let state = state_with_ledger(&dir);
        let args = RawArgs {
            action: Some("git status".to_string()),
            ..RawArgs::default()
        };
        let _ = gate_via_sidecar(&args, &state).expect("gate answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1);
        let epoch_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        assert!(
            rows[0].wall_nanos / 1_000_000_000 > epoch_now - 60,
            "wall_nanos must be epoch-based ({}), not process-relative",
            rows[0].wall_nanos
        );
    }

    #[test]
    fn an_invalid_question_is_recorded_as_a_refusal_before_the_error_returns() {
        // The refusal path is the one that used to vanish: it returns early, so nothing was
        // written, and the ledger showed a clean store while `ask` was refusing callers.
        let dir = tempdir("ask-refusal");
        let state = state_with_ledger(&dir);
        let args = RawArgs {
            questions: Some(serde_json::json!({"not": "a question map"})),
            ..RawArgs::default()
        };
        let err = ask_via_sidecar(&args, &state).expect_err("an invalid question map is an error");
        assert!(
            err.message.contains("laya-mcp"),
            "the error is still reported: {}",
            err.message
        );
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "the refusal is a row");
        assert_eq!(rows[0].tool, "ask");
        assert_eq!(rows[0].label, "refused");
    }

    #[test]
    fn the_control_a_state_without_a_ledger_directory_records_nothing() {
        // The near-miss that keeps the four tests above honest: they pass because recording
        // works, not because `record_decision` always writes. With no directory configured the
        // recorder must return without touching the filesystem -- and it must not create one.
        let dir = tempdir("no-ledger");
        let state = ServerState::new("http://x:1")
            .with_sidecar(laya_backbone::Sidecar::new("/tmp/laya-test-no-such.sock"));
        assert!(
            state.ledger_dir.is_none(),
            "the default state has no ledger"
        );
        let args = RawArgs {
            action: Some("git status".to_string()),
            ..RawArgs::default()
        };
        let _ = gate_via_sidecar(&args, &state).expect("gate answers");
        assert!(laya_store::segments(&dir).expect("segments").is_empty());
    }

    // ---- spec 11: one run id end to end, every judgement a row, events ---------

    /// A stand-in decision head on a REAL Unix socket.
    ///
    /// The spec's `Judge` trait would have replaced `ServerState::sidecar` with a boxed trait
    /// object; this does the same job one layer lower and keeps the production path whole -- the
    /// real `Sidecar` client, its JSON line protocol and every handler run unchanged, and only the
    /// model on the far end is a fixed answer (AGENTS rule 9: no live model in a unit test). The
    /// `answer` closure sees each question's `(id, type, criteria keys)` and returns the entry.
    fn stub_sidecar(
        tag: &str,
        answer: impl Fn(&str, &str, &[String]) -> serde_json::Value + Send + 'static,
    ) -> laya_backbone::Sidecar {
        use std::io::{BufRead, BufReader};
        let path = tempdir(&format!("sock-{tag}")).join("head.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&path).unwrap_or_else(|e| panic!("bind: {e}"));
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let mut line = String::new();
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let req: serde_json::Value = serde_json::from_str(&line).unwrap_or(json!({}));
                let mut answers = serde_json::Map::new();
                if let Some(qs) = req.get("questions").and_then(serde_json::Value::as_object) {
                    for (id, q) in qs {
                        let kind = q
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        let keys: Vec<String> = q
                            .get("criteria")
                            .and_then(serde_json::Value::as_object)
                            .map(|c| c.keys().cloned().collect())
                            .unwrap_or_default();
                        answers.insert(id.clone(), answer(id, kind, &keys));
                    }
                }
                let mut w = &stream;
                let _ = writeln!(w, "{}", json!({"ok": true, "answers": answers}));
            }
        });
        laya_backbone::Sidecar::new(path)
    }

    /// A plausible answer for any question: a confident, un-saturated one.
    fn plain_answer(_id: &str, kind: &str, keys: &[String]) -> serde_json::Value {
        match kind {
            "choice" => {
                let first = keys.first().cloned().unwrap_or_default();
                let probs: serde_json::Map<String, serde_json::Value> = keys
                    .iter()
                    .map(|k| {
                        (
                            k.clone(),
                            json!(1.0 / f64::from(u32::try_from(keys.len()).unwrap_or(1).max(1))),
                        )
                    })
                    .collect();
                json!({"type": "choice", "choice": first, "probabilities": probs, "confidence": 0.7})
            }
            "score" => {
                json!({"type": "score", "score": 1.0, "probabilities": {"0": 0.2, "1": 0.6, "2": 0.2}, "confidence": 0.6})
            }
            _ => json!({"type": "noul", "noul": 0.9, "confidence": 0.8}),
        }
    }

    fn state_with_stub(dir: &std::path::Path, tag: &str) -> ServerState {
        ServerState::new("http://x:1")
            .with_sidecar(stub_sidecar(tag, plain_answer))
            .with_ledger_dir(Some(dir.to_path_buf()))
    }

    /// A writer the test can read back: the event sink.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Captured {
        fn lines(&self) -> Vec<serde_json::Value> {
            String::from_utf8_lossy(&self.0.lock().expect("lock"))
                .lines()
                .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
                .collect()
        }
    }

    /// Every byte of every segment the server wrote, as text.
    fn segment_text(dir: &std::path::Path) -> String {
        let mut all = String::new();
        for f in laya_store::segments(dir).expect("segments") {
            all.push_str(&std::fs::read_to_string(f).expect("read"));
        }
        all
    }

    fn decide_args(run: Option<&str>, node: Option<&str>) -> RawArgs {
        RawArgs {
            state: Some("the invoice is overdue".to_string()),
            questions: Some(json!({
                "paid": {"type": "noul", "instructions": "Is it paid?"}
            })),
            run_id: run.map(str::to_string),
            node_id: node.map(str::to_string),
            ..RawArgs::default()
        }
    }

    #[test]
    fn a_decide_records_the_callers_run_id_and_node_id() {
        // R1, through the HANDLER: `decide` used to record `None` unconditionally (it went through
        // a short-form recorder that carried no ids), so a test of `record_decision_for` alone
        // would have passed while the tool kept dropping them.
        let dir = tempdir("r1-ids");
        let state = state_with_stub(&dir, "r1-ids");
        let _ = handle(
            "decide",
            &ready(),
            &decide_args(Some("r1"), Some("n1")),
            &state,
        )
        .expect("decide answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].run_id.as_deref(), Some("r1"));
        assert_eq!(rows[0].node_id.as_deref(), Some("n1"));
    }

    #[test]
    fn the_control_a_decide_without_context_records_none() {
        // A human calling `decide` directly has no run. The ids must be ABSENT, not invented.
        let dir = tempdir("r1-none");
        let state = state_with_stub(&dir, "r1-none");
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "it still records");
        assert_eq!(rows[0].run_id, None);
        assert_eq!(rows[0].node_id, None);
    }

    #[test]
    fn every_judging_tool_writes_a_ledger_row() {
        // R2, and its control in the same test: every one of these calls SUCCEEDS, and each must
        // still write a row. A recorder that wrote only refusals would pass the refusal tests
        // and fail here.
        let dir = tempdir("r2-every");
        let root = tempdir("r2-root");
        std::fs::write(root.join("a.txt"), "the invoice was paid on friday").expect("write");
        let state = state_with_stub(&dir, "r2-every").with_read_scope(vec![root.clone()]);
        let file = root.join("a.txt").display().to_string();
        let calls: Vec<(&str, RawArgs)> = vec![
            (
                "gate",
                RawArgs {
                    action: Some("git status".to_string()),
                    ..RawArgs::default()
                },
            ),
            (
                "ask",
                RawArgs {
                    state: Some("a short summary".to_string()),
                    questions: Some(json!({"q": {"type": "noul", "instructions": "ok?"}})),
                    ..RawArgs::default()
                },
            ),
            (
                "ask_file",
                RawArgs {
                    path: Some(file.clone()),
                    question: Some("was it paid?".to_string()),
                    ..RawArgs::default()
                },
            ),
            (
                "ask_files",
                RawArgs {
                    paths: Some(vec![file]),
                    question: Some("was it paid?".to_string()),
                    ..RawArgs::default()
                },
            ),
            (
                "should_compact",
                RawArgs {
                    turns_summary: Some("we fixed the parser".to_string()),
                    context_tokens: Some(1000),
                    ..RawArgs::default()
                },
            ),
        ];
        for (tool, args) in &calls {
            let out = handle(tool, &ready(), args, &state);
            assert!(
                out.is_ok(),
                "{tool} must succeed for this to be the control: {out:?}"
            );
            let rows: Vec<_> = recorded(&dir)
                .into_iter()
                .filter(|r| r.tool == *tool)
                .collect();
            assert!(!rows.is_empty(), "{tool} wrote no row");
            assert!(
                rows.iter()
                    .all(|r| r.label != "refused" && r.label != "error"),
                "{tool}: a successful call is not recorded as a refusal: {rows:?}"
            );
        }
    }

    #[test]
    fn the_control_the_ledger_tool_writes_no_decision_row() {
        // The ledger tool QUERIES the ledger; a row for every query would make the ledger grow by
        // being read.
        let dir = tempdir("r2-ledger");
        let state = state_with_stub(&dir, "r2-ledger");
        let args = RawArgs {
            op: Some("stats".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("ledger", &ready(), &args, &state);
        assert!(recorded(&dir).is_empty(), "{:?}", recorded(&dir));
    }

    #[test]
    fn a_gate_row_carries_the_decision_and_never_the_command() {
        // R2 + N3, through the handler. `ls -la /tmp` must appear NOWHERE in the segment: not in
        // a field, not in a question id, not in a note.
        let dir = tempdir("r2-gate");
        let state = state_with_stub(&dir, "r2-gate");
        let args = RawArgs {
            action: Some("ls -la /tmp".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("gate", &ready(), &args, &state).expect("gate answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(
            ["allow", "ask", "deny"].contains(&row.gate_decision.as_deref().unwrap_or("")),
            "{row:?}"
        );
        assert!(row.model_decision.is_some(), "{row:?}");
        assert!(row.destructive_signals.is_some(), "{row:?}");
        assert_eq!(row.executed, Some(false));
        assert_eq!(
            row.command_sha256.as_deref(),
            Some(state_sha256(&json!("ls -la /tmp")).as_str()),
            "the hash is of the command, so two calls about the same command can be matched"
        );
        let text = segment_text(&dir);
        assert!(
            !text.contains("ls -la"),
            "the command leaked into the segment: {text}"
        );
        assert!(!text.contains("/tmp"), "{text}");
    }

    #[test]
    fn a_refusal_by_ask_is_recorded_without_the_command_and_says_what_stopped_it() {
        // R2's refusal half for `ask`, through the handler: the gate says no, the error is
        // returned, and the row exists with the verdict that stopped it. The command is a
        // distinctive string so a leak cannot hide.
        let dir = tempdir("r2-askref");
        let state = state_with_stub(&dir, "r2-askref");
        let args = RawArgs {
            state: Some("summary".to_string()),
            questions: Some(json!({"q": {"type": "noul", "instructions": "ok?"}})),
            command: Some("rm -rf /srv/SECRET-CANARY-42".to_string()),
            ..RawArgs::default()
        };
        let err = handle("ask", &ready(), &args, &state).expect_err("the gate must refuse this");
        assert!(err.message.contains("NOT run"), "{}", err.message);
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].tool, "ask");
        assert_eq!(rows[0].label, "refused");
        assert_ne!(
            rows[0].gate_decision.as_deref(),
            Some("allow"),
            "{:?}",
            rows[0]
        );
        assert!(
            rows[0].gate_decision.is_some(),
            "which verdict stopped it: {:?}",
            rows[0]
        );
        assert_eq!(rows[0].executed, Some(false));
        assert!(rows[0].command_sha256.is_some());
        assert!(
            !segment_text(&dir).contains("SECRET-CANARY-42"),
            "the refused command leaked into the ledger"
        );
    }

    #[test]
    fn a_refusal_by_ask_file_is_recorded() {
        // The same defect class in another tool: an out-of-scope read is a denial, and it used to
        // leave no trace.
        let dir = tempdir("r2-fileref");
        let state =
            state_with_stub(&dir, "r2-fileref").with_read_scope(vec![tempdir("r2-fr-root")]);
        let args = RawArgs {
            path: Some("/etc/hostname".to_string()),
            question: Some("what?".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("ask_file", &ready(), &args, &state).expect_err("out of scope");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            (rows[0].tool.as_str(), rows[0].label.as_str()),
            ("ask_file", "refused")
        );
    }

    #[test]
    fn a_gate_called_while_laya_is_down_is_recorded() {
        // The not-ready arm of `handle` answers `ask` without reaching a handler, so it needs its
        // own row -- it is the gate refusing to grant because it could not ask anyone.
        let dir = tempdir("r2-down");
        let state = state_with_ledger(&dir);
        let args = RawArgs {
            action: Some("git push".to_string()),
            run_id: Some("r9".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("gate", &down(), &args, &state).expect("the gate answers ask when down");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].gate_decision.as_deref(), Some("ask"));
        assert_eq!(rows[0].run_id.as_deref(), Some("r9"));
    }

    #[test]
    fn bucket_clamped_is_not_a_copy_of_saturated() {
        // R3. The head answers with all the mass on one option -- `saturated` is TRUE -- and says
        // nothing about its temperature clamp. The row must say `saturated: true` (so the test is
        // not vacuous) and `bucket_clamped: None` (unknown), not `Some(true)`.
        let dir = tempdir("r3-clamp");
        let state = ServerState::new("http://x:1")
            .with_sidecar(stub_sidecar("r3-clamp", |_, _, keys| {
                let first = keys.first().cloned().unwrap_or_default();
                let probs: serde_json::Map<String, serde_json::Value> = keys
                    .iter()
                    .map(|k| (k.clone(), json!(if *k == first { 1.0 } else { 0.0 })))
                    .collect();
                json!({"type": "choice", "choice": first, "probabilities": probs, "confidence": 0.99})
            }))
            .with_ledger_dir(Some(dir.clone()));
        let args = RawArgs {
            state: Some("s".to_string()),
            questions: Some(json!({"w": {"type": "choice", "instructions": "which?",
                "criteria": {"a": "a", "b": "b", "c": "c"}}})),
            ..RawArgs::default()
        };
        let _ = handle("decide", &ready(), &args, &state).expect("answers");
        let rows = recorded(&dir);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].saturated,
            "the fixture must be saturated: {:?}",
            rows[0]
        );
        assert_eq!(
            rows[0].bucket_clamped, None,
            "saturated is not the clamp: {:?}",
            rows[0]
        );
    }

    #[test]
    fn the_control_a_clamp_state_the_head_reports_is_recorded_as_reported() {
        // The near miss: a head that DOES report `bucket_clamped: false` must see `Some(false)` in
        // the row, not `None`. Without this, "always None" would satisfy the test above.
        let dir = tempdir("r3-known");
        let state = ServerState::new("http://x:1")
            .with_sidecar(stub_sidecar("r3-known", |_, _, _| {
                json!({"type": "noul", "noul": 0.9, "confidence": 0.8, "bucket_clamped": false})
            }))
            .with_ledger_dir(Some(dir.clone()));
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("answers");
        assert_eq!(recorded(&dir)[0].bucket_clamped, Some(false));
    }

    #[test]
    fn a_missing_confidence_is_recorded_as_absent_not_zero() {
        // R3's second half, through the handler. The head answers a noul with NO confidence.
        // `answer_from_laya` defaults that to 0.0 on the way in; the row must not inherit it.
        let dir = tempdir("r3-conf");
        let state = ServerState::new("http://x:1")
            .with_sidecar(stub_sidecar(
                "r3-conf",
                |_, _, _| json!({"type": "noul", "noul": 0.9}),
            ))
            .with_ledger_dir(Some(dir.clone()));
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("answers");
        assert_eq!(recorded(&dir)[0].confidence, None);
        // And the gate: the same missing confidence must not become 0.0 there either.
        let args = RawArgs {
            action: Some("git status".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("gate", &ready(), &args, &state).expect("gate");
        let gate_rows: Vec<_> = recorded(&dir)
            .into_iter()
            .filter(|r| r.tool == "gate")
            .collect();
        assert_eq!(gate_rows[0].confidence, None, "{:?}", gate_rows[0]);
    }

    #[test]
    fn the_control_a_reported_confidence_is_recorded() {
        let dir = tempdir("r3-conf-ctl");
        let state = state_with_stub(&dir, "r3-conf-ctl");
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("answers");
        assert_eq!(recorded(&dir)[0].confidence, Some(0.8));
    }

    #[test]
    fn a_noul_row_carries_its_p() {
        // R9: the doc promised `p`; the payload carries it; the row now copies it.
        let dir = tempdir("r9-p");
        let state = state_with_stub(&dir, "r9-p");
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("answers");
        assert_eq!(recorded(&dir)[0].p, Some(0.9));
    }

    #[test]
    fn the_ledger_state_is_announced_at_startup() {
        // R4. A configured ledger announces ON, with its directory.
        let dir = tempdir("r4-on");
        let sink = Captured::default();
        let state = state_with_ledger(&dir).with_event_sink(Box::new(sink.clone()));
        state.announce_ledger();
        let lines = sink.lines();
        assert_eq!(lines.len(), 1, "exactly one startup event: {lines:?}");
        assert_eq!(lines[0]["event"], "ledger");
        assert_eq!(lines[0]["enabled"], true);
        assert_eq!(lines[0]["dir"], json!(dir.display().to_string()));
        assert_eq!(lines[0]["level"], "info", "ON is not a warning");
    }

    #[test]
    fn the_control_an_unset_ledger_directory_is_announced_as_off() {
        // R4's control, and the deployment failure this spec answers: with no directory the server
        // must SAY so, as a warning, rather than say nothing.
        let sink = Captured::default();
        let state = test_state().with_event_sink(Box::new(sink.clone()));
        assert!(state.ledger_dir.is_none());
        state.announce_ledger();
        let lines = sink.lines();
        assert_eq!(lines.len(), 1, "silence is the defect: {lines:?}");
        assert_eq!(lines[0]["event"], "ledger");
        assert_eq!(lines[0]["enabled"], false);
        assert_eq!(lines[0]["dir"], serde_json::Value::Null);
        assert_eq!(lines[0]["level"], "warn", "off is a warning, not an info");
        assert!(
            lines[0]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("LAYA_LEDGER_DIR"),
            "it names the variable to set: {}",
            lines[0]
        );
    }

    #[test]
    fn each_tool_call_emits_one_structured_event() {
        // R6, through `handle`: one gate call, one line, and a second call gets the next `seq`.
        let dir = tempdir("r6-one");
        let sink = Captured::default();
        let state = state_with_stub(&dir, "r6-one").with_event_sink(Box::new(sink.clone()));
        let args = RawArgs {
            action: Some("git status".to_string()),
            ..RawArgs::default()
        };
        let _ = handle("gate", &ready(), &args, &state).expect("gate");
        let lines = sink.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let e = &lines[0];
        assert_eq!(e["event"], "tool_call");
        assert_eq!(e["tool"], "gate");
        assert_eq!(e["outcome"], "ok");
        assert_eq!(e["seq"], 1);
        assert!(e["latency_ms"].is_u64(), "{e}");
        assert!(e["ts"].is_u64(), "{e}");
        assert!(
            ["allow", "ask", "deny"].contains(&e["decision"].as_str().unwrap_or("")),
            "the gate's verdict is on the event: {e}"
        );
        let _ = handle("gate", &ready(), &args, &state).expect("gate");
        assert_eq!(sink.lines()[1]["seq"], 2, "seq counts calls");
    }

    #[test]
    fn a_failed_call_is_an_error_event_not_silence() {
        // The control for `outcome`: an event that always said "ok" would pass the test above.
        let sink = Captured::default();
        let state = test_state().with_event_sink(Box::new(sink.clone()));
        let _ = handle("decide", &ready(), &decide_args(None, None), &state)
            .expect_err("the head is unreachable");
        let lines = sink.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["outcome"], "error");
        assert_eq!(lines[0]["tool"], "decide");
    }

    #[test]
    fn events_carry_the_run_and_node_id() {
        // R6's real requirement. The ids are what make an event joinable to a run; the JSON shape
        // is incidental. The control is the same call with none: the fields are null, not absent.
        let dir = tempdir("r6-ids");
        let sink = Captured::default();
        let state = state_with_stub(&dir, "r6-ids").with_event_sink(Box::new(sink.clone()));
        let _ = handle(
            "decide",
            &ready(),
            &decide_args(Some("run-42"), Some("triage")),
            &state,
        )
        .expect("decide");
        let _ = handle("decide", &ready(), &decide_args(None, None), &state).expect("decide");
        let lines = sink.lines();
        assert_eq!(lines[0]["run_id"], "run-42", "{}", lines[0]);
        assert_eq!(lines[0]["node_id"], "triage", "{}", lines[0]);
        assert_eq!(lines[1]["run_id"], serde_json::Value::Null);
        assert_eq!(lines[1]["node_id"], serde_json::Value::Null);
    }

    // ---- the question contract, which three defects disagreed with --------------

    /// Every question the model can read, in the shape the model documents.
    fn a_readable_question_map() -> serde_json::Value {
        json!({
            "kind": {"type": "noul", "instructions": "Is this a bug?"},
            "where": {
                "type": "choice",
                "instructions": "Where is the fault?",
                "criteria": {"parser": "the parser", "lexer": "the lexer"}
            },
            "size": {
                "type": "score",
                "instructions": "How big?",
                "criteria": ["one file", "one module", "the workspace"]
            }
        })
    }

    #[test]
    fn decide_refuses_a_malformed_question_through_the_handler() {
        // This is the test the control proved was missing. Disabling the call site in
        // `decide_via_sidecar` left every validation test above PASSING, because they call
        // `validate_question_shape` directly -- they test the validator, not the wiring. A
        // check that stays green while the live path is unguarded certifies the bug.
        //
        // So this one goes through `handle`, the same entry point production uses, and must
        // fail if the validator is ever unwired from `decide`.
        let mut a = args_with(Some("some state"), None);
        a.questions = Some(json!([
            {"id": "k", "type": "noul", "text": "Is this a bug?"}
        ]));
        let out = handle("decide", &ready(), &a, &test_state());
        let err = out.expect_err("an array of questions must be refused by `decide`");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("must be an OBJECT"),
            "the refusal must come from the shape validator, not the model: {msg}"
        );
        assert!(
            !msg.contains("AttributeError") && !msg.contains("KeyError"),
            "the caller must not be shown the model's own Python error: {msg}"
        );
    }

    #[test]
    fn decide_refuses_a_text_field_through_the_handler_and_names_the_rename() {
        let mut a = args_with(Some("some state"), None);
        a.questions = Some(json!({"k": {"type": "noul", "text": "Is this a bug?"}}));
        let err = handle("decide", &ready(), &a, &test_state())
            .expect_err("`text` must be refused by `decide`");
        let msg = format!("{err:?}");
        assert!(msg.contains("not `text`"), "must name the rename: {msg}");
    }

    #[test]
    fn decide_refuses_a_score_without_criteria_through_the_handler() {
        let mut a = args_with(Some("some state"), None);
        a.questions = Some(json!({"s": {"type": "score", "instructions": "How big?"}}));
        let err = handle("decide", &ready(), &a, &test_state())
            .expect_err("a score without criteria must be refused by `decide`");
        assert!(format!("{err:?}").contains("needs `criteria`"), "{err:?}");
    }

    #[test]
    fn a_well_formed_question_map_is_accepted() {
        // The control: the validator must not refuse the shape the schema advertises, or it
        // would reject every valid call and the three refusal tests below would prove nothing
        // about anything else.
        assert!(validate_question_shape(&a_readable_question_map()).is_ok());
    }

    #[test]
    fn an_array_of_questions_is_refused_by_name() {
        // Defect 1, measured: the schema used to say `array`, and sending one produced
        // `AttributeError: 'list' object has no attribute 'keys'` from inside the model's
        // Python -- an error that names neither the argument nor the fix.
        let err = validate_question_shape(&json!([
            {"id": "k", "type": "noul", "instructions": "Is this a bug?"}
        ]))
        .expect_err("an array must be refused");
        assert!(err.contains("must be an OBJECT"), "{err}");
        assert!(
            err.contains("instructions"),
            "the message must say how: {err}"
        );
    }

    #[test]
    fn text_instead_of_instructions_is_refused_and_names_the_rename() {
        // Defect 2: `laya_abi::Question` names the field `text`, the model requires
        // `instructions`, and the schema named neither. The refusal has to point at the
        // rename, or the caller is left comparing two words it cannot see.
        let err = validate_question_shape(&json!({
            "k": {"type": "noul", "text": "Is this a bug?"}
        }))
        .expect_err("`text` must be refused");
        assert!(err.contains("no `instructions`"), "{err}");
        assert!(
            err.contains("not `text`"),
            "the message must name the rename: {err}"
        );
    }

    #[test]
    fn a_score_without_criteria_is_refused_by_name() {
        // Defect 3: a score without criteria raises `TypeError: 'NoneType' object is not
        // iterable`, and for a score the criteria ARE the scale -- so the number is
        // uninterpretable even where it does not raise.
        let err = validate_question_shape(&json!({
            "s": {"type": "score", "instructions": "How big?"}
        }))
        .expect_err("a score without criteria must be refused");
        assert!(err.contains("needs `criteria`"), "{err}");
        assert!(err.contains("ARRAY") && err.contains("ascending"), "{err}");
    }

    #[test]
    fn a_choice_without_criteria_is_refused_and_says_which_shape() {
        let err = validate_question_shape(&json!({
            "c": {"type": "choice", "instructions": "Which?"}
        }))
        .expect_err("a choice without criteria must be refused");
        assert!(err.contains("OBJECT mapping each option"), "{err}");
    }

    #[test]
    fn a_choice_with_an_empty_criteria_offers_nothing_to_choose() {
        // The near-miss: `criteria` present but empty. It passes a presence check and still
        // cannot be answered, which is exactly the "validator accepts a request that can
        // never work" defect this project keeps removing.
        let err = validate_question_shape(&json!({
            "c": {"type": "choice", "instructions": "Which?", "criteria": {}}
        }))
        .expect_err("an empty choice must be refused");
        assert!(err.contains("no option to choose"), "{err}");
    }

    #[test]
    fn a_noul_needs_no_criteria() {
        // And the other near-miss: `criteria` is required for two of the three types, NOT
        // for all three. A rule that demanded it everywhere would refuse valid calls.
        assert!(
            validate_question_shape(&json!({
                "k": {"type": "noul", "instructions": "Is this a bug?"}
            }))
            .is_ok()
        );
    }

    #[test]
    fn an_unknown_question_type_is_refused_by_name() {
        let err = validate_question_shape(&json!({
            "k": {"type": "maybe", "instructions": "?"}
        }))
        .expect_err("an unknown type must be refused");
        assert!(err.contains("maybe"), "{err}");
    }

    #[test]
    fn the_advertised_schema_states_the_three_things_the_defects_got_wrong() {
        // The schema IS the contract, so it gets asserted like one. Each assertion here is a
        // defect that shipped: `array` instead of object, `text` instead of `instructions`,
        // and `criteria` unmarked as required.
        let s = questions_schema();
        assert_eq!(s["type"], "object", "the questions are a MAP, not an array");
        let item = &s["additionalProperties"];
        let required: Vec<&str> = item["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert!(
            required.contains(&"instructions"),
            "instructions is required"
        );
        assert!(
            !required.contains(&"text"),
            "`text` is not the field name and must not be advertised"
        );
        assert!(
            item["properties"]["instructions"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("NOT `text`")),
            "the schema must say the rename out loud"
        );
        assert!(
            item["properties"]["criteria"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("Required for `choice` and `score`")),
            "criteria's requirement must be stated"
        );
    }

    #[test]
    fn every_tool_that_takes_questions_advertises_the_same_schema() {
        // Two tools, one contract. If `decide` and `ask` disagreed, a caller that worked with
        // one would fail with the other -- and only one of the two would ever get fixed.
        //
        // The arguments live under `properties`; the schema's own root is the object wrapper.
        // My first version read the root and failed a correct schema -- the check was wrong,
        // which is why the assertions name the path they read.
        for tool in ["decide", "ask"] {
            let (_, schema) = tool_meta(tool);
            let q = schema["properties"].get("questions").unwrap_or_else(|| {
                panic!(
                    "{tool} must advertise `questions`; it advertises {:?}",
                    schema["properties"]
                        .as_object()
                        .map(|m| m.keys().collect::<Vec<_>>())
                )
            });
            assert_eq!(
                q["type"], "object",
                "{tool} advertises a non-object questions argument: {q}"
            );
            assert!(
                q["additionalProperties"]["required"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|v| v == "instructions")),
                "{tool}'s question items must require `instructions`: {q}"
            );
        }
    }

    #[test]
    fn route_is_not_a_tool() {
        // The load-bearing absence: exposing `route` would let an agent select edges
        // outside any graph, the injection failure mode the architecture prevents.
        assert!(!is_known_tool("route"), "`route` must never be exposed");
        assert!(!TOOL_NAMES.contains(&"route"));
    }

    #[test]
    fn every_tool_has_a_description_and_a_schema() {
        for t in TOOL_NAMES {
            let (desc, schema) = tool_meta(t);
            assert!(!desc.is_empty(), "{t} needs a description");
            assert_eq!(
                schema.get("type"),
                Some(&json!("object")),
                "{t} needs a schema"
            );
            assert!(schema.contains_key("properties"), "{t} needs properties");
        }
    }

    #[test]
    fn a_command_like_tool_requires_its_arguments() {
        // A schema that does not mark the load-bearing argument required lets a caller
        // omit it and get a confusing failure later.
        for (t, req) in [
            ("decide", "state"),
            ("ask_file", "path"),
            ("ask_files", "paths"),
            ("gate", "action"),
        ] {
            let (_, schema) = tool_meta(t);
            let required = schema.get("required").and_then(|r| r.as_array());
            assert!(
                required.is_some_and(|r| r.iter().any(|v| v.as_str() == Some(req))),
                "{t} must require `{req}`"
            );
        }
    }

    // ---- an unknown tool ----------------------------------------------------

    #[test]
    fn an_unknown_tool_is_an_invalid_params_error() {
        let e = handle("nonsense", &ready(), &no_args(), &test_state());
        assert!(e.is_err(), "an unknown tool must error");
    }

    // ---- FAIL CLOSED: the property that matters most -------------------------

    /// Extract the `decision` field from a gate result's JSON text.
    ///
    /// Parsed rather than substring-matched: the first version of the test below
    /// grepped the rendered result for "allow" and failed on the *note* ("the gate
    /// could not ask, so it must not allow") while the decision was correctly `ask`.
    /// A substring check on a human-readable message tests the wording, not the
    /// behaviour.
    fn gate_decision(result: &CallToolResult) -> String {
        let text = result
            .content
            .iter()
            .find_map(|c| match c {
                ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("decision").and_then(|d| d.as_str()).map(String::from))
            .unwrap_or_else(|| panic!("no `decision` field in the gate result: {text}"))
    }

    #[test]
    fn when_laya_is_down_the_gate_answers_ask_and_never_allow() {
        // THE property. An unavailable gate must never mean permission.
        let r = handle("gate", &down(), &no_args(), &test_state())
            .unwrap_or_else(|e| panic!("gate must return a decision, not an error: {e:?}"));
        assert_eq!(
            gate_decision(&r),
            "ask",
            "the gate must answer ask while down"
        );
    }

    #[test]
    fn the_gate_result_carries_the_laya_unavailable_reason() {
        // The decision alone is not enough for a caller trying to explain itself, so
        // the reason is part of the contract.
        let r =
            handle("gate", &down(), &no_args(), &test_state()).unwrap_or_else(|e| panic!("{e:?}"));
        let text = format!("{r:?}");
        assert!(
            text.contains("laya-unavailable"),
            "the reason must be stated: {text}"
        );
    }

    #[test]
    fn when_laya_is_down_every_other_tool_is_a_protocol_error() {
        // The control for the line above: only `gate` gets the decision treatment.
        for t in [
            "decide",
            "ask_file",
            "ask_files",
            "ask",
            "should_compact",
            "ledger",
        ] {
            let out = handle(t, &down(), &no_args(), &test_state());
            assert!(
                out.is_err(),
                "{t} must error while Laya is down, not answer"
            );
        }
    }

    #[test]
    fn the_down_error_names_the_reason_and_not_a_key() {
        let Some(e) = handle("decide", &down(), &no_args(), &test_state()).err() else {
            panic!("a down server must error for decide")
        };
        let msg = format!("{e:?}");
        assert!(msg.contains("not ready"), "must state readiness: {msg}");
        assert!(msg.contains("refused"), "must carry the reason: {msg}");
        assert!(!msg.to_lowercase().contains("api_key"));
        assert!(!msg.contains("Authorization"));
    }

    // ---- ready: honest unimplemented ---------------------------------------

    #[test]
    fn the_still_unimplemented_tools_say_so() {
        // These need the file-reading / compaction work the spec assigns elsewhere.
        // An unimplemented tool that "succeeds" is how a caller comes to trust a
        // fabricated answer, so each must be an ERROR that says why.
        // `ask_file` and `ask_files` are IMPLEMENTED now, so they are not in this list.
        // Leaving them here would assert the opposite of the truth — and this is exactly
        // how a "not implemented" claim outlives the implementation.
        // EVERY tool is implemented now. This test is inverted from its original purpose —
        // it used to prove that the unimplemented ones SAID SO rather than faking success.
        // That property still matters, so it is asserted positively: no tool may be reported
        // as unimplemented, because none is.
        for tool in [
            "decide",
            "gate",
            "ask_file",
            "ask_files",
            "should_compact",
            "ask",
            "ledger",
        ] {
            let out = handle(tool, &ready(), &no_args(), &test_state());
            let unimplemented = out
                .err()
                .is_some_and(|e| e.message.contains("not implemented"));
            assert!(
                !unimplemented,
                "`{tool}` must no longer report itself unimplemented"
            );
        }
    }

    #[test]
    fn an_implemented_tool_missing_a_required_argument_says_which_one() {
        // `decide`/`gate` now have implementations, so a missing argument is an
        // arg-validation error naming the argument — not "not implemented", and not a
        // confusing failure deeper in.
        for (tool, want) in [("decide", "state"), ("gate", "action")] {
            let out = handle(tool, &ready(), &no_args(), &test_state());
            let msg = format!("{:?}", out.err().unwrap_or_default());
            assert!(
                msg.contains("requires"),
                "{tool} must say it requires something: {msg}"
            );
            assert!(msg.contains(want), "{tool} must name `{want}`: {msg}");
        }
    }

    /// Arguments for `ask_file`: a path and the question to ask about it.
    fn args_ask_file(path: &str, question: &str) -> RawArgs {
        RawArgs {
            path: Some(path.to_string()),
            question: Some(question.to_string()),
            ..RawArgs::default()
        }
    }

    fn args_with(state: Option<&str>, action: Option<&str>) -> RawArgs {
        RawArgs {
            state: state.map(str::to_string),
            action: action.map(str::to_string),
            ..RawArgs::default()
        }
    }

    #[test]
    fn decide_with_a_dead_sidecar_is_an_error_not_a_fabricated_answer() {
        // The property that matters most for `decide`: with no head there is no
        // answer, and inventing one would be far worse than failing. The test state
        // points at a socket that cannot exist.
        // A WELL-FORMED question, or my shape validator refuses first and the assertion
        // below would be checking the wrong cause. This test is about a dead sidecar, so the
        // arguments must be valid enough to reach it -- which is itself the point of
        // validating in front of the model.
        let mut a = args_with(Some("some state"), None);
        a.questions = Some(json!({
            "k": {"type": "noul", "instructions": "Is this a bug?"}
        }));
        let out = handle("decide", &ready(), &a, &test_state());
        assert!(out.is_err(), "decide must error with no head");
        let msg = out
            .as_ref()
            .err()
            .map_or_else(|| String::from("(no error produced)"), |e| format!("{e:?}"));
        assert!(
            msg.contains("sidecar") || msg.contains("unavailable"),
            "the error must name the cause: {msg}"
        );
    }

    #[test]
    fn gate_with_a_dead_sidecar_answers_ask_and_never_allow() {
        // The fail-closed contract, exercised through the SAME entry point production
        // uses — not a re-enactment of it.
        let a = args_with(None, Some("rm -rf /important"));
        let out = handle("gate", &ready(), &a, &test_state())
            .unwrap_or_else(|e| panic!("gate must answer, not error: {e:?}"));
        assert_eq!(gate_decision(&out), "ask");
        assert_ne!(gate_decision(&out), "allow");
    }

    // ---- ask: validate the WHOLE request before anything executes (R7/R8) ---
    //
    // These call `ask_execute`/`run_with_deadline` directly rather than going through
    // `handle("ask", ...)`: `test_state()` points at a dead sidecar (deliberately, so every
    // other test in this module stays offline), so there is no way to stub the MODEL'S
    // answer through the full pipeline. The gate decision these tests need IS the seam
    // `ask_execute` exists to make testable — see its doc comment.

    #[test]
    fn ask_refuses_before_it_executes() {
        // A request `compose_state` refuses: the summary ALONE is already over the cap, so
        // the command's output could only make it worse — certain to fail, known without
        // running anything. THE FIX is that this is caught before the command runs, not
        // after: the measured live defect was the gate allowing a destructive command, the
        // command running, and only THEN compose_state refusing — by which point the data
        // was already gone.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let canary = dir.path().join("canary");
        let huge_summary = "x".repeat(laya_core::agentic::STATE_CAP_BYTES + 1);
        let cmd = format!("touch {}", canary.display());

        let out = ask_execute(
            Some(&huge_summary),
            &[],
            Some(&cmd),
            // Stubbed to Allow ON PURPOSE: proves the refusal comes from the precondition,
            // not from the gate — a gate that happened to also refuse would prove nothing.
            laya_core::GateDecision::Allow,
            60,
        );
        assert!(out.is_err(), "an oversized state must refuse: {out:?}");
        assert!(
            !canary.exists(),
            "the command must NOT have run: {}",
            canary.display()
        );
    }

    #[test]
    fn ask_the_control_a_valid_request_does_execute() {
        // THE control. Proves the precondition check does not block every request — only
        // the one that was always going to fail.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let canary = dir.path().join("canary");
        let cmd = format!("touch {}", canary.display());

        let out = ask_execute(
            Some("a short, valid summary"),
            &[],
            Some(&cmd),
            laya_core::GateDecision::Allow,
            60,
        );
        assert!(out.is_ok(), "a valid request must execute: {out:?}");
        assert!(canary.exists(), "the command must have run");
    }

    #[test]
    fn ask_refuses_a_command_the_gate_did_not_allow() {
        // The companion control: even a request that WOULD compose fine must not run a
        // command the gate judged `ask`/`deny`.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let canary = dir.path().join("canary");
        let cmd = format!("touch {}", canary.display());

        let out = ask_execute(
            Some("a short, valid summary"),
            &[],
            Some(&cmd),
            laya_core::GateDecision::Ask,
            60,
        );
        assert!(out.is_err(), "a non-allow decision must refuse: {out:?}");
        assert!(!canary.exists(), "the command must NOT have run");
    }

    #[test]
    fn ask_a_command_past_its_deadline_is_killed() {
        let start = std::time::Instant::now();
        let (_, _, timed_out) =
            run_with_deadline("sleep 30", 1).unwrap_or_else(|e| panic!("must run: {e:?}"));
        assert!(
            timed_out,
            "a command past its deadline must report timed_out"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "must return promptly, not wait out the full sleep: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn ask_a_command_within_its_deadline_is_not_marked_timed_out() {
        // The control for the row above: a quick command must not be falsely flagged.
        let (stdout, _, timed_out) =
            run_with_deadline("echo hi", 60).unwrap_or_else(|e| panic!("must run: {e:?}"));
        assert!(!timed_out);
        assert!(stdout.contains("hi"));
    }

    // ---- the Laya <-> laya_abi question adapter ----------------------------

    #[test]
    fn the_adapter_sends_layas_own_shape_not_ours() {
        // The two shapes are genuinely different, so the adapter is the one place the
        // translation happens. Pinned so a rename cannot silently pass `text` where
        // Laya expects `instructions`.
        let qs = [
            laya_abi::Question::noul("churn", "Does the user threaten to cancel?"),
            laya_abi::Question::choice("team", "Which team?", ["billing", "technical"]),
        ];
        let out = questions_to_laya(&qs);

        // `get` chains rather than indexing: the workspace denies `indexing_slicing`
        // because it panics, and a panic here would hide WHICH assertion failed.
        let churn = out.get("churn");
        assert_eq!(
            churn.and_then(|c| c.get("type")),
            Some(&json!("noul")),
            "a noul is tagged `noul`: {out}"
        );
        assert_eq!(
            churn.and_then(|c| c.get("instructions")),
            Some(&json!("Does the user threaten to cancel?")),
            "Laya's field is `instructions`, not `text`"
        );
        assert!(
            churn.and_then(|c| c.get("text")).is_none(),
            "we must NOT send our own `text` field"
        );

        let team = out.get("team");
        assert_eq!(team.and_then(|c| c.get("type")), Some(&json!("choice")));
        assert!(
            team.and_then(|c| c.get("criteria"))
                .is_some_and(serde_json::Value::is_object),
            "choice criteria is a MAP of option -> description"
        );
        assert_eq!(
            team.and_then(|c| c.get("criteria"))
                .and_then(|c| c.get("billing")),
            Some(&json!("billing")),
            "each declared option appears as its own key"
        );
    }

    #[test]
    fn a_choice_answer_is_converted_with_declared_option_order() {
        // `probabilities` arrives as a MAP; the adapter must emit a LIST in the order
        // the caller declared, so a consumer that zips them gets a stable mapping.
        let entry = json!({
            "type": "choice", "choice": "billing", "confidence": 0.9,
            "probabilities": {"technical": 0.05, "billing": 0.93, "sales": 0.02}
        });
        let declared = vec![
            "billing".to_string(),
            "technical".to_string(),
            "sales".to_string(),
        ];
        let a = answer_from_laya(&laya_core::Policy::default(), "t", &entry, &declared)
            .unwrap_or_else(|e| panic!("conversion failed: {e}"));
        assert_eq!(a.value, "billing");
        assert_eq!(a.options, declared, "option order is the DECLARED order");
        assert_eq!(a.probabilities, vec![0.93, 0.05, 0.02]);
    }

    #[test]
    fn an_undeclared_choice_is_rejected_not_accepted() {
        // The load-bearing invariant of the whole design: the model may only select
        // among options the caller declared. A violation is an ERROR, never a guess.
        let entry = json!({"type": "choice", "choice": "marketing", "confidence": 0.9,
                           "probabilities": {"billing": 0.5}});
        let declared = vec!["billing".to_string(), "technical".to_string()];
        let r = answer_from_laya(&laya_core::Policy::default(), "t", &entry, &declared);
        assert!(r.is_err(), "an undeclared choice must be rejected");
        assert!(format!("{:?}", r.err()).contains("not one of the declared options"));
    }

    #[test]
    fn a_noul_answer_maps_to_yes_no_and_keeps_p() {
        let yes = answer_from_laya(
            &laya_core::Policy::default(),
            "n",
            &json!({"type": "noul", "noul": 0.8, "confidence": 0.8}),
            &[],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(yes.value, "yes");
        assert_eq!(yes.p, Some(0.8));
        let no = answer_from_laya(
            &laya_core::Policy::default(),
            "n",
            &json!({"type": "noul", "noul": 0.2, "confidence": 0.8}),
            &[],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(no.value, "no");
    }
    #[test]
    fn a_failure_is_never_a_successful_result_with_an_error_prefix() {
        // The pattern we deliberately do NOT use: returning
        // `CallToolResult::success` with text `"error: …"` is fail-open by accident,
        // because a caller checking only for success reads a failure as an answer.
        let out = handle("decide", &down(), &no_args(), &test_state());
        assert!(out.is_err());
        let out = handle("decide", &ready(), &no_args(), &test_state());
        assert!(out.is_err());
    }

    // ---- the readiness cache is actually consulted --------------------------

    #[test]
    fn state_without_a_verdict_has_nothing_fresh() {
        // A caller must re-probe; an absent verdict is NOT permission to proceed.
        let s = test_state();
        assert!(s.fresh_readiness(0).is_none());
    }

    #[test]
    fn state_serves_a_fresh_verdict_within_the_ttl() {
        let s = test_state();
        s.record_readiness(ready(), 100);
        assert_eq!(s.fresh_readiness(200), Some(Readiness::Ready));
        assert!(
            s.fresh_readiness(100 + laya_core::readiness::READINESS_TTL_MS + 1)
                .is_none()
        );
    }

    // ---- the read scope ----------------------------------------------------

    #[test]
    fn a_scope_allows_a_path_inside_its_root() {
        let root = std::env::temp_dir();
        let scope = laya_core::ask_file::Scope::new(vec![root.clone()]);
        let inside = root.join("laya-scope-probe.txt");
        std::fs::write(&inside, b"x").unwrap_or_default();
        assert!(
            scope.allows(&inside),
            "a file inside the root must be allowed"
        );
        let _ = std::fs::remove_file(&inside);
    }

    #[test]
    fn a_scope_refuses_a_path_outside_its_root() {
        let scope = laya_core::ask_file::Scope::new(vec![std::path::PathBuf::from("/tmp")]);
        assert!(
            !scope.allows(std::path::Path::new("/etc/passwd")),
            "a blanket grant is exactly what we must not do"
        );
    }

    #[test]
    fn a_scope_refuses_a_traversal_that_escapes_the_root() {
        // The reason `allows` canonicalises rather than doing a string-prefix check:
        // a prefix check accepts `/tmp/../etc/passwd` for a `/tmp` root.
        let scope = laya_core::ask_file::Scope::new(vec![std::path::PathBuf::from("/tmp")]);
        assert!(!scope.allows(std::path::Path::new("/tmp/../etc/passwd")));
    }

    #[test]
    fn a_nonexistent_path_inside_the_scope_is_in_scope_and_reads_as_missing() {
        // CORRECTED, matching the core module's correction: a missing path inside the scope
        // is IN scope and then fails as missing. The old assertion here had it backwards —
        // it conflated "absent" with "forbidden", which would report a typo as a
        // permissions problem. The rule that actually guards against an existence oracle
        // is that an OUT-of-scope path fails identically whether or not it exists, and
        // that is pinned in the core module.
        let scope = laya_core::ask_file::Scope::new(vec![std::path::PathBuf::from("/tmp")]);
        assert!(
            scope.allows(std::path::Path::new("/tmp/definitely-not-here-9f3a")),
            "a missing path inside the scope is still inside the scope"
        );
    }
    #[test]
    fn ask_file_needs_a_path_and_a_question_and_says_which_is_missing() {
        for (a, want) in [
            (args_with(None, None), "path"),
            (args_ask_file("/tmp/x", ""), "question"),
        ] {
            let out = handle("ask_file", &ready(), &a, &test_state());
            let msg = format!(
                "{:?}",
                out.as_ref()
                    .err()
                    .map_or_else(String::new, |e| format!("{e:?}"))
            );
            assert!(out.is_err(), "ask_file must require its arguments");
            assert!(msg.contains(want), "must name `{want}`: {msg}");
        }
    }

    #[test]
    fn ask_file_refuses_an_out_of_scope_path_even_when_the_head_is_up() {
        // ORDER: the scope is enforced before the model is consulted. A refusal must not
        // depend on a network round trip, and must not be reachable by asking nicely.
        let t =
            ServerState::new("http://x:1").with_read_scope(vec![std::path::PathBuf::from("/tmp")]);
        let a = args_ask_file("/etc/hostname", "what is this?");
        let out = handle("ask_file", &ready(), &a, &t);
        let msg = format!(
            "{:?}",
            out.as_ref()
                .err()
                .map_or_else(String::new, |e| format!("{e:?}"))
        );
        assert!(out.is_err(), "an out-of-scope path must be refused");
        assert!(
            msg.contains("read scope"),
            "the reason must name the scope: {msg}"
        );
    }

    #[test]
    fn ask_file_with_an_empty_scope_refuses_everything() {
        // FAIL CLOSED. `test_state()` has no roots configured, so nothing is readable.
        // A default-open scope is how "least privilege" becomes a slogan.
        let a = args_ask_file("/tmp/whatever.txt", "q");
        let out = handle("ask_file", &ready(), &a, &test_state());
        assert!(
            out.is_err(),
            "with no roots configured, ask_file must refuse"
        );
    }

    #[test]
    fn resolve_read_scope_drops_a_bad_root_and_reports_it() {
        let (scope, notes) = resolve_read_scope(Some("/tmp:/definitely/not/a/dir/xyzzy:/usr"));
        assert_eq!(scope.roots().len(), 2, "the bogus root must be dropped");
        assert!(
            notes.iter().any(|n| n.contains("xyzzy")),
            "a dropped root must be REPORTED, not silent: {notes:?}"
        );
    }

    #[test]
    fn resolve_read_scope_without_the_env_var_defaults_to_the_cwd_and_says_so() {
        let (scope, notes) = resolve_read_scope(None);
        assert_eq!(scope.roots().len(), 1);
        assert!(
            notes.iter().any(|n| n.contains("unset")),
            "the default must be visible, not inferred: {notes:?}"
        );
        assert!(
            !scope.roots().iter().any(|r| r == std::path::Path::new("/")),
            "the default must NEVER be the filesystem root"
        );
    }

    #[test]
    fn the_server_does_not_use_a_work_queue() {
        // A REGRESSION GUARD ON AN ABSENCE, which is unusual and deliberate.
        //
        // This server previously carried a priority queue whose push/pop happened in one
        // lock scope, so it could never fill and its "queue is full, fail closed" error
        // could never fire. It read as protection and provided none. The queue is gone;
        // this test exists so a future contributor who adds one back has to think about
        // whether this server has any concurrency to prioritise (it does not: one client
        // over stdio, current-thread runtime).
        let src = include_str!("lib.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
        for forbidden in ["PriorityQueue", ".queue.lock()"] {
            assert!(
                !prod.contains(forbidden),
                "`{forbidden}` is back in production code — see the note in `dispatch` \
                 before wiring a queue into a serial server"
            );
        }
    }
    // ---- the clock wiring (regression guard) --------------------------------
    //
    // These use `clock::now_ms()` the way the PROGRAM does. The tests above use
    // hand-picked timestamps, which is why they all passed while the server was
    // recording its startup verdict at 0 and therefore treating a healthy Laya as
    // down. A check whose inputs are chosen by the test cannot catch a wiring bug
    // between the test's numbers and the program's clock.

    #[test]
    fn the_clock_is_monotonic_and_starts_near_zero() {
        let a = clock::now_ms();
        let b = clock::now_ms();
        assert!(b >= a, "the clock must not go backwards");
        assert!(
            a < 60_000,
            "the origin is process start, so this is milliseconds since boot of the process"
        );
    }

    #[test]
    fn a_verdict_recorded_with_the_shared_clock_is_fresh() {
        // THE regression test. Record exactly as `main` does, read exactly as
        // `dispatch` does. If the record site ever goes back to a constant timestamp,
        // this fails — which the timestamp-100/200 tests could not have caught.
        let s = test_state();
        s.record_readiness(ready(), clock::now_ms());
        assert_eq!(
            s.fresh_readiness(clock::now_ms()),
            Some(Readiness::Ready),
            "a startup verdict recorded with the shared clock must be fresh"
        );
    }

    #[test]
    fn a_verdict_recorded_at_zero_is_stale_against_a_real_clock() {
        // Documents the bug precisely: this is what the original code did, and why it
        // made a healthy Laya look down.
        let s = test_state();
        s.record_readiness(ready(), 0);
        assert!(
            s.fresh_readiness(clock::now_ms() + laya_core::readiness::READINESS_TTL_MS + 1)
                .is_none(),
            "a timestamp of 0 is stale once the process has been alive past the TTL"
        );
    }

    #[test]
    fn a_fresh_verdict_is_not_rewritten_by_a_cache_miss_path() {
        // The property the fix restores: a ready verdict recorded normally is served,
        // so the fail-closed path is NOT what answers ordinary traffic.
        let s = test_state();
        s.record_readiness(ready(), clock::now_ms());
        let served = s.fresh_readiness(clock::now_ms());
        assert!(
            served.is_some_and(|r| r.is_ready()),
            "ordinary traffic must be served by the ready verdict"
        );
    }
}
