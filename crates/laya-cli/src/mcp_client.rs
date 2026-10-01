//! A minimal JSON-RPC-over-stdio client for the real `laya-mcp` binary.
//!
//! This is the production counterpart of `laya-graph`'s acceptance test
//! client: both speak the same hand-rolled protocol (`initialize` ->
//! `notifications/initialized` -> `tools/call`, newline-delimited JSON), each
//! implemented independently rather than shared, because the test's whole
//! point is to prove the SAME wire protocol a real client uses actually
//! works — a shared helper crate would let the two drift without either
//! noticing.
//!
//! **This is the translation boundary the spec names as the part's real
//! gap**: a `decide`-lowered `mcp` node's arguments (`question`, `options`,
//! `prompt` — see `laya_graph::lower`) are NOT the real `decide` tool's call
//! shape (`state`, `questions` — `laya-mcp/src/lib.rs`). [`LiveMcpCaller`] is
//! the boundary that bridges both directions: translating the call going in,
//! and reshaping the `payloads`-keyed response into the singular
//! `{"payload": {...}}` object `route`'s `$.payload.<field>` grammar expects
//! coming back.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laya_graph::McpCaller;
use serde_json::{Value, json};

/// Overrides where `laya-mcp` is found. Unset means "resolve it from `PATH`"
/// — the normal case once it is installed, and what lets a deployment that
/// has not put it on `PATH` still point at it explicitly.
pub const LAYA_MCP_BIN_ENV: &str = "LAYA_MCP_BIN";

/// How long a single JSON-RPC round trip may take before this gives up and
/// reports a labelled failure rather than hang. Generous: `decide` is a real
/// forward pass through the sidecar, not a mock.
const CALL_TIMEOUT: Duration = Duration::from_mins(2);

/// Where to find the real `laya-mcp` binary: `LAYA_MCP_BIN` if set, else the
/// bare name, resolved from `PATH` by the OS when spawned.
#[must_use]
pub fn resolve_mcp_binary() -> PathBuf {
    std::env::var(LAYA_MCP_BIN_ENV).map_or_else(|_| PathBuf::from("laya-mcp"), PathBuf::from)
}

/// The `McpCaller` `laya graph run` injects.
///
/// Spawns the real `laya-mcp` over stdio for every `mcp` node, translating a
/// lowered `decide` node's call shape on the way in and reshaping its answer
/// on the way out (see the module docs).
pub struct LiveMcpCaller {
    /// The `laya-mcp` binary to spawn — see [`resolve_mcp_binary`].
    pub bin: PathBuf,
}

impl McpCaller for LiveMcpCaller {
    fn call(&self, tool: &str, arguments: &Value) -> Result<String, String> {
        if tool != "decide" {
            return call_real_mcp(&self.bin, tool, arguments);
        }

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
        // `prompt` is the literal state text: nothing in this tree evaluates
        // a `$.inputs.*` reference yet, so `lower::lower_decide` forwards it
        // verbatim.
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

/// Read one newline-delimited JSON-RPC message with a bounded wait, via a
/// background reader thread — a plain blocking `read_line` cannot be given a
/// timeout on a pipe the way a socket can.
fn read_message(rx: &mpsc::Receiver<std::io::Result<String>>) -> Result<Value, String> {
    let line = rx
        .recv_timeout(CALL_TIMEOUT)
        .map_err(|e| match e {
            // A std mpsc channel with no sender left returns `Disconnected`
            // IMMEDIATELY, never waiting for the timeout — so this must stay
            // a distinct message. Measured: reporting this as "within
            // {CALL_TIMEOUT:?}" when laya-mcp actually exited in 16ms is
            // exactly the class of lying diagnostic this project exists to
            // avoid (it would read as a hang that never happened).
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
///
/// The error type is never swallowed into a generic message: every `Err`
/// here carries the specific reason (spawn failure, a protocol error
/// `laya-mcp` returned, a timeout), so the CLI's caller can print the actual
/// diagnosis rather than "something went wrong".
fn call_real_mcp(bin: &Path, tool: &str, arguments: &Value) -> Result<String, String> {
    let mut child: Child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "failed to spawn `{}` (set {LAYA_MCP_BIN_ENV} if it is not on PATH): {e}",
                bin.display()
            )
        })?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err("laya-mcp: stdin was not piped".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        return Err("laya-mcp: stdout was not piped".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        return Err("laya-mcp: stderr was not piped".to_string());
    };

    let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
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
                Ok(0) => break,
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
                "clientInfo": {"name": "laya-cli", "version": env!("CARGO_PKG_VERSION")}
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

    drop(stdin);
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
