//! `laya-backbone` — the only crate that talks to the network.
//!
//! Two responsibilities:
//!
//! 1. **Readiness** (spec 05, mandatory — user directive *"it will be mandatory the
//!    app to check for laya be running! and it must inform/stop if not!"*): probe the
//!    Laya endpoint and return a [`ProbeOutcome`] for
//!    [`laya_core::readiness::judge`] to rule on. The *judgement* is pure and lives
//!    in `laya-core`; this crate supplies the observation.
//! 2. **The backbone call**: `POST /embedding` to llama.cpp, returning hidden states.
//!    No FFI — llama.cpp serves the encoder, Rust speaks HTTP.
//!
//! ## What was measured before this was written (2026-09-29)
//!
//! ```text
//! /health                          -> {"status":"ok"}   HTTP 200
//! /v1/models                       -> /models/laya-Q8_0.gguf, capabilities ["completion"]
//! /embedding                       -> 35 ms median, 103 KB of hidden states
//! 8 concurrent /embedding          -> 121 ms wall  => 5.8x parallel, NOT serialised
//! ```
//!
//! Two consequences for this module:
//!
//! * **The capability string is `["completion"]`, not `"embedding"`.** A readiness
//!   check that insists on an `"embedding"` capability would report a perfectly
//!   working server as broken. The model *id* is the reliable signal, so that is what
//!   [`parse_models`] checks.
//! * **Probing is cheap** (~35 ms), so a re-check with a short cache costs little;
//!   but it still must be cached, because it runs per tool call.
//!
//! ## Constraints
//!
//! * **A timeout is not a hang** — every request is bounded, and expiry becomes
//!   [`ProbeOutcome::TimedOut`], never an unhandled error.
//! * **A 200 is not proof of health** — an unparseable body is
//!   [`ProbeOutcome::MalformedBody`], because treating a 200 as healthy is how a
//!   server reports ready and then fails every decision.
//! * **Never log a key.** The endpoint may be logged; a credential never.

#![forbid(unsafe_code)]

pub mod sidecar;

pub use sidecar::{DEFAULT_SOCKET, Sidecar, SidecarError};

pub use laya_core::readiness::{ProbeOutcome, Readiness, ReadinessFailure, judge};

/// The default Laya endpoint in this workspace (container `llamacpp-laya`).
pub const DEFAULT_ENDPOINT: &str = "http://10.10.10.217:8083";

/// How long a readiness probe may take before it is judged a timeout.
///
/// Bounded on purpose: the check runs at startup and once per cache window, so it
/// must never be able to hang the server.
pub const PROBE_TIMEOUT_MS: u64 = 2000;

/// The model id Laya serves in this workspace.
///
/// Checked by [`parse_models`] because the capability string is `"completion"` (the
/// GGUF holds the backbone only), so the id is the signal that the right model —
/// not some other one — is loaded.
pub const EXPECTED_MODEL: &str = "laya-Q8_0.gguf";

/// Probe the **decision head** (the sidecar) and judge it.
///
/// Why this is separate from the backbone probe: they fail independently and need
/// different remedies. The backbone is a container (`llamacpp-laya`); the head is a
/// Python process (~24 s to load) holding the tensors the GGUF does not contain.
/// Restarting the backbone when the head is dead is wasted work, and — worse —
/// reporting READY in that state certifies a decision layer that cannot decide.
///
/// `socket` is the sidecar's unix socket. A missing socket is
/// [`ProbeOutcome::HeadUnavailable`], never a bare connection error, because the
/// reader needs to know the backbone is fine.
#[must_use]
pub fn probe_head(socket: &std::path::Path) -> ProbeOutcome {
    // No socket file at all is the common case (the sidecar was never started, or the
    // container restarted and /run is tmpfs). Say so without paying a connect timeout.
    if !socket.exists() {
        return ProbeOutcome::HeadUnavailable(format!(
            "no sidecar socket at {} — the head was never started, or the container \
             restarted and its tmpfs /run was emptied",
            socket.display()
        ));
    }
    match Sidecar::new(socket).health() {
        Ok(h) => {
            // A live socket that answers `ok` is not quite enough: we want the head's
            // own report on its weights. A head that loaded nothing is running and
            // useless, and that must not read as ready.
            //
            // Field names are the sidecar's own, read from a LIVE health call rather
            // than guessed: `head_tensors` 36, `head_matched` 36, `head_missing` [].
            // An earlier version of this function checked a `tensors_matched` key that
            // does not exist — which would have silently passed everything, since a
            // missing key reads as `None`.
            let total = h.get("head_tensors").and_then(serde_json::Value::as_u64);
            let matched = h.get("head_matched").and_then(serde_json::Value::as_u64);
            match (total, matched) {
                (Some(0), _) | (_, Some(0)) => ProbeOutcome::HeadUnavailable(
                    "the sidecar is up but loaded 0 head tensors — its weights did not \
                     load, so every prediction would be garbage"
                        .to_string(),
                ),
                (Some(t), Some(m)) if m < t => ProbeOutcome::HeadUnavailable(format!(
                    "the sidecar loaded {m} of {t} head tensors — the head is incomplete, \
                     so its answers would be untrustworthy"
                )),
                (Some(_), Some(_)) => {
                    // The key must be PRESENT. A sidecar that omits `head_tensors`
                    // entirely is a different (older or broken) build, and treating a
                    // missing field as success is how a check passes while the thing it
                    // checks is absent.
                    ProbeOutcome::Healthy
                }
                _ => ProbeOutcome::HeadUnavailable(format!(
                    "the sidecar answered but did not report its head state \
                     (expected `head_tensors`/`head_matched`): {h}"
                )),
            }
        }
        Err(e) => ProbeOutcome::HeadUnavailable(e.to_string()),
    }
}

/// Parse the `/v1/models` body and check the expected model is served.
///
/// Pure, so the parsing is unit-testable without a server. The `/v1/models` shape is
/// llama.cpp's, observed live:
/// `{"models":[{"name":"/models/laya-Q8_0.gguf","model":"/models/laya-Q8_0.gguf", ...}]}`
///
/// We match on the model **name or model** field *ending with* [`EXPECTED_MODEL`],
/// because the served path is absolute (`/models/laya-Q8_0.gguf`) while the constant
/// is the bare filename.
#[must_use]
pub fn parse_models(body: &str) -> ProbeOutcome {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return ProbeOutcome::MalformedBody("response was not valid JSON".into());
    };
    let Some(models) = v.get("models").and_then(|m| m.as_array()) else {
        return ProbeOutcome::MalformedBody("no `models` array in the response".into());
    };
    if models.is_empty() {
        return ProbeOutcome::ModelMissing {
            wanted: EXPECTED_MODEL.into(),
        };
    }
    let found = models.iter().any(|m| {
        ["name", "model"].iter().any(|k| {
            m.get(k)
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.ends_with(EXPECTED_MODEL))
        })
    });
    if found {
        ProbeOutcome::Healthy
    } else {
        ProbeOutcome::ModelMissing {
            wanted: EXPECTED_MODEL.into(),
        }
    }
}

/// What a probe of the `/health` endpoint concluded from the raw parts.
///
/// Split from the network call so the status/body judgement is testable without a
/// socket — the awkward cases (a 200 with junk, a 404) are the ones that matter.
#[must_use]
pub fn from_health_response(status: u16, body: &str) -> ProbeOutcome {
    if status != 200 {
        return ProbeOutcome::BadStatus { status };
    }
    // A 200 alone is NOT proof of health: llama.cpp answers `{"status":"ok"}`.
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => match v.get("status").and_then(|s| s.as_str()) {
            Some("ok") => ProbeOutcome::Healthy,
            Some(other) => {
                ProbeOutcome::MalformedBody(format!("status field is {other:?}, expected \"ok\""))
            }
            None => ProbeOutcome::MalformedBody("no `status` field in the response".into()),
        },
        Err(e) => ProbeOutcome::MalformedBody(format!("response was not valid JSON: {e}")),
    }
}

/// Map a transport error to a probe outcome.
///
/// Three cases, and getting them apart matters because each needs a different
/// remedy and the diagnostic names one:
///
/// * **An HTTP status error is NOT a connection failure.** `ureq` reports a 4xx/5xx
///   as an `Err`, so a 404 on a *perfectly reachable* server would otherwise be
///   reported as "connection failed" and send the reader to restart a healthy
///   container. We recover the status and report it as [`ProbeOutcome::BadStatus`].
/// * **A timeout** is the server not answering in time — a different remedy again.
/// * **Anything else** is genuinely unreachable.
///
/// (Both misleading-diagnostic cases were found by running the command against a
/// live server with a wrong path, not by reading this function.)
#[must_use]
pub fn from_transport_error(err: &str) -> ProbeOutcome {
    let lower = err.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return ProbeOutcome::TimedOut {
            after_ms: PROBE_TIMEOUT_MS,
        };
    }
    // ureq phrases a status failure as "http status: 404". Recover the code so the
    // reader is told the server answered, just not with what we asked for.
    if let Some(code) = lower
        .split("http status:")
        .nth(1)
        .and_then(|rest| rest.trim().split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|d| d.parse::<u16>().ok())
    {
        return ProbeOutcome::BadStatus { status: code };
    }
    ProbeOutcome::Unreachable(err.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;

    // ---- parse_models: the REAL body we captured live ------------------------

    const LIVE_MODELS: &str = r#"{"models":[{"name":"/models/laya-Q8_0.gguf","model":"/models/laya-Q8_0.gguf","modified_at":"","size":"","digest":"","type":"model","description":"","tags":[""],"capabilities":["completion"],"parameters":"","details":{"parent_model":"","format":"gguf","family":"","families":[""],"parameter_size":""}}]}"#;

    #[test]
    fn the_live_models_body_is_healthy() {
        // The control for the whole module: the EXACT body the server returns must be
        // judged healthy. If this fails, the check is wrong, not the server.
        assert_eq!(parse_models(LIVE_MODELS), ProbeOutcome::Healthy);
    }

    #[test]
    fn the_capability_string_is_not_used_as_the_signal() {
        // The model advertises capabilities ["completion"], NOT "embedding". A check
        // that required "embedding" would report a working server as broken — this
        // test pins that we do not make that mistake.
        assert!(LIVE_MODELS.contains("\"capabilities\":[\"completion\"]"));
        assert!(!LIVE_MODELS.contains("\"embedding\""));
        assert_eq!(parse_models(LIVE_MODELS), ProbeOutcome::Healthy);
    }

    #[test]
    fn an_empty_model_list_is_not_ready_and_names_what_it_wanted() {
        let o = parse_models(r#"{"models":[]}"#);
        assert_eq!(
            o,
            ProbeOutcome::ModelMissing {
                wanted: EXPECTED_MODEL.into()
            }
        );
    }

    #[test]
    fn a_different_model_loaded_is_not_ready() {
        // The server is up and serving SOMETHING — but not our model. Reporting
        // healthy here would let decisions run against the wrong weights.
        let o = parse_models(r#"{"models":[{"name":"/models/other-model.gguf"}]}"#);
        assert_eq!(
            o,
            ProbeOutcome::ModelMissing {
                wanted: EXPECTED_MODEL.into()
            }
        );
    }

    #[test]
    fn a_missing_models_key_is_malformed_not_healthy() {
        assert!(matches!(
            parse_models(r#"{"data":[]}"#),
            ProbeOutcome::MalformedBody(_)
        ));
    }

    #[test]
    fn junk_from_the_models_endpoint_is_malformed() {
        assert!(matches!(
            parse_models("<html>nope</html>"),
            ProbeOutcome::MalformedBody(_)
        ));
    }

    // ---- from_health_response ------------------------------------------------

    #[test]
    fn the_live_health_body_is_healthy() {
        assert_eq!(
            from_health_response(200, r#"{"status":"ok"}"#),
            ProbeOutcome::Healthy
        );
    }

    #[test]
    fn a_200_with_junk_is_not_healthy() {
        // THE subtle case: a 200 is not proof of health. This is exactly how a server
        // reports ready and then fails every decision.
        assert!(matches!(
            from_health_response(200, "ok"),
            ProbeOutcome::MalformedBody(_)
        ));
    }

    #[test]
    fn a_200_without_a_status_field_is_not_healthy() {
        assert!(matches!(
            from_health_response(200, r#"{"ready":true}"#),
            ProbeOutcome::MalformedBody(_)
        ));
    }

    #[test]
    fn a_200_with_a_non_ok_status_is_not_healthy() {
        // The near-miss CONTROL for the line above: this one DOES parse and HAS the
        // field — it just says something else. It must still be not-ready, and the
        // message must quote what it actually said.
        let o = from_health_response(200, r#"{"status":"loading"}"#);
        assert!(matches!(o, ProbeOutcome::MalformedBody(_)));
        if let ProbeOutcome::MalformedBody(m) = o {
            assert!(m.contains("loading"), "must quote the real value: {m}");
        }
    }

    #[test]
    fn a_503_is_a_bad_status() {
        assert_eq!(
            from_health_response(503, ""),
            ProbeOutcome::BadStatus { status: 503 }
        );
    }

    #[test]
    fn a_404_is_also_a_bad_status_and_keeps_the_code() {
        // A 404 usually means the WRONG ENDPOINT. Saying "unhealthy" would send a
        // reader to restart a server that is fine, so the status must survive.
        assert_eq!(
            from_health_response(404, ""),
            ProbeOutcome::BadStatus { status: 404 }
        );
    }

    // ---- from_transport_error -----------------------------------------------

    #[test]
    fn a_timeout_error_is_reported_as_a_timeout() {
        assert!(matches!(
            from_transport_error("operation timed out after 2000ms"),
            ProbeOutcome::TimedOut { .. }
        ));
    }

    #[test]
    fn a_connection_error_is_unreachable_not_a_timeout() {
        // The control: these two need different remedies (restart vs network), so
        // they must not collapse into one outcome.
        assert!(matches!(
            from_transport_error("Connection refused (os error 111)"),
            ProbeOutcome::Unreachable(_)
        ));
    }

    #[test]
    fn a_404_on_a_live_server_is_a_bad_status_not_a_connection_failure() {
        // MEASURED, not invented: this is the exact string `ureq` produced when the
        // command was run against the LIVE server with a wrong path. Reporting it as
        // "connection failed" would send the reader to restart a healthy container —
        // a misleading diagnostic is worse than a vague one.
        let real = "io: http status: 404";
        assert_eq!(
            from_transport_error(real),
            ProbeOutcome::BadStatus { status: 404 }
        );
    }

    #[test]
    fn a_500_on_a_live_server_is_also_a_bad_status() {
        assert_eq!(
            from_transport_error("http status: 500"),
            ProbeOutcome::BadStatus { status: 500 }
        );
    }

    #[test]
    fn a_refused_connection_is_never_mistaken_for_a_status() {
        // The near-miss CONTROL: a message with no "http status:" must NOT be parsed
        // into a bogus code, or a real outage would look like a 404.
        match from_transport_error("Connection refused (os error 111)") {
            ProbeOutcome::Unreachable(m) => assert!(m.contains("111")),
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }

    // ---- the two together reach the right verdict ---------------------------

    #[test]
    fn a_bad_status_and_a_malformed_body_both_judge_not_ready() {
        for outcome in [
            from_health_response(503, ""),
            from_health_response(200, "nope"),
            parse_models(r#"{"models":[]}"#),
        ] {
            let r = judge(DEFAULT_ENDPOINT, &outcome);
            assert!(!r.is_ready(), "{outcome:?} must not judge ready");
        }
    }

    #[test]
    fn a_missing_socket_is_head_unavailable_and_names_the_reason() {
        // The common failure. It must NOT report a bare connection error, because the
        // reader would go hunting for a network problem: the real causes are "nobody
        // started it" and "the container restarted and /run is tmpfs".
        let out = probe_head(std::path::Path::new(
            "/tmp/laya-head-probe-does-not-exist-91bc.sock",
        ));
        let ProbeOutcome::HeadUnavailable(msg) = out else {
            panic!("a missing socket must be HeadUnavailable, got {out:?}");
        };
        assert!(msg.contains("never started"), "{msg}");
        assert!(
            msg.contains("tmpfs"),
            "must explain the container-restart case: {msg}"
        );
    }

    #[test]
    fn a_head_failure_reads_as_the_head_and_not_the_backbone() {
        // The distinction the whole fix exists for: judging a head failure must not
        // produce advice about the backbone container, which is healthy.
        let r = judge(
            DEFAULT_ENDPOINT,
            &probe_head(std::path::Path::new("/tmp/nope-91bc.sock")),
        );
        let Readiness::NotReady(f) = r else {
            panic!("a dead head must not be ready");
        };
        assert!(f.reason.contains("decision head"), "{}", f.reason);
        assert!(
            f.advice.contains("do NOT restart it"),
            "must not send the reader to restart a healthy backbone: {}",
            f.advice
        );
        assert!(f.advice.contains("laya-sidecar"), "{}", f.advice);
    }

    #[test]
    fn the_backbone_being_ready_does_not_imply_the_head_is() {
        // The regression that motivated this: these two probes are independent, and
        // the old `doctor` only ran the first. Pinned so they cannot be conflated again.
        let backbone_ok = judge(
            DEFAULT_ENDPOINT,
            &from_health_response(200, r#"{"status":"ok"}"#),
        );
        assert!(backbone_ok.is_ready());

        let head_dead = judge(
            DEFAULT_ENDPOINT,
            &probe_head(std::path::Path::new("/tmp/nope-91bc.sock")),
        );
        assert!(
            !head_dead.is_ready(),
            "a healthy backbone must NOT make the head look ready"
        );
    }

    #[test]
    fn healthy_bodies_judge_ready() {
        // And the positive path, so the negative tests above are not passing merely
        // because `judge` always says no.
        assert!(
            judge(
                DEFAULT_ENDPOINT,
                &from_health_response(200, r#"{"status":"ok"}"#)
            )
            .is_ready()
        );
        assert!(judge(DEFAULT_ENDPOINT, &parse_models(LIVE_MODELS)).is_ready());
    }
}
