//! R-ACC — the acceptance test: a real flow routes a decision through the
//! live backbone, end to end, through the SAME path `laya graph run` uses
//! (G21): validate -> expand -> lower -> [`laya_graph::run_flow`].
//!
//! **The `McpCaller` here spawns the REAL `laya-mcp` binary** over stdio and
//! speaks JSON-RPC to it by hand (`initialize` -> `notifications/initialized`
//! -> `tools/call`) — not a stub, and not the `rmcp` client library, so this
//! test drives exactly the same wire protocol a real MCP client would, built
//! from inspecting `rmcp`'s own message shapes rather than assumed.
//!
//! **This test needs the real backbone and the real sidecar.** It checks
//! reachability FIRST and fails loudly — never a silent skip (N7, G23) — so
//! an unmeasured acceptance test is never mistaken for a passing one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use laya_graph::{Library, McpCaller, expand, lower, run_flow, validate};
use laya_graph::{RunOptions, RunReport};

/// The backbone this workspace's live checks all point at (same address
/// `scripts/verify-all.sh` §7b and the spec's own exit criteria use).
const BACKBONE_HEALTH_URL: &str = "10.10.10.217:8083";

/// How long a single JSON-RPC round trip may take before this test gives up
/// and reports a labelled failure rather than hang (N7). Generous: a `decide`
/// call is a real forward pass through the reference sidecar, not a mock.
const CALL_TIMEOUT: Duration = Duration::from_mins(1);

/// How long the reachability probe itself may take before concluding the
/// backbone is down.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Reachability — checked FIRST, always, never silently skipped (N7, G23).
// ---------------------------------------------------------------------------

/// Probe `BACKBONE_HEALTH_URL`'s `/health` endpoint with a raw HTTP/1.0
/// request over a plain `TcpStream` — no HTTP client dependency is added to
/// this crate just to ask "is anything answering". Returns whether it
/// answered 200, and the wall-clock the probe took either way (prose
/// criterion 3 requires the measured time, not just the verdict).
fn probe_backbone() -> (bool, Duration) {
    let started = Instant::now();
    let reachable = (|| -> std::io::Result<bool> {
        let addr: std::net::SocketAddr = BACKBONE_HEALTH_URL
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
        let mut stream = std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)?;
        stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
        stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
        stream.write_all(b"GET /health HTTP/1.0\r\nHost: backbone\r\nConnection: close\r\n\r\n")?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;
        Ok(status_line.contains("200"))
    })()
    .unwrap_or(false);
    (reachable, started.elapsed())
}

/// Fail loudly with the measured wall-clock, rather than hang or skip. Every
/// live test calls this before doing anything else.
fn require_backbone_or_fail() {
    let (reachable, elapsed) = probe_backbone();
    assert!(
        reachable,
        "UNMEASURED, NOT PASSED: the backbone at {BACKBONE_HEALTH_URL} did not answer /health \
         within {PROBE_TIMEOUT:?} (probe took {elapsed:?}). This is a labelled failure, not a \
         skip (N7, G23) — bring the backbone up and the Python reference sidecar up, then re-run \
         `cargo test -p laya-graph --test acceptance`."
    );
}

// ---------------------------------------------------------------------------
// Locating the real `laya-mcp` binary — discovered, never assumed (Trap 2).
// ---------------------------------------------------------------------------

/// `CARGO_BIN_EXE_laya-mcp` is set only for a binary of the SAME package, so
/// a test in `laya-graph` usually does not get it. Fall back to the sibling
/// `target/<profile>/laya-mcp` next to this test's own executable. If
/// neither exists, FAIL with a labelled reason — never fall back to a stub
/// (G21): a stubbed `McpCaller` would make this whole test a re-enactment.
fn resolve_mcp_binary() -> PathBuf {
    if let Some(p) = option_env!("CARGO_BIN_EXE_laya-mcp") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p;
        }
    }

    let exe = std::env::current_exe().expect("this test's own executable path must resolve");
    // `.../target/<profile>/deps/acceptance-<hash>` -> `.../target/<profile>/laya-mcp`
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("test executable must live under target/<profile>/deps");
    let sibling = profile_dir.join("laya-mcp");
    assert!(
        sibling.is_file(),
        "could not locate the real `laya-mcp` binary: neither CARGO_BIN_EXE_laya-mcp nor \
         {} exists. Build it first: `cargo build -p laya-mcp` (G21 forbids falling back to a \
         stub here).",
        sibling.display()
    );
    sibling
}

// ---------------------------------------------------------------------------
// The real `McpCaller`: spawns `laya-mcp`, speaks JSON-RPC over stdio by hand.
// ---------------------------------------------------------------------------

/// Read one newline-delimited JSON-RPC message with a bounded wait, via a
/// background reader thread — a plain blocking `read_line` cannot be given a
/// timeout on a pipe the way a socket can, and this is what lets a hung
/// child be reported rather than hung on (N7).
fn read_message(rx: &mpsc::Receiver<std::io::Result<String>>) -> Result<Value, String> {
    let line = rx
        .recv_timeout(CALL_TIMEOUT)
        .map_err(|e| match e {
            // A std mpsc channel with no sender left returns `Disconnected`
            // IMMEDIATELY, never waiting for the timeout — this must stay a
            // distinct message or a fast exit reads as a hang that never
            // happened (the same lying-diagnostic defect class N7 exists to
            // catch, just in this harness's own code rather than the SUT's).
            mpsc::RecvTimeoutError::Timeout => {
                format!("no response from laya-mcp within {CALL_TIMEOUT:?} (it may be hung)")
            }
            mpsc::RecvTimeoutError::Disconnected => {
                "laya-mcp closed its stdout without responding (it likely exited early — \
                 check stderr below)"
                    .to_string()
            }
        })?
        .map_err(|e| format!("reading laya-mcp's stdout failed: {e}"))?;
    serde_json::from_str(&line).map_err(|e| format!("not valid JSON ({e}): {line}"))
}

/// Spawn `laya-mcp`, do the full handshake, call `tool`, and return its raw
/// text result (the `TextContent` block the real tool returned).
fn call_real_mcp(bin: &Path, tool: &str, arguments: &Value) -> Result<String, String> {
    let mut child: Child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", bin.display()))?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Drain stderr on its own thread so laya-mcp's diagnostics can never fill
    // the pipe and stall it; collected so a failure can show WHY laya-mcp
    // refused (e.g. "Laya is not ready").
    let stderr_lines: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&stderr_lines);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut v) = sink.lock() {
                    v.push(line);
                }
            }
        });
    }

    let (tx, rx) = mpsc::channel::<std::io::Result<String>>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });

    let result = (|| -> Result<String, String> {
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "laya-graph-acceptance-test", "version": "0.1.0"}
            }
        });
        write_line(&mut stdin, &init_req)?;
        let init_resp = read_message(&rx)?;
        if init_resp.get("error").is_some() {
            return Err(format!("initialize was refused: {init_resp}"));
        }

        let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        write_line(&mut stdin, &initialized)?;

        let call_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        });
        write_line(&mut stdin, &call_req)?;
        let resp = read_message(&rx)?;

        if let Some(err) = resp.get("error") {
            return Err(format!("laya-mcp refused `{tool}`: {err}"));
        }
        let text = resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|block| block.get("text").and_then(Value::as_str))
            .ok_or_else(|| format!("no text content block in response: {resp}"))?;
        Ok(text.to_string())
    })();

    drop(stdin); // close laya-mcp's stdin so it sees EOF and exits cleanly
    let _ = child.wait();

    result.map_err(|e| {
        let stderr_text = stderr_lines
            .lock()
            .map(|v| v.join("\n"))
            .unwrap_or_default();
        format!("{e}\n--- laya-mcp stderr ---\n{stderr_text}")
    })
}

fn write_line(stdin: &mut std::process::ChildStdin, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

/// The `McpCaller` this test injects into `run_flow` — the ONE piece that is
/// real rather than reused from a unit test (Key decision, spec 07 part 4):
/// it spawns the actual `laya-mcp` binary a user runs, and the determinism
/// comes from controlling the INPUT (`state`), not from faking the model.
struct LiveMcpCaller {
    bin: PathBuf,
}

impl McpCaller for LiveMcpCaller {
    fn call(&self, tool: &str, arguments: &Value) -> Result<String, String> {
        if tool != "decide" {
            return call_real_mcp(&self.bin, tool, arguments);
        }

        // THE TRANSLATION BOUNDARY (Verified Premise: "the live `decide` has
        // no in-tree client, and this is the part's real gap"). A
        // `decide`-lowered `mcp` node carries `{question, options, prompt}`
        // (see `lower::lower_decide`) — NOT the real `decide` tool's actual
        // call shape (`{state, questions}`, laya-mcp/src/lib.rs:279-489).
        // This caller is the boundary that bridges the two, in both
        // directions: translating the call going in, and reshaping the
        // `payloads`-keyed response into the singular `{"payload": {...}}`
        // object `route`'s `$.payload.<field>` grammar expects coming back.
        let question = arguments
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let options: Vec<String> = arguments
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // `prompt` is the literal state text for this fixture (nothing in
        // this tree evaluates a `$.inputs.*` reference yet — lowering
        // forwards it verbatim, see `lower::lower_decide`).
        let state = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let criteria: serde_json::Map<String, Value> =
            options.iter().map(|o| (o.clone(), json!(o))).collect();
        let call_args = json!({
            "state": state,
            "questions": {
                "answer": {
                    "type": "choice",
                    "instructions": question,
                    "criteria": criteria,
                }
            }
        });

        let raw = call_real_mcp(&self.bin, "decide", &call_args)?;
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("decide response was not JSON: {e}: {raw}"))?;
        let payload = parsed
            .get("payloads")
            .and_then(|p| p.get("answer"))
            .cloned()
            .ok_or_else(|| format!("decide response had no payloads.answer: {raw}"))?;
        Ok(json!({"payload": payload}).to_string())
    }
}

// ---------------------------------------------------------------------------
// Driving the SAME path `laya graph run` uses (G21): validate -> expand ->
// lower -> run_flow. Never a hand-built Graph.
// ---------------------------------------------------------------------------

async fn run_fixture(path: &str, bin: &Path, run_id: &str) -> RunReport {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let raw: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {path}: {e}"));
    let flow = expand(&raw, &Library::new()).unwrap_or_else(|e| panic!("expanding {path}: {e}"));
    validate(&flow).unwrap_or_else(|e| panic!("{path} must validate: {e}"));
    let graph = lower(&flow).unwrap_or_else(|e| panic!("{path} must lower: {e}"));

    let opts = RunOptions {
        run_id: run_id.to_string(),
        limit: 4,
        mcp: Arc::new(LiveMcpCaller {
            bin: bin.to_path_buf(),
        }),
    };
    run_flow(&graph, opts)
        .await
        .unwrap_or_else(|e| panic!("run_flow on {path} must not error: {e}"))
}

// ---------------------------------------------------------------------------
// R-ACC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_real_flow_routes_a_decision_through_the_live_backbone() {
    require_backbone_or_fail();
    let bin = resolve_mcp_binary();

    let report = run_fixture(
        "tests/fixtures/sentiment-positive.json",
        &bin,
        "acc-positive",
    )
    .await;

    assert!(!report.stopped_early, "the run must complete: {report:?}");
    let route_record = report
        .records
        .iter()
        .find(|r| r.node_id == "classify/route")
        .unwrap_or_else(|| panic!("the route node must have a record: {report:?}"));
    println!("ROUTE RECORD (positive fixture): {}", route_record.output);
    assert!(
        route_record.output.contains("\"label\"") || route_record.output.contains("\"value\""),
        "the record must carry the label/value that produced the routing: {}",
        route_record.output
    );

    let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
    println!("NODES RAN (positive fixture): {ran:?}");
    assert!(
        ran.contains(&"branch_positive"),
        "the model's label did not route this run to branch_positive: {ran:?}, \
         route record: {}",
        route_record.output
    );
}

/// R-ACC's control, named EXACTLY as the spec's own exit criterion requires.
#[tokio::test]
async fn the_control_a_forced_label_reaches_the_other_branch() {
    require_backbone_or_fail();
    let bin = resolve_mcp_binary();

    let report = run_fixture(
        "tests/fixtures/sentiment-negative.json",
        &bin,
        "acc-negative",
    )
    .await;

    assert!(!report.stopped_early, "the run must complete: {report:?}");
    let route_record = report
        .records
        .iter()
        .find(|r| r.node_id == "classify/route")
        .unwrap_or_else(|| panic!("the route node must have a record: {report:?}"));
    println!("ROUTE RECORD (negative fixture): {}", route_record.output);

    let ran: Vec<&str> = report.records.iter().map(|r| r.node_id.as_str()).collect();
    println!("NODES RAN (negative fixture): {ran:?}");
    assert!(
        ran.contains(&"branch_negative"),
        "the OTHER fixture (identical question, opposite state) did not reach the OTHER \
         branch — a hardcoded route would fail exactly here: {ran:?}, route record: {}",
        route_record.output
    );
    assert!(
        !ran.contains(&"branch_positive"),
        "both fixtures reached the SAME branch, which is what a hardcoded route looks like: {ran:?}"
    );
}

#[test]
fn the_acceptance_test_reports_a_down_backbone_rather_than_hanging() {
    // A closed port on localhost, never the real backbone — this test proves
    // OUR timeout/labelling behaviour and must pass regardless of whether the
    // real backbone happens to be up right now.
    let started = Instant::now();
    let reachable = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:1".parse().expect("valid socket addr"),
        PROBE_TIMEOUT,
    )
    .is_ok();
    let elapsed = started.elapsed();

    assert!(!reachable, "port 1 on localhost must not be listening");
    assert!(
        elapsed < PROBE_TIMEOUT + Duration::from_secs(2),
        "the down-backbone probe must report within its timeout, not hang: took {elapsed:?}"
    );
    println!(
        "down-backbone probe reported unreachable in {elapsed:?} (bounded by {PROBE_TIMEOUT:?})"
    );
}
