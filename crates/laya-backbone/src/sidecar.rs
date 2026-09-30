//! The sidecar client — how `laya-mcp` reaches the decision head.
//!
//! The head needs `torch` (it is not part of the GGUF), so it cannot live in this
//! process. It runs in `scripts/laya-sidecar.py`, a **persistent, pinned** process
//! that speaks newline-delimited JSON over a **unix socket**.
//!
//! ## Why a unix socket and not a port
//!
//! A TCP port is reachable from anywhere on the network; a socket file is reachable
//! only by something that can open that path. The sidecar answers arbitrary
//! natural-language questions about arbitrary state, so it must not be
//! network-reachable. Mode 0600, owner only.
//!
//! ## Why the process must persist
//!
//! Measured: loading the Python stack takes **~24 s**. A per-call spawn would pay
//! that on every decision (blocker B3's failure shape). A warm decision is **~0.7 s**.
//! So this client ASSUMES a running sidecar and fails with an actionable message when
//! there is none — it never silently spawns one per call.
//!
//! ## Status — PROOF OF CONCEPT
//!
//! The client talks the real protocol to the real sidecar. It is deliberately minimal:
//! `health` and `predict`, nothing more. It does not yet run the answer through
//! `laya_core::DecisionPayload` or write to the ledger — those are the next steps, and
//! they are Rust work with no unknowns left in them.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The default socket path, matching `scripts/laya-sidecar.py`'s default.
pub const DEFAULT_SOCKET: &str = "/run/laya/sidecar.sock";

/// How long to wait for a *warm* decision.
///
/// Generous because the first call after a restart can still be paying for a
/// tokenizer pass; a cold LOAD is not waited for here (that is the sidecar's own
/// startup, which the readiness check covers).
pub const PREDICT_TIMEOUT: Duration = Duration::from_mins(2);

/// What went wrong talking to the sidecar.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// The socket is not there, or nobody is listening.
    ///
    /// Carries the actionable fix, because the fix is not obvious from "connection
    /// refused" — the sidecar is a script that must be started.
    #[error(
        "the Laya sidecar is not reachable at {path}\n\
         \x20 the decision head needs python+torch, so it runs as a separate process.\n\
         \x20 start it with:\n\
         \x20   /projects/laya-rs/.venv-laya/bin/python /projects/laya-rs/scripts/laya-sidecar.py &\n\
         \x20 (it takes ~24 s to load; `laya doctor` covers the backbone, not the head)"
    )]
    Unreachable {
        /// The socket path we tried.
        path: String,
    },
    /// The transport failed mid-request.
    #[error("sidecar i/o failed: {0}")]
    Io(String),
    /// The sidecar answered with an error, or with something unparseable.
    #[error("sidecar returned an error: {0}")]
    Protocol(String),
}

/// A running sidecar.
#[derive(Debug, Clone)]
pub struct Sidecar {
    path: PathBuf,
}

impl Sidecar {
    /// Point at a socket.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Point at the default socket.
    #[must_use]
    pub fn default_path() -> Self {
        Self::new(DEFAULT_SOCKET)
    }

    /// The socket path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Send one request and read one response line.
    fn request(&self, req: &serde_json::Value) -> Result<serde_json::Value, SidecarError> {
        let stream = UnixStream::connect(&self.path).map_err(|_| SidecarError::Unreachable {
            path: self.path.display().to_string(),
        })?;
        let _ = stream.set_read_timeout(Some(PREDICT_TIMEOUT));
        let _ = stream.set_write_timeout(Some(PREDICT_TIMEOUT));

        let mut writer = stream
            .try_clone()
            .map_err(|e| SidecarError::Io(e.to_string()))?;
        writer
            .write_all(format!("{req}\n").as_bytes())
            .map_err(|e| SidecarError::Io(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| SidecarError::Io(e.to_string()))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| SidecarError::Io(e.to_string()))?;
        if line.trim().is_empty() {
            return Err(SidecarError::Protocol("no response line".into()));
        }
        serde_json::from_str(&line).map_err(|e| SidecarError::Protocol(e.to_string()))
    }

    /// Ask the sidecar about itself.
    ///
    /// # Errors
    /// [`SidecarError`] when the sidecar is unreachable or answers with an error.
    pub fn health(&self) -> Result<serde_json::Value, SidecarError> {
        let r = self.request(&serde_json::json!({"op": "health"}))?;
        if r.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(r)
        } else {
            Err(SidecarError::Protocol(r.to_string()))
        }
    }

    /// Ask typed questions about a state.
    ///
    /// Returns the raw `answers` object. Turning that into a
    /// `laya_core::DecisionPayload` (label + band + confidence + margin) is the next
    /// step and is deliberately NOT done here, so this client stays a faithful
    /// transport and the payload shape has exactly one implementation.
    ///
    /// # Errors
    /// [`SidecarError`] when the sidecar is unreachable, errors, or answers without
    /// an `answers` object.
    pub fn predict(
        &self,
        state: &serde_json::Value,
        questions: &serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        let r = self.request(&serde_json::json!({
            "op": "predict",
            "state": state,
            "questions": questions,
        }))?;
        if r.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(SidecarError::Protocol(
                r.get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown sidecar error")
                    .to_string(),
            ));
        }
        r.get("answers")
            .cloned()
            .ok_or_else(|| SidecarError::Protocol("no `answers` object in the response".into()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;

    /// A socket path that cannot exist, so the unreachable path is testable offline.
    fn missing() -> Sidecar {
        Sidecar::new("/tmp/laya-sidecar-does-not-exist-4f2a.sock")
    }

    #[test]
    fn an_unreachable_sidecar_names_the_fix_not_just_the_failure() {
        // The whole value of this error: "connection refused" would send a reader to
        // debug the network, when the real answer is "the sidecar is a script; start
        // it". The message must contain the command.
        let e = match missing().health() {
            Ok(v) => panic!("an unreachable sidecar must not answer: {v}"),
            Err(e) => e,
        };
        let msg = format!("{e}");
        assert!(msg.contains("not reachable"), "{msg}");
        assert!(
            msg.contains("laya-sidecar.py"),
            "must name the script: {msg}"
        );
        assert!(
            msg.contains(".venv-laya"),
            "must name the pinned interpreter: {msg}"
        );
        assert!(msg.contains("24 s"), "must set the load expectation: {msg}");
    }

    #[test]
    fn an_unreachable_sidecar_is_unreachable_not_a_protocol_error() {
        // The two call for different remedies, so they must not collapse: unreachable
        // means "start it", protocol means "it answered and disagreed".
        assert!(matches!(
            missing().health(),
            Err(SidecarError::Unreachable { .. })
        ));
    }

    #[test]
    fn predict_on_an_unreachable_sidecar_also_names_the_fix() {
        let e = match missing().predict(&serde_json::json!({"s": 1}), &serde_json::json!({})) {
            Ok(v) => panic!("an unreachable sidecar must not answer: {v}"),
            Err(e) => e,
        };
        assert!(format!("{e}").contains("laya-sidecar.py"));
    }

    #[test]
    fn the_default_socket_matches_the_sidecars_default() {
        // A drift here is a silent wiring failure: the server would look for a socket
        // the sidecar never creates. Pinned against the script's own default.
        assert_eq!(DEFAULT_SOCKET, "/run/laya/sidecar.sock");
        assert_eq!(Sidecar::default_path().path(), Path::new(DEFAULT_SOCKET));
    }

    #[test]
    fn the_path_is_reportable_for_diagnostics() {
        assert_eq!(
            missing().path().display().to_string(),
            "/tmp/laya-sidecar-does-not-exist-4f2a.sock"
        );
    }
}
