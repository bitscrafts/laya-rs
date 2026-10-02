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

use laya_graph::{CallContext, McpCaller};
use serde_json::{Value, json};

/// Overrides where `laya-mcp` is found. Unset means "resolve it from `PATH`"
/// — the normal case once it is installed, and what lets a deployment that
/// has not put it on `PATH` still point at it explicitly.
pub const LAYA_MCP_BIN_ENV: &str = "LAYA_MCP_BIN";

/// How long a single JSON-RPC round trip may take before this gives up and
/// reports a labelled failure rather than hang. Generous: `decide` is a real
/// forward pass through the sidecar, not a mock.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_mins(2);

/// How a `laya-mcp` was found — reported by `laya doctor` so the choice is observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSource {
    /// `LAYA_MCP_BIN` was set and resolved.
    Env,
    /// The sibling of the running `laya`.
    Sibling,
    /// A directory on `PATH`.
    Path,
}

impl McpSource {
    /// The word `doctor` prints.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Sibling => "sibling",
            Self::Path => "path",
        }
    }
}

/// A resolved `laya-mcp`: where it is and how it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcp {
    /// The binary to spawn.
    pub bin: PathBuf,
    /// Which resolution step found it.
    pub source: McpSource,
}

/// Where to find the real `laya-mcp`, from the live environment — see [`resolve_mcp_binary_from`].
///
/// # Errors
/// When nothing resolves; the message lists every path tried, in order.
pub fn resolve_mcp_binary() -> Result<ResolvedMcp, String> {
    let env = std::env::var_os(LAYA_MCP_BIN_ENV);
    let exe = std::env::current_exe().ok();
    let path = std::env::var_os("PATH");
    resolve_mcp_binary_from(env.as_deref(), exe.as_deref(), path.as_deref())
}

/// Resolve `laya-mcp` in a fixed order (spec 22): `LAYA_MCP_BIN`, the sibling of `laya`, then `PATH`.
///
/// The inputs are parameters so each step is testable without touching the
/// process environment.
///
/// A `LAYA_MCP_BIN` that is set but does not resolve is an error naming it: it never falls
/// through, because a set variable that is silently ignored is indistinguishable from a wrong one.
///
/// # Errors
/// A set-but-unresolvable `LAYA_MCP_BIN`, or nothing found; the latter lists every path tried.
pub fn resolve_mcp_binary_from(
    env: Option<&std::ffi::OsStr>,
    exe: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
) -> Result<ResolvedMcp, String> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(set) = env {
        let bin = PathBuf::from(set);
        if is_executable_file(&bin) || (!has_dir(&bin) && find_on(&bin, path).is_some()) {
            return Ok(ResolvedMcp {
                bin,
                source: McpSource::Env,
            });
        }
        return Err(format!(
            "{LAYA_MCP_BIN_ENV} is set to `{}` but that does not resolve to a `laya-mcp`; \
             it is not silently ignored — fix or unset it",
            bin.display()
        ));
    }
    tried.push(format!("{LAYA_MCP_BIN_ENV} (unset)"));

    match exe.and_then(Path::parent) {
        Some(dir) => {
            let sibling = dir.join(MCP_BIN_NAME);
            if is_executable_file(&sibling) {
                return Ok(ResolvedMcp {
                    bin: sibling,
                    source: McpSource::Sibling,
                });
            }
            tried.push(format!("sibling {}", sibling.display()));
        }
        None => tried.push("sibling (the running laya's location is unknown)".to_string()),
    }

    if let Some(found) = find_on(Path::new(MCP_BIN_NAME), path) {
        return Ok(ResolvedMcp {
            bin: found,
            source: McpSource::Path,
        });
    }
    tried.push(format!(
        "PATH ({})",
        path.map_or_else(|| "unset".to_string(), |p| p.to_string_lossy().into_owned())
    ));

    Err(format!(
        "could not find `{MCP_BIN_NAME}`; tried, in order: {}. Set {LAYA_MCP_BIN_ENV} to point at it.",
        tried.join("; ")
    ))
}

const MCP_BIN_NAME: &str = "laya-mcp";

fn has_dir(p: &Path) -> bool {
    p.components().count() > 1
}

fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The first executable `name` in a directory of `path`, if any.
fn find_on(name: &Path, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|d| d.join(name))
        .find(|c| is_executable_file(c))
}

/// The `McpCaller` `laya graph run` injects.
///
/// Spawns the real `laya-mcp` over stdio for every `mcp` node, translating a
/// lowered `decide` node's call shape on the way in and reshaping its answer
/// on the way out (see the module docs).
pub struct LiveMcpCaller {
    /// The `laya-mcp` binary to spawn — see [`resolve_mcp_binary`].
    pub bin: PathBuf,
    /// How long one JSON-RPC round trip may take — [`DEFAULT_CALL_TIMEOUT`]
    /// in production; a test shortens it to prove a hung server is killed.
    pub call_timeout: Duration,
}

/// `arguments` with the caller's run and node ids added as `run_id` / `node_id` (spec 11, R1).
///
/// The tool server records them on the ledger row, which is what makes a decision joinable to the
/// run that made it. A non-object `arguments` is left alone: there is nowhere to put them, and a
/// tool that takes a bare value does not record by id.
fn with_context(arguments: &Value, ctx: &CallContext) -> Value {
    let mut out = arguments.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("run_id".to_string(), json!(ctx.run_id));
        obj.insert("node_id".to_string(), json!(ctx.node_id));
    }
    out
}

impl McpCaller for LiveMcpCaller {
    fn call(&self, tool: &str, arguments: &Value, ctx: &CallContext) -> Result<String, String> {
        if tool != "decide" {
            return call_real_mcp(
                &self.bin,
                tool,
                &with_context(arguments, ctx),
                self.call_timeout,
            );
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

        // Spec 16: absent means `choice`; a `noul` must be asked for by name. `validate` refuses
        // anything else, so reaching here with another value is an error, not a guess.
        let answer = match arguments.get("question_type").and_then(Value::as_str) {
            None | Some("choice") => {
                let criteria: serde_json::Map<String, Value> =
                    options.iter().map(|o| (o.clone(), json!(o))).collect();
                json!({"type": "choice", "instructions": question, "criteria": criteria})
            }
            // A `noul` has no options to score: the server refuses `criteria` on one.
            Some("noul") => json!({"type": "noul", "instructions": question}),
            Some(other) => {
                return Err(format!(
                    "decide: unknown question_type `{other}`; expected `choice` or `noul`"
                ));
            }
        };
        let call_args = with_context(
            &json!({
                "state": state,
                "questions": {"answer": answer}
            }),
            ctx,
        );

        let raw = call_real_mcp(&self.bin, "decide", &call_args, self.call_timeout)?;
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
fn read_message(
    rx: &mpsc::Receiver<std::io::Result<String>>,
    call_timeout: Duration,
) -> Result<Value, String> {
    let line = rx
        .recv_timeout(call_timeout)
        .map_err(|e| match e {
            // A std mpsc channel with no sender left returns `Disconnected`
            // IMMEDIATELY, never waiting for the timeout — so this must stay
            // a distinct message. Measured: reporting this as "within
            // {call_timeout:?}" when laya-mcp actually exited in 16ms is
            // exactly the class of lying diagnostic this project exists to
            // avoid (it would read as a hang that never happened).
            mpsc::RecvTimeoutError::Timeout => {
                format!("no response from laya-mcp within {call_timeout:?} (it may be hung)")
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
fn call_real_mcp(
    bin: &Path,
    tool: &str,
    arguments: &Value,
    call_timeout: Duration,
) -> Result<String, String> {
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
        reap(&mut child, true);
        return Err("laya-mcp: stdin was not piped".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        reap(&mut child, true);
        return Err("laya-mcp: stdout was not piped".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        reap(&mut child, true);
        return Err("laya-mcp: stderr was not piped".to_string());
    };

    let stderr_lines = collect_stderr(stderr);

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
        let init_resp = read_message(&rx, call_timeout)?;
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
        let resp = read_message(&rx, call_timeout)?;

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
    // Every `Err` kills before it waits (R4): a server that is hung rather
    // than slow ignores stdin EOF, and `wait()` alone would then never return.
    reap(&mut child, result.is_err());

    result.map_err(|e| {
        let stderr_text = stderr_lines
            .lock()
            .map(|v| v.join("\n"))
            .unwrap_or_default();
        format!("{e}\n--- laya-mcp stderr ---\n{stderr_text}")
    })
}

/// Drain `stderr` on a background thread, so a chatty server cannot block on a
/// full pipe and the lines are there to report if the call fails.
fn collect_stderr(stderr: std::process::ChildStderr) -> Arc<Mutex<Vec<String>>> {
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut v) = sink.lock() {
                v.push(line);
            }
        }
    });
    lines
}

/// Optionally kill `child`, then wait for it so it is never left behind.
fn reap(child: &mut Child, kill: bool) {
    if kill {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn write_line(stdin: &mut std::process::ChildStdin, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    fn ctx() -> CallContext {
        CallContext {
            run_id: "run-7".to_string(),
            node_id: "triage".to_string(),
        }
    }

    /// Write an executable `laya-mcp` stand-in into a fresh temp dir.
    fn fake_server(tag: &str, script: &str) -> (PathBuf, PathBuf) {
        // The scratch dir must be unique PER CALL, not per process. It was `{tag}-{pid}`, which is
        // unique only while every tag in this file stays distinct -- a property maintained by hand in a
        // file that grows. A collision means one test writes the fake server while another `exec`s the
        // same path, and the kernel refuses with `Text file busy` (ETXTBSY, os error 26).
        //
        // MEASURED 2026-10-02: one such failure in ~20 runs of this binary, load-dependent, never
        // reproduced in 12 quiet runs + 8 runs of the affected test alone. The MECHANISM was not
        // established -- every tag in the file is in fact distinct -- so the counter is not a fix for a
        // proven collision. It removes the possibility by construction rather than by discipline.
        static NEXT_DIR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = NEXT_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "laya-mcp-client-{tag}-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        let bin = dir.join("fake-laya-mcp");
        let pid_file = dir.join("pid");
        let script = script.replace("{PID}", &pid_file.display().to_string());
        std::fs::write(&bin, script).expect("write script");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        (bin, pid_file)
    }

    /// Run `call` on its own thread and fail if it has not returned in 5 s —
    /// a `wait()` that never returns must read as a failure, not a hang.
    fn within_5s<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(call());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the call did not return within 5s")
    }

    fn is_gone(pid: &str) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
            stat.rsplit(") ")
                .next()
                .is_some_and(|rest| rest.starts_with('Z'))
        })
    }

    /// A `laya-mcp` stand-in that answers every call with `text`.
    fn answering(tag: &str, text: &str) -> PathBuf {
        let script = format!(
            "#!/bin/sh\n\
             read init; echo '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}'\n\
             read note; read call\n\
             echo '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}'\n"
        );
        fake_server(tag, &script).0
    }

    /// Rename a fake into `dir` under the real name `laya-mcp`.
    fn as_laya_mcp(fake: &Path) -> PathBuf {
        let dir = fake.parent().expect("parent").to_path_buf();
        let real = dir.join("laya-mcp");
        std::fs::rename(fake, &real).expect("rename");
        real
    }

    /// Spawn what `resolved` names and return what answered.
    fn who_answers(resolved: &ResolvedMcp) -> String {
        let caller = LiveMcpCaller {
            bin: resolved.bin.clone(),
            call_timeout: Duration::from_secs(5),
        };
        within_5s(move || caller.call("anything", &json!({}), &ctx())).expect("server answers")
    }

    #[test]
    fn the_sibling_is_preferred_over_path() {
        let sibling = as_laya_mcp(&answering("sib", "from-sibling"));
        let on_path = as_laya_mcp(&answering("pathdir", "from-path"));
        let exe = sibling.parent().expect("dir").join("laya");
        let path = on_path.parent().expect("dir").as_os_str().to_owned();
        let got = resolve_mcp_binary_from(None, Some(&exe), Some(&path)).expect("resolves");
        assert_eq!(got.source, McpSource::Sibling);
        assert_eq!(who_answers(&got), "from-sibling");
    }

    #[test]
    fn the_control_path_is_used_when_there_is_no_sibling() {
        let on_path = as_laya_mcp(&answering("pathonly", "from-path"));
        let lonely = std::env::temp_dir().join(format!("laya-no-sibling-{}", std::process::id()));
        std::fs::create_dir_all(&lonely).expect("dir");
        let exe = lonely.join("laya");
        let path = on_path.parent().expect("dir").as_os_str().to_owned();
        let got = resolve_mcp_binary_from(None, Some(&exe), Some(&path)).expect("resolves");
        assert_eq!(got.source, McpSource::Path);
        assert_eq!(who_answers(&got), "from-path");
    }

    #[test]
    fn a_set_but_unresolvable_laya_mcp_bin_fails_by_name() {
        // A sibling AND a PATH entry exist: a silent fall-through would succeed.
        let sibling = as_laya_mcp(&answering("sib2", "from-sibling"));
        let exe = sibling.parent().expect("dir").join("laya");
        let err = resolve_mcp_binary_from(
            Some("/nope/laya-mcp".as_ref()),
            Some(&exe),
            Some(sibling.parent().expect("dir").as_os_str()),
        )
        .expect_err("must not fall through");
        assert!(err.contains("/nope/laya-mcp"), "{err}");
        assert!(err.contains(LAYA_MCP_BIN_ENV), "{err}");
    }

    #[test]
    fn the_control_a_resolvable_laya_mcp_bin_is_used() {
        let env_bin = answering("envbin", "from-env");
        let sibling = as_laya_mcp(&answering("sib3", "from-sibling"));
        let exe = sibling.parent().expect("dir").join("laya");
        let got =
            resolve_mcp_binary_from(Some(env_bin.as_os_str()), Some(&exe), None).expect("resolves");
        assert_eq!(got.source, McpSource::Env);
        assert_eq!(who_answers(&got), "from-env");
    }

    #[test]
    fn the_error_lists_every_path_tried() {
        let empty = std::env::temp_dir().join(format!("laya-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).expect("dir");
        let exe = empty.join("laya");
        let err = resolve_mcp_binary_from(None, Some(&exe), Some(empty.as_os_str()))
            .expect_err("nothing resolves");
        assert!(err.contains(LAYA_MCP_BIN_ENV), "{err}");
        assert!(
            err.contains(&empty.join("laya-mcp").display().to_string()),
            "{err}"
        );
        assert!(err.contains("PATH"), "{err}");
    }

    #[test]
    fn a_hung_server_is_killed_not_awaited_forever() {
        // Reads nothing, answers nothing, ignores stdin EOF: `exec` makes the
        // recorded pid the process that hangs.
        let (bin, pid_file) = fake_server("hung", "#!/bin/sh\necho $$ > {PID}\nexec sleep 600\n");
        let caller = LiveMcpCaller {
            bin,
            call_timeout: Duration::from_secs(1),
        };
        let started = Instant::now();
        let result = within_5s(move || caller.call("anything", &json!({}), &ctx()));
        let err = result.expect_err("a hung server must be an error");
        assert!(err.contains("no response"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));

        let pid = std::fs::read_to_string(&pid_file)
            .expect("pid")
            .trim()
            .to_string();
        assert!(is_gone(&pid), "the hung server {pid} was left running");
    }

    #[test]
    fn the_control_a_server_that_answers_is_not_an_error() {
        // A near miss for "kill on error": a server that completes the
        // handshake and answers inside the deadline must return its text.
        let script = "#!/bin/sh\n\
            read init; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n\
            read note; read call\n\
            echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"pong\"}]}}'\n";
        let (bin, _) = fake_server("answers", script);
        let caller = LiveMcpCaller {
            bin,
            call_timeout: Duration::from_secs(5),
        };
        let result = within_5s(move || caller.call("anything", &json!({}), &ctx()));
        assert_eq!(result.as_deref(), Ok("pong"));
    }

    #[test]
    fn the_run_and_node_id_reach_the_server_as_tool_arguments() {
        // R1 at the wire: what the server RECEIVES. The stand-in writes the `tools/call` line it
        // was sent, so the assertion is about the bytes on the pipe and not about a helper.
        let script = "#!/bin/sh\n\
            read init; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n\
            read note; read call\n\
            echo \"$call\" > {PID}.call\n\
            echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}'\n";
        let (bin, pid_file) = fake_server("ctx", script);
        let caller = LiveMcpCaller {
            bin,
            call_timeout: Duration::from_secs(5),
        };
        let result = within_5s(move || caller.call("gate", &json!({"action": "ls"}), &ctx()));
        assert_eq!(result.as_deref(), Ok("ok"));

        let sent = std::fs::read_to_string(format!("{}.call", pid_file.display())).expect("call");
        let sent: Value = serde_json::from_str(sent.trim()).expect("json");
        let arg = |k: &str| sent.pointer(&format!("/params/arguments/{k}")).cloned();
        assert_eq!(arg("run_id"), Some(json!("run-7")), "{sent}");
        assert_eq!(arg("node_id"), Some(json!("triage")), "{sent}");
        assert_eq!(
            arg("action"),
            Some(json!("ls")),
            "the caller's own arguments survive: {sent}"
        );
    }

    #[test]
    fn the_decide_translation_also_carries_the_ids() {
        // `decide` is rebuilt from `{question, options, prompt}`, so it is the path most likely to
        // drop the ids: the arguments are not the caller's, they are made here.
        let script = "#!/bin/sh\n\
            read init; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n\
            read note; read call\n\
            echo \"$call\" > {PID}.call\n\
            echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"payloads\\\":{\\\"answer\\\":{\\\"label\\\":\\\"a\\\"}}}\"}]}}'\n";
        let (bin, pid_file) = fake_server("ctx-decide", script);
        let caller = LiveMcpCaller {
            bin,
            call_timeout: Duration::from_secs(5),
        };
        let result = within_5s(move || {
            caller.call(
                "decide",
                &json!({"question": "q", "options": ["a", "b"], "prompt": "p"}),
                &ctx(),
            )
        });
        assert!(result.is_ok(), "{result:?}");
        let sent = std::fs::read_to_string(format!("{}.call", pid_file.display())).expect("call");
        let sent: Value = serde_json::from_str(sent.trim()).expect("json");
        assert_eq!(
            sent.pointer("/params/arguments/run_id"),
            Some(&json!("run-7")),
            "{sent}"
        );
        assert_eq!(
            sent.pointer("/params/arguments/node_id"),
            Some(&json!("triage")),
            "{sent}"
        );
    }

    /// The `tools/call` line a `decide` with these arguments puts on the wire (spec 16).
    fn decide_wire(tag: &str, args: Value) -> (Result<String, String>, Option<Value>) {
        let script = "#!/bin/sh\n\
            read init; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n\
            read note; read call\n\
            echo \"$call\" > {PID}.call\n\
            echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"payloads\\\":{\\\"answer\\\":{\\\"label\\\":\\\"yes\\\"}}}\"}]}}'\n";
        let (bin, pid_file) = fake_server(tag, script);
        let caller = LiveMcpCaller {
            bin,
            call_timeout: Duration::from_secs(5),
        };
        let result = within_5s(move || caller.call("decide", &args, &ctx()));
        let sent = std::fs::read_to_string(format!("{}.call", pid_file.display()))
            .ok()
            .and_then(|s| serde_json::from_str(s.trim()).ok());
        (result, sent)
    }

    /// Spec 16, the default: no `question_type` is a `choice` with `criteria` keyed by option.
    /// Control for the noul row — a change that made everything a `noul` fails here.
    #[test]
    fn a_flow_without_question_type_still_asks_a_choice() {
        let args = json!({"question": "q", "options": ["a", "b"], "prompt": "p"});
        let (result, sent) = decide_wire("s16-default", args);
        assert!(result.is_ok(), "{result:?}");
        let sent = sent.expect("call line");
        assert_eq!(
            sent.pointer("/params/arguments/questions/answer"),
            Some(&json!({"type": "choice", "instructions": "q", "criteria": {"a": "a", "b": "b"}})),
            "{sent}"
        );
    }

    /// Spec 16: the same with `question_type: "choice"` named — criteria present, keyed by option.
    #[test]
    fn the_control_a_choice_decide_still_carries_its_options() {
        let args = json!({"question": "q", "options": ["a", "b"], "prompt": "p",
            "question_type": "choice"});
        let (result, sent) = decide_wire("s16-choice", args);
        assert!(result.is_ok(), "{result:?}");
        let sent = sent.expect("call line");
        assert_eq!(
            sent.pointer("/params/arguments/questions/answer/type"),
            Some(&json!("choice")),
            "{sent}"
        );
        assert_eq!(
            sent.pointer("/params/arguments/questions/answer/criteria"),
            Some(&json!({"a": "a", "b": "b"})),
            "{sent}"
        );
    }

    /// Spec 16 R2: a `noul` goes out as `type=noul` with NO `criteria` key at all.
    #[test]
    fn a_noul_decide_sends_no_criteria() {
        let args = json!({"question": "Is it big?", "options": [], "prompt": "p",
            "question_type": "noul"});
        let (result, sent) = decide_wire("s16-noul", args);
        assert!(result.is_ok(), "{result:?}");
        let sent = sent.expect("call line");
        let answer = sent
            .pointer("/params/arguments/questions/answer")
            .cloned()
            .expect("answer question");
        assert_eq!(answer.get("type"), Some(&json!("noul")), "{sent}");
        assert_eq!(
            answer.get("instructions"),
            Some(&json!("Is it big?")),
            "{sent}"
        );
        assert!(answer.get("criteria").is_none(), "no criteria: {sent}");
    }

    /// Spec 16 R3/R4: an unknown type that reaches the caller is an error naming it, and
    /// nothing is sent.
    #[test]
    fn an_unknown_question_type_is_refused_by_name() {
        let args = json!({"question": "q", "options": ["a"], "prompt": "p",
            "question_type": "yolo"});
        let (result, sent) = decide_wire("s16-yolo", args);
        let err = result.expect_err("an unknown question_type must be refused");
        assert!(err.contains("yolo"), "must name the value: {err}");
        assert!(sent.is_none(), "nothing may reach the server: {sent:?}");
    }

    /// Spec 20 R4, wire level. Each `decide` field a flow can set is observable in the BYTES the
    /// server receives: two calls differing only in that field must send different values at the
    /// pointer named here. A field the translation dropped would fail its row.
    #[test]
    #[allow(clippy::indexing_slicing)]
    fn every_decide_field_reaches_the_wire() {
        // (field, call A, call B, where it lands in the `tools/call` params)
        let base = json!({"question": "q", "options": ["a", "b"], "prompt": "p"});
        let with = |field: &str, value: Value| {
            let mut v = base.clone();
            v[field] = value;
            v
        };
        let table: [(&str, Value, Value, &str); 4] = [
            (
                "question",
                with("question", json!("one?")),
                with("question", json!("two?")),
                "/params/arguments/questions/answer/instructions",
            ),
            (
                "options",
                with("options", json!(["a", "b"])),
                with("options", json!(["a", "c"])),
                "/params/arguments/questions/answer/criteria",
            ),
            (
                "prompt",
                with("prompt", json!("state one")),
                with("prompt", json!("state two")),
                "/params/arguments/state",
            ),
            (
                "question_type",
                with("question_type", json!("choice")),
                with("question_type", json!("noul")),
                "/params/arguments/questions/answer/type",
            ),
        ];
        let script = "#!/bin/sh\n\
            read init; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n\
            read note; read call\n\
            echo \"$call\" > {PID}.call\n\
            echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"payloads\\\":{\\\"answer\\\":{\\\"label\\\":\\\"a\\\"}}}\"}]}}'\n";
        for (field, a, b, pointer) in table {
            let mut seen = Vec::new();
            for (n, args) in [a, b].into_iter().enumerate() {
                let (bin, pid_file) = fake_server(&format!("wire-{field}-{n}"), script);
                let caller = LiveMcpCaller {
                    bin,
                    call_timeout: Duration::from_secs(5),
                };
                let result = within_5s(move || caller.call("decide", &args, &ctx()));
                assert!(result.is_ok(), "{field}: {result:?}");
                let sent =
                    std::fs::read_to_string(format!("{}.call", pid_file.display())).expect("call");
                let sent: Value = serde_json::from_str(sent.trim()).expect("json");
                seen.push(sent.pointer(pointer).cloned());
            }
            assert!(seen[0].is_some(), "{field}: nothing at {pointer}");
            assert_ne!(
                seen[0], seen[1],
                "`{field}` did not reach the wire at {pointer}"
            );
        }
    }
}
