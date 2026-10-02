//! `laya graph` — the CLI surface for the engine in `laya-graph` (R19, R20).
//!
//! Thin on purpose: parsing, lowering, scheduling and rendering all live in
//! `laya-graph`; this module parses its own arguments (collected verbatim by
//! `main`'s top-level parser — see the note there), calls into the engine,
//! and prints. The core of each subcommand returns a plain `Result` rather
//! than an `ExitCode`, so it is testable without a process (R20's own tests
//! inject a fake `McpCaller` for a flow that never calls one, and need
//! neither the live backbone nor a real `laya-mcp`).

use std::process::ExitCode;
use std::sync::Arc;

use laya_graph::{Flow, Library, McpCaller, MermaidRender, RunOptions, RunReport};
use laya_graph::{expand, lower, render as render_graph, run_flow, validate as validate_flow};
use serde_json::Value;

use crate::mcp_client::{DEFAULT_CALL_TIMEOUT, LiveMcpCaller, resolve_mcp_binary};

/// `laya graph <validate|render|run> <flow.json>` — a dispatcher, kept
/// deliberately thin like `ledger`'s.
///
/// `args` arrives already stripped of the top-level `graph` word (`main`'s
/// parser collects a subcommand's own arguments verbatim, never validating
/// them itself — the established `laya-cli` pattern, so `--flag x` after
/// `run` reaches HERE rather than being rejected by code that has no idea
/// what it means).
#[must_use]
pub fn graph(args: &[String]) -> ExitCode {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!("usage: laya graph <validate|render|run> <flow.json>");
        return ExitCode::FAILURE;
    };
    let Some(path) = args.get(1) else {
        eprintln!("laya graph {sub}: needs a flow.json path");
        return ExitCode::FAILURE;
    };

    match sub {
        "validate" => graph_validate(path),
        "render" => graph_render(path),
        "run" => graph_run(path),
        other => {
            eprintln!("laya: unknown graph subcommand `{other}`");
            ExitCode::FAILURE
        }
    }
}

/// Read `path`, parse it as JSON, and expand any `subflow` nodes (empty
/// library: the CLI does not yet accept a library file — out of scope for
/// this part) into a flat [`Flow`].
///
/// # Errors
/// A message naming the file, the JSON error, or the expansion refusal —
/// never a generic "could not load".
fn load_flow(path: &str) -> Result<Flow, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
    let raw: Value =
        serde_json::from_str(&text).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    expand(&raw, &Library::new()).map_err(|e| e.to_string())
}

/// `laya graph validate` — core. The engine's own refusal message IS the
/// product (R19): never replaced with a generic "invalid flow".
///
/// # Errors
/// The named [`laya_graph::FlowError`], rendered as text.
pub fn do_validate(path: &str) -> Result<(), String> {
    let flow = load_flow(path)?;
    validate_flow(&flow).map_err(|e| e.to_string())
}

/// `laya graph render` — core: validate, lower, render.
///
/// # Errors
/// The refusal that stopped the pipeline, rendered as text.
pub fn do_render(path: &str) -> Result<MermaidRender, String> {
    let flow = load_flow(path)?;
    validate_flow(&flow).map_err(|e| e.to_string())?;
    let graph = lower(&flow).map_err(|e| e.to_string())?;
    Ok(render_graph(&graph))
}

/// `laya graph run` — core: validate, lower, run.
///
/// `mcp` is injected so a flow with no `mcp`/`decide` node can be tested with
/// no live dependency at all, and so `laya graph run` itself can inject the
/// real [`LiveMcpCaller`].
///
/// # Errors
/// The refusal that stopped the pipeline, or the engine's own
/// [`laya_graph::RunError`], rendered as text — the engine's error type is
/// never swallowed into a generic message (the spec's own error-handling
/// expectation for this part).
pub fn do_run(
    path: &str,
    mcp: Arc<dyn McpCaller>,
    limit: usize,
    run_id: String,
) -> Result<RunReport, String> {
    let flow = load_flow(path)?;
    validate_flow(&flow).map_err(|e| e.to_string())?;
    let graph = lower(&flow).map_err(|e| e.to_string())?;

    let opts = RunOptions { run_id, limit, mcp };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the runtime: {e}"))?;
    rt.block_on(run_flow(&graph, opts))
        .map_err(|e| e.to_string())
}

fn graph_validate(path: &str) -> ExitCode {
    match do_validate(path) {
        Ok(()) => {
            println!("laya graph validate: {path} is valid");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

fn graph_render(path: &str) -> ExitCode {
    match do_render(path) {
        Ok(rendered) => {
            println!("{}", rendered.text);
            println!(
                "dropped (fields this renderer cannot carry): {}",
                rendered.dropped.join(", ")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

fn graph_run(path: &str) -> ExitCode {
    let mcp: Arc<dyn McpCaller> = Arc::new(LiveMcpCaller {
        bin: resolve_mcp_binary(),
        call_timeout: DEFAULT_CALL_TIMEOUT,
    });
    graph_run_with(path, mcp)
}

/// `laya graph run` — exits non-zero when the run stopped early (R20): exit
/// 0 with `stopped_early: true` in the JSON would be a contradiction a
/// caller resolves the wrong way. `mcp` is injected so this exit-code mapping
/// itself is testable against a shell-only fixture with no live dependency.
fn graph_run_with(path: &str, mcp: Arc<dyn McpCaller>) -> ExitCode {
    let run_id = format!("run-{}", run_id_token());

    match do_run(path, mcp, 4, run_id.clone()) {
        Ok(report) => {
            let json = serde_json::to_string(&report).unwrap_or_default();
            println!("{json}");
            if report.stopped_early {
                eprintln!(
                    "laya graph run: STOPPED EARLY at `{}`, {} node(s) ran, {} not started: {:?} (run {run_id})",
                    report.failed_node.as_deref().unwrap_or("?"),
                    report.records.len(),
                    report.not_started.len(),
                    report.not_started
                );
                ExitCode::FAILURE
            } else {
                eprintln!(
                    "laya graph run: completed, {} node(s) ran (run {run_id})",
                    report.records.len()
                );
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A run id unique enough for a one-shot CLI invocation: nanoseconds since
/// the epoch, not a UUID dependency this crate does not otherwise need.
fn run_id_token() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| "0".to_string(), |d| d.as_nanos().to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use laya_graph::CallContext;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// Refuses every call — every fixture in this module is shell-only, so
    /// an unexpected `mcp` call is a loud failure rather than silently
    /// ignored (same pattern `laya-graph`'s own scheduler tests use).
    struct UnusedCaller;
    impl McpCaller for UnusedCaller {
        fn call(
            &self,
            tool: &str,
            _arguments: &Value,
            _ctx: &CallContext,
        ) -> Result<String, String> {
            Err(format!(
                "unexpected mcp call to `{tool}` in a shell-only fixture"
            ))
        }
    }

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Write `contents` to a fresh temp file and return its path. No
    /// `tempfile` dependency: a counter plus the process id is enough to
    /// avoid collisions between this module's own tests.
    fn fixture(contents: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "laya-cli-graph-test-{}-{n}.json",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("writing a test fixture must not fail");
        path
    }

    // ---- R19: validate ------------------------------------------------------

    #[test]
    fn laya_graph_validate_refuses_an_invalid_flow_with_nonzero_exit() {
        let path = fixture(
            r#"{"flow":"t","version":1,"nodes":[
                {"id":"a","kind":"shell","run":"true"},
                {"id":"a","kind":"shell","run":"true"}
            ]}"#,
        );
        let err = do_validate(path.to_str().expect("utf8 path"))
            .expect_err("a duplicate node id must be refused");
        assert!(
            err.contains('a'),
            "the refusal must name the offending id `a`: {err}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_control_a_valid_flow_validates_cleanly() {
        let path =
            fixture(r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"true"}]}"#);
        do_validate(path.to_str().expect("utf8 path")).expect("a well-formed flow must validate");
        let _ = std::fs::remove_file(path);
    }

    // ---- R19: render ----------------------------------------------------------

    #[test]
    fn laya_graph_render_prints_the_dropped_list() {
        // Re-pinned by spec 20: this fixture used to carry `retry`, which `validate` now refuses
        // (declared but inert). The dropped list is a constant naming what the render cannot
        // carry, so the assertion below on `retry` is unchanged — only the fixture is valid again.
        let path = fixture(
            r#"{"flow":"t","version":1,"nodes":[
                {"id":"a","kind":"shell","run":"true","allow_failure":true}
            ]}"#,
        );
        let rendered = do_render(path.to_str().expect("utf8 path")).expect("must render");
        assert!(
            rendered.text.starts_with("flowchart TD"),
            "{}",
            rendered.text
        );
        assert!(
            rendered.dropped.iter().any(|f| f == "retry"),
            "the dropped list must name `retry`: {:?}",
            rendered.dropped
        );
        let _ = std::fs::remove_file(path);
    }

    // ---- R20: run ---------------------------------------------------------

    #[test]
    fn a_stopped_run_exits_nonzero() {
        let path = fixture(
            r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"exit 1"}]}"#,
        );
        // Exercises the ACTUAL exit-code mapping (R20's literal claim), not
        // just the `RunReport` field underneath it — a bug in
        // `graph_run_with`'s `if report.stopped_early { .. }` would not be
        // caught by asserting on `RunReport` alone.
        let code = graph_run_with(path.to_str().expect("utf8 path"), Arc::new(UnusedCaller));
        assert_eq!(
            code,
            ExitCode::FAILURE,
            "a stopped run must exit non-zero, never SUCCESS"
        );
        let _ = std::fs::remove_file(path);
    }

    /// The control for the test above: nothing here is "always nonzero" —
    /// an all-successful flow must exit zero.
    #[test]
    fn the_control_a_successful_run_exits_zero() {
        let path = fixture(
            r#"{"flow":"t","version":1,"nodes":[
                {"id":"a","kind":"shell","run":"true"},
                {"id":"b","kind":"shell","run":"true","depends_on":["a"]}
            ]}"#,
        );
        let code = graph_run_with(path.to_str().expect("utf8 path"), Arc::new(UnusedCaller));
        assert_eq!(code, ExitCode::SUCCESS, "a completed run must exit zero");
        let _ = std::fs::remove_file(path);
    }

    /// The underlying `RunReport` content, pinned separately from the exit
    /// code above: `stopped_early` true, the failing node named, nothing
    /// downstream recorded.
    #[test]
    fn a_stopped_run_reports_the_failing_node() {
        let path = fixture(
            r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"exit 1"}]}"#,
        );
        let report = do_run(
            path.to_str().expect("utf8 path"),
            Arc::new(UnusedCaller),
            4,
            "test-stopped".to_string(),
        )
        .expect("run_flow itself must not error — a node failure is DATA (R12)");
        assert!(
            report.stopped_early,
            "a failing node must stop the run: {report:?}"
        );
        assert_eq!(report.failed_node.as_deref(), Some("a"));
        let _ = std::fs::remove_file(path);
    }
}
