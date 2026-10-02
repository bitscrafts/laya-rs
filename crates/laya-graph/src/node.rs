//! Node executors: `shell` and `mcp` (R12).
//!
//! Both report duration and raw output into an [`Executed`] — the raw
//! material [`crate::sched`] turns into a [`crate::run::NodeRecord`]. Neither
//! executor decides what the *run* does next; that is the scheduler's job
//! (fail-fast, `allow_failure`). A non-zero shell exit and an `mcp` tool
//! error are both `success: false` here — "could not determine" is never
//! success.
//!
//! `mcp` calls go through [`McpCaller`], a trait this crate defines and never
//! implements against the real server: `laya-graph` must not depend on
//! `laya-mcp` (N2, G16), so the real wiring is injected by whoever owns both
//! crates (the CLI, a later part). Tests inject a fake.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::process::Command;

/// What running one node produced — never interpreted here as "the run
/// should stop"; that is [`crate::sched::run`]'s decision, using `success`
/// and the node's own `allow_failure`.
#[derive(Debug, Clone)]
pub struct Executed {
    /// Whether the node's own work succeeded. A non-zero shell exit or an
    /// `mcp` tool error both set this `false` — recorded as data, but never
    /// laundered into success (R12).
    pub success: bool,
    /// The shell exit code, when the node was a `shell` node and the process
    /// actually ran to completion. `None` for `mcp` nodes, and for a `shell`
    /// node whose process could not even be spawned.
    pub exit_code: Option<i32>,
    /// Raw stdout (and, for `shell`, stderr appended after it).
    pub output: String,
    /// Set when `success` is `false`: why. Kept as text rather than a typed
    /// error, because the two sources (a process's stderr, a tool's error
    /// string) do not share a vocabulary.
    pub error: Option<String>,
    /// Wall-clock duration of the node's own work, in milliseconds.
    pub duration_ms: u64,
    /// `true` when the node's deadline expired and its work was killed (R5).
    pub timed_out: bool,
}

/// Call an `mcp` tool by name.
///
/// Injected so `laya-graph` never depends on `laya-mcp`: the real
/// implementation lives wherever both crates meet.
pub trait McpCaller: Send + Sync {
    /// Call `tool` with `arguments`.
    ///
    /// # Errors
    /// Any `Err` is a tool error — a failure, not data to interpret later
    /// (R12).
    fn call(&self, tool: &str, arguments: &Value) -> Result<String, String>;
}

/// Run a shell command under an optional deadline (R1, R2).
///
/// The command runs in its own process group (`process_group(0)`), so an
/// expiry can signal the whole group — `sh -c "sleep 600 & wait"` must leave
/// no orphan, or a "timed out" step keeps working after the run moved on.
/// Without a `timeout` the command is unbounded, exactly as before (N2).
///
/// The exit code **is** the result: a non-zero exit is `success: false`, even
/// though it is also recorded as data. An expired deadline is `success:
/// false` with `timed_out: true`.
pub async fn execute_shell(run: &str, timeout: Option<Duration>) -> Executed {
    let start = Instant::now();
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(run)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if timeout.is_some() {
        command.process_group(0);
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return failed(
                format!("failed to spawn `sh -c`: {e}"),
                elapsed_ms(start),
                false,
            );
        }
    };
    // Read before `wait_with_output` consumes the child: it is the process
    // group id, since `process_group(0)` makes the child its own leader.
    let pgid = child.id();

    // Pinned and polled by reference, so an expiry does not drop it: the
    // group is signalled while `sh` is still the child's owner, and only then
    // is the future (and with it `kill_on_drop`) let go.
    let waited = child.wait_with_output();
    tokio::pin!(waited);
    let outcome = match timeout {
        None => Ok((&mut waited).await),
        Some(limit) => tokio::time::timeout(limit, &mut waited).await,
    };
    let duration_ms = elapsed_ms(start);

    match outcome {
        Err(_elapsed) => {
            // Signal the whole group, not just `sh`: its children would
            // otherwise outlive the deadline.
            let signalled = match pgid {
                Some(pgid) => kill_group(pgid).await,
                None => Err("the child had no process id".to_string()),
            };
            let note = signalled.err().map_or_else(String::new, |why| {
                format!(" (killing its process group failed: {why})")
            });
            failed(
                format!("timed out after {}ms{note}", limit_ms(timeout)),
                duration_ms,
                true,
            )
        }
        Ok(Ok(output)) => {
            let success = output.status.success();
            let exit_code = output.status.code();
            let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }
            let error = if success {
                None
            } else {
                Some(exit_code.map_or_else(
                    || "process terminated by signal, no exit code".to_string(),
                    |code| format!("exited {code}"),
                ))
            };
            Executed {
                success,
                exit_code,
                output: combined,
                error,
                duration_ms,
                timed_out: false,
            }
        }
        Ok(Err(e)) => failed(format!("failed to run `sh -c`: {e}"), duration_ms, false),
    }
}

/// `SIGKILL` every process in group `pgid`, by running `kill -KILL -- -<pgid>`.
///
/// The workspace forbids `unsafe_code`, so `libc::kill` is not available; the
/// `kill` utility is the pure-Rust way to signal a group.
async fn kill_group(pgid: u32) -> Result<(), String> {
    let status = Command::new("kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("could not run `kill`: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`kill` exited {status}"))
    }
}

fn limit_ms(timeout: Option<Duration>) -> u128 {
    timeout.map_or(0, |limit| limit.as_millis())
}

/// An [`Executed`] for work that did not succeed and produced no output.
#[must_use]
pub const fn failed(error: String, duration_ms: u64, timed_out: bool) -> Executed {
    Executed {
        success: false,
        exit_code: None,
        output: String::new(),
        error: Some(error),
        duration_ms,
        timed_out,
    }
}

/// Call `tool` through `caller`. A tool error is a failure, never success
/// (R12); the error text is kept, not swallowed.
#[must_use]
pub fn execute_mcp(caller: &dyn McpCaller, tool: &str, arguments: &Value) -> Executed {
    let start = Instant::now();
    let outcome = caller.call(tool, arguments);
    let duration_ms = elapsed_ms(start);

    match outcome {
        Ok(output) => Executed {
            success: true,
            exit_code: None,
            output,
            error: None,
            duration_ms,
            timed_out: false,
        },
        Err(error) => failed(error, duration_ms, false),
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    // ---- R12: shell -----------------------------------------------------

    #[tokio::test]
    async fn a_shell_node_reports_its_exit_code_and_fails() {
        let executed = execute_shell("exit 3", None).await;
        assert!(!executed.success, "a non-zero exit must not be success");
        assert_eq!(executed.exit_code, Some(3));
    }

    #[tokio::test]
    async fn the_control_a_shell_node_exiting_zero_succeeds() {
        // The mirror-image bug this guards: "fail on non-zero exit" is also
        // satisfied by failing EVERYTHING. A command that exits zero must
        // succeed, or that bug would pass unnoticed.
        let executed = execute_shell("exit 0", None).await;
        assert!(executed.success, "a zero exit must succeed");
        assert_eq!(executed.exit_code, Some(0));
    }

    #[tokio::test]
    async fn shell_output_is_captured() {
        let executed = execute_shell("echo hello", None).await;
        assert!(executed.output.contains("hello"), "{executed:?}");
    }

    // ---- R12: mcp ---------------------------------------------------------

    struct OkCaller;
    impl McpCaller for OkCaller {
        fn call(&self, tool: &str, _arguments: &Value) -> Result<String, String> {
            Ok(format!("{tool}: ok"))
        }
    }

    struct ErrCaller;
    impl McpCaller for ErrCaller {
        fn call(&self, _tool: &str, _arguments: &Value) -> Result<String, String> {
            Err("tool blew up".to_string())
        }
    }

    #[test]
    fn an_mcp_node_error_is_not_success() {
        let executed = execute_mcp(&ErrCaller, "decide", &Value::Null);
        assert!(!executed.success);
        assert_eq!(executed.error.as_deref(), Some("tool blew up"));
    }

    #[test]
    fn the_control_an_mcp_node_ok_result_succeeds() {
        let executed = execute_mcp(&OkCaller, "decide", &Value::Null);
        assert!(executed.success);
        assert!(executed.output.contains("ok"));
    }
}
