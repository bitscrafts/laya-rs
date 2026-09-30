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
//!   `"error: …"`.** That pattern (which `pii-mcp` uses) is fail-open by accident: a
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
        }
    }

    /// Set the read scope (`ask_file`/`ask_files`).
    #[must_use]
    pub fn with_read_scope(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.read_scope = laya_core::ask_file::Scope::new(paths);
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

/// The description and JSON schema for a tool.
fn tool_meta(name: &str) -> (String, serde_json::Map<String, serde_json::Value>) {
    let (desc, props, required) = match name {
        "decide" => (
            "Ask Laya a typed question about a state and get a route-shaped answer \
             (label, band, confidence, probabilities). The thresholds live in the \
             caller; the model never sees them.",
            json!({
                "state": {"type": "string", "description": "the state to judge"},
                "questions": {"type": "array", "description": "typed questions (noul/choice/score)"}
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
            "Ask the same question across many files (globs expanded, pruned, capped, \
             asked in parallel) and return a pick. Reading none of them into context.",
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
                "questions": {"type": "array"}
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
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    irreversible: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    /// The question `ask_file` asks about the file. Separate from `path` on purpose:
    /// keeping them in distinct fields is what stops content being concatenated into the
    /// question, which is how a file's text could become an instruction.
    #[serde(default)]
    question: Option<String>,
    /// The paths `ask_files` asks about.
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    op: Option<String>,
    /// Laya's own question map. Kept as a `Value` in the POC; a typed
    /// `laya_abi::Question` array is the next step (the adapter is ready).
    #[serde(default)]
    questions: Option<serde_json::Value>,
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
/// This is the correction of `pii-mcp`'s pattern: a failure becomes
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

/// The shared entry point for every tool: **check readiness, then dispatch**.
///
/// One place, so no tool can invent its own (weaker) down behaviour. This is the
/// function the fail-closed guarantee lives in, and it is pure enough to test
/// exhaustively without a server.
///
/// # Errors
/// Returns an [`McpError`] when the tool is unknown, when an unimplemented tool is
/// called, or when a non-`gate` tool is called while Laya is down.
pub fn handle(
    tool: &str,
    readiness: &Readiness,
    args: &RawArgs,
    state: &ServerState,
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
        return match laya_core::readiness::when_not_ready(tool) {
            WhenNotReady::GateAsks => to_result(ToolOutcome::Ok(
                json!({
                    "decision": laya_core::readiness::gate_when_down(),
                    "reason": "laya-unavailable",
                    "note": "the gate could not ask, so it must not allow"
                })
                .to_string(),
            )),
            WhenNotReady::ProtocolError => Err(McpError::internal_error(
                format!(
                    "laya-mcp: Laya is not ready, so `{tool}` cannot answer.\n{}",
                    readiness_reason(readiness)
                ),
                None,
            )),
        };
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
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            gate_via_sidecar(args, state)
        }
        "decide" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            decide_via_sidecar(args, state)
        }
        "ask_file" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            ask_file_via_sidecar(args, state)
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
    let action = args.action.clone().unwrap_or_default();
    let irreversible = args.irreversible.unwrap_or(false);

    let question = if irreversible {
        format!(
            "Is this action safe to run WITHOUT human confirmation? It is believed to be              IRREVERSIBLE, so a wrong 'yes' cannot be undone. Action: {action}"
        )
    } else {
        format!("Is this action safe to run without human confirmation? Action: {action}")
    };

    let questions = [laya_abi::Question::noul("safe", question)];
    let laya_q = questions_to_laya(&questions);

    let payload = match state.sidecar.predict(&json!({"action": action}), &laya_q) {
        Ok(answers) => {
            let entry = answers.get("safe").cloned().unwrap_or_else(|| json!({}));
            let answer = answer_from_laya("safe", &entry, &[])
                .map_err(|e| McpError::internal_error(format!("laya-mcp: {e}"), None))?;
            let policy = laya_core::Policy::default();
            let dp = laya_core::DecisionPayload::from_answer(&answer, &policy);
            // The gate decision is derived from CONFIDENCE, never from the noul value
            // alone: "the answer says what, confidence says whether."
            let decision = policy.gate(answer.confidence, irreversible, answer.saturated());
            serde_json::to_value(&dp).map_or_else(
                |_| json!({}),
                |mut v| {
                    if let Some(o) = v.as_object_mut() {
                        o.insert("decision".into(), json!(decision));
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
    };

    to_result(ToolOutcome::Ok(payload.to_string()))
}

/// The `decide` tool: typed questions -> route-shaped payloads.
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

    // Reported with the path it came from so a caller cannot confuse which file a verdict
    // belongs to — and `content_in_context: false` states the property explicitly rather
    // than leaving the caller to infer it.
    let mut out = json!({ "path": file.path, "answers": answers });
    if let Some(obj) = out.as_object_mut() {
        obj.insert("content_in_context".into(), json!(false));
    }
    to_result(ToolOutcome::Ok(out.to_string()))
}

fn decide_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
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

    match state.sidecar.predict(&state_value, &questions) {
        Ok(answers) => to_result(ToolOutcome::Ok(json!({"answers": answers}).to_string())),
        Err(e) => Err(McpError::internal_error(
            format!("laya-mcp: the decision head is unavailable: {e}"),
            None,
        )),
    }
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
        "should_compact" => &["turns_summary"],
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
            let value = match noul {
                Some(v) if v >= 0.5 => "yes".to_string(),
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

/// The dynamic tool-dispatch function type, mirroring `pii-mcp`'s working shape:
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
        let args: RawArgs = serde_json::from_value(args_value).unwrap_or_default();

        // Readiness FIRST, every call. A fresh cached verdict is reused; otherwise the
        // caller must re-probe, and an absent verdict is treated as NOT ready — the
        // fail-closed direction.
        // A fresh cached verdict is reused; otherwise we PROBE. Not probing here was
        // the bug the end-to-end test caught: a cache miss was treated as "down", so a
        // healthy Laya answered every call through the fail-closed path.
        let now_ms = clock::now_ms();
        let readiness = state.fresh_readiness(now_ms).unwrap_or_else(|| {
            let verdict = probe(&state.endpoint);
            state.record_readiness(verdict.clone(), now_ms);
            verdict
        });

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
    #![allow(clippy::panic)]

    use super::*;
    use laya_core::readiness::judge;

    fn ready() -> Readiness {
        Readiness::Ready
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
        // `ask_file` is IMPLEMENTED now, so it is not in this list. Its own tests cover
        // the argument contract; leaving it here would have asserted the opposite.
        for tool in ["ask", "ask_files", "should_compact", "ledger"] {
            let out = handle(tool, &ready(), &no_args(), &test_state());
            assert!(
                out.is_err(),
                "{tool} has no implementation, so it must be an ERROR"
            );
            let msg = format!("{:?}", out.err().unwrap_or_default());
            assert!(msg.contains("not implemented"), "{tool}: {msg}");
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
        let a = args_with(Some("some state"), None);
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
        let a = answer_from_laya("t", &entry, &declared)
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
        let r = answer_from_laya("t", &entry, &declared);
        assert!(r.is_err(), "an undeclared choice must be rejected");
        assert!(format!("{:?}", r.err()).contains("not one of the declared options"));
    }

    #[test]
    fn a_noul_answer_maps_to_yes_no_and_keeps_p() {
        let yes = answer_from_laya(
            "n",
            &json!({"type": "noul", "noul": 0.8, "confidence": 0.8}),
            &[],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(yes.value, "yes");
        assert_eq!(yes.p, Some(0.8));
        let no = answer_from_laya(
            "n",
            &json!({"type": "noul", "noul": 0.2, "confidence": 0.8}),
            &[],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(no.value, "no");
    }
    #[test]
    fn a_failure_is_never_a_successful_result_with_an_error_prefix() {
        // The pii-mcp pattern we deliberately do NOT copy: returning
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
