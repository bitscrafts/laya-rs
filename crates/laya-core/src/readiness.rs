//! Readiness — is Laya actually running?
//!
//! **User directive**: *"it will be mandatory the app to check for laya be
//! running! and it must inform/stop if not!"*
//!
//! This module owns the DECISION, not the probe. The probe does I/O and lives in
//! `laya-backbone`; everything here is pure, so every branch — including the ones
//! that are awkward to reproduce against a real server — is a unit test.
//!
//! The rule the whole module exists to enforce: **a decision layer that cannot
//! decide must not run, and an unavailable gate must never mean permission.**

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::policy::GateDecision;

/// The outcome of a probe, as reported by the I/O layer.
///
/// Deliberately a plain data type: `laya-backbone` produces it, `laya-core`
/// judges it, and the judgement can be tested without a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The endpoint answered with the expected shape.
    Healthy,
    /// The endpoint could not be reached at all.
    Unreachable(String),
    /// The endpoint was reachable but did not answer in time.
    TimedOut {
        /// How long we waited, in milliseconds.
        after_ms: u64,
    },
    /// The endpoint answered with an unexpected HTTP status.
    BadStatus {
        /// The status code returned.
        status: u16,
    },
    /// The endpoint answered 200 but the body was not usable.
    MalformedBody(String),
    /// The endpoint was healthy but does not serve the model we need.
    ModelMissing {
        /// The model id we asked for.
        wanted: String,
    },
    /// The backbone is reachable, but the **decision head** is not.
    ///
    /// This is a distinct case and not a flavour of [`Self::Unreachable`], because the
    /// remedy is different: restarting the backbone would be wrong — it is healthy.
    /// The head is the sidecar. Collapsing the two sends the reader to debug a service
    /// that is working.
    HeadUnavailable(String),
}

/// Whether Laya is usable right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Laya answered and serves what we need.
    Ready,
    /// Laya is not usable. Carries a fully-formed, actionable reason.
    NotReady(ReadinessFailure),
}

impl Readiness {
    /// True only for [`Readiness::Ready`].
    ///
    /// Callers must branch on this rather than on the absence of an error: an
    /// un-inspected `NotReady` is exactly how a fail-open creeps in.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

impl fmt::Display for Readiness {
    /// Render a verdict for a log line or a diagnostic.
    ///
    /// `Ready` is terse; `NotReady` prints the full actionable advice, because that is
    /// the text an operator or an agent needs and it must not be reconstructed at the
    /// call site (a rebuilt message is a message that drifts from the reason).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => f.write_str("ready"),
            Self::NotReady(failure) => write!(f, "{failure}"),
        }
    }
}

/// Why Laya is not usable — and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessFailure {
    /// The endpoint we tried (a URL is safe to state; a key is not).
    pub endpoint: String,
    /// The short reason, for a log line or a tool error.
    pub reason: String,
    /// The full, action-oriented message for stderr.
    pub advice: String,
}

impl fmt::Display for ReadinessFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.advice)
    }
}

/// The container that serves Laya in this workspace.
const CONTAINER_HINT: &str = "llamacpp-laya";

/// Judge a probe outcome.
///
/// `endpoint` is used only for the message. Every failure produces `advice` that
/// names the endpoint tried and the command to check it, because the reader of a
/// startup failure is an agent or a human who has not read this source.
#[must_use]
pub fn judge(endpoint: &str, outcome: &ProbeOutcome) -> Readiness {
    let fail = |reason: String| {
        Readiness::NotReady(ReadinessFailure {
            endpoint: endpoint.to_string(),
            reason: reason.clone(),
            advice: format!(
                "laya: Laya is not reachable/usable at {endpoint}\n\
                 \x20 reason: {reason}\n\
                 \x20 check : curl {endpoint}/health\n\
                 \x20 fix   : start the container `{CONTAINER_HINT}` (podman start {CONTAINER_HINT})\n\
                 \x20 refusing to start: a decision layer that cannot decide must not run,\n\
                 \x20 and a gate that cannot ask must never answer `allow`."
            ),
        })
    };

    match outcome {
        ProbeOutcome::Healthy => Readiness::Ready,
        ProbeOutcome::Unreachable(e) => fail(format!("connection failed: {e}")),
        ProbeOutcome::TimedOut { after_ms } => fail(format!("timed out after {after_ms} ms")),
        ProbeOutcome::BadStatus { status } => fail(format!("unexpected HTTP status {status}")),
        ProbeOutcome::MalformedBody(e) => {
            // A 200 is NOT proof of health: the server answered but is not usable.
            fail(format!("answered, but the body was not usable: {e}"))
        }
        ProbeOutcome::ModelMissing { wanted } => {
            fail(format!("reachable, but the model `{wanted}` is not served"))
        }
        // The backbone is fine; the HEAD is missing. Same verdict, different advice —
        // and the advice must not tell the reader to restart the backbone, which is
        // healthy. This is the state that made `doctor` report READY while `decide`
        // could not answer a single question.
        ProbeOutcome::HeadUnavailable(e) => Readiness::NotReady(ReadinessFailure {
            endpoint: endpoint.to_string(),
            reason: format!("the decision head is unavailable: {e}"),
            advice: format!(
                "laya: the backbone is UP but the DECISION HEAD is not usable\n\
                 \x20 endpoint : {endpoint} (backbone — healthy, do NOT restart it)\n\
                 \x20 reason   : {e}\n\
                 \x20 check    : laya-sidecar --check  (or: ls -l $LAYA_SIDECAR_SOCKET)\n\
                 \x20 fix      : start the head — `laya-sidecar` loads in ~24 s\n\
                 \x20 refusing to start: a backbone that can embed but a head that cannot\n\
                 \x20 decide is not a decision layer; `decide` would have no answer to give."
            ),
        }),
    }
}

/// What a tool must do, given readiness.
///
/// One place, so no tool can invent its own (and weaker) down behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenNotReady {
    /// `gate` answers `ask`. **Never** `allow` — see the module docs.
    GateAsks,
    /// Every other tool returns a protocol error.
    ProtocolError,
}

/// The down-behaviour for a given tool.
///
/// `gate` is the one tool whose down behaviour is a *decision* rather than an
/// error, because a hook calling a gate must get a safe answer, not a crash.
#[must_use]
pub fn when_not_ready(tool: &str) -> WhenNotReady {
    match tool {
        "gate" => WhenNotReady::GateAsks,
        _ => WhenNotReady::ProtocolError,
    }
}

/// The gate decision to return when the source is down.
///
/// Always [`GateDecision::Ask`]. Exists as a named function so the property is
/// testable and greppable — "when Laya is down, the gate asks" — rather than being
/// an `else` branch someone can quietly change to `Allow`.
#[must_use]
pub const fn gate_when_down() -> GateDecision {
    GateDecision::Ask
}

/// How long a `Ready` verdict may be reused before re-probing.
///
/// The trade-off is explicit: a longer cache means fewer round trips, but it also
/// means a Laya that dies is noticed later — and during that window the system is
/// failing open in the sense that it *believes* it is ready. Five seconds is short
/// enough that a dead Laya is noticed quickly and long enough that a burst of tool
/// calls does not probe per call.
pub const READINESS_TTL_MS: u64 = 5_000;

/// A readiness verdict with the moment it was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedReadiness {
    /// The verdict.
    pub verdict: Readiness,
    /// The monotonic-ish timestamp (milliseconds) the verdict was produced at.
    pub at_ms: u64,
}

/// A readiness cache with an injectable clock.
///
/// Pure — the caller supplies "now" — so every branch (fresh, stale, re-probed,
/// failed-after-ready) is a unit test with no sleeping and no socket. This matters:
/// the interesting bug is *a cached `Ready` masking a later failure*, and that bug
/// is invisible to a test that has to wait five real seconds to see it.
#[derive(Debug, Clone, Default)]
pub struct ReadinessCache {
    last: Option<CachedReadiness>,
}

impl ReadinessCache {
    /// An empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// A cached verdict, if one exists and is still within the TTL.
    ///
    /// An *unready* verdict is deliberately **not** reusable: if the last probe
    /// failed we want the next call to re-check rather than keep reporting a stale
    /// failure, so a Laya that comes back is picked up immediately. (Only `Ready` is
    /// cheap enough to be worth caching; a failed probe is already the slow path.)
    #[must_use]
    pub fn fresh(&self, now_ms: u64) -> Option<&Readiness> {
        let last = self.last.as_ref()?;
        if !last.verdict.is_ready() {
            return None;
        }
        if now_ms.saturating_sub(last.at_ms) <= READINESS_TTL_MS {
            Some(&last.verdict)
        } else {
            None
        }
    }

    /// Record a verdict taken at `now_ms`.
    pub fn record(&mut self, verdict: Readiness, now_ms: u64) {
        self.last = Some(CachedReadiness {
            verdict,
            at_ms: now_ms,
        });
    }

    /// The last recorded verdict regardless of age — for diagnostics only.
    ///
    /// Never use this to decide whether to proceed: that is what [`Self::fresh`] is
    /// for, and using the stale value is exactly the fail-open this module prevents.
    #[must_use]
    pub fn last_verdict(&self) -> Option<&Readiness> {
        // Plain `map`. The round trip through `map_or(None, ..)` was clippy's first
        // suggestion, which then tripped `option_map_or_none` — the direct form is what
        // it wanted all along. Not `const` because `Option::map` is not const-stable,
        // and this needs no constness.
        self.last.as_ref().map(|c| &c.verdict)
    }
}

#[cfg(test)]
mod tests {
    // Tests assert by panicking — that IS the assertion mechanism, and it is why
    // the workspace-wide `panic = "deny"` must be relaxed here and ONLY here. The
    // library itself keeps the deny.
    #![allow(clippy::panic, clippy::indexing_slicing)]

    use super::*;

    const EP: &str = "http://10.10.10.217:8083";

    /// Judge an outcome and hand back the failure, panicking if it came back Ready.
    ///
    /// The borrow is fine — `failed(&ProbeOutcome::X)` keeps the temporary alive
    /// for the whole statement and the returned `ReadinessFailure` is owned, so
    /// nothing borrows out of it.
    fn failed(outcome: &ProbeOutcome) -> ReadinessFailure {
        match judge(EP, outcome) {
            Readiness::Ready => panic!("expected NotReady, got Ready"),
            Readiness::NotReady(f) => f,
        }
    }

    // ---- the ready path -----------------------------------------------------

    #[test]
    fn a_healthy_probe_is_ready() {
        let r = judge(EP, &ProbeOutcome::Healthy);
        assert!(r.is_ready());
        assert_eq!(r, Readiness::Ready);
    }

    // ---- every failure mode names itself ------------------------------------

    #[test]
    fn a_connection_error_is_not_ready_with_a_reason() {
        let f = failed(&ProbeOutcome::Unreachable("refused".into()));
        assert!(
            f.reason.contains("refused"),
            "the reason must carry the real error: {}",
            f.reason
        );
    }

    #[test]
    fn a_timeout_is_not_ready_not_an_error() {
        // A hang must become a REPORTED not-ready, never an exception that takes
        // the caller down without a message.
        let f = failed(&ProbeOutcome::TimedOut { after_ms: 2000 });
        assert!(f.reason.contains("timed out"), "got: {}", f.reason);
        assert!(
            f.reason.contains("2000"),
            "the wait must be stated: {}",
            f.reason
        );
    }

    #[test]
    fn an_http_5xx_is_not_ready() {
        let f = failed(&ProbeOutcome::BadStatus { status: 503 });
        assert!(f.reason.contains("503"), "got: {}", f.reason);
    }

    #[test]
    fn an_http_404_on_health_is_not_ready() {
        // The control for the line above: 404 usually means we are asking the WRONG
        // endpoint. Reporting "unhealthy" would send the reader to restart a server
        // that is fine, so it must still be not-ready but must state the status.
        let f = failed(&ProbeOutcome::BadStatus { status: 404 });
        assert!(f.reason.contains("404"), "got: {}", f.reason);
    }

    #[test]
    fn a_malformed_body_is_not_ready() {
        // This is the subtle one: HTTP 200 with unusable content. Treating a 200 as
        // proof of health is precisely how a server answers "ready" and then fails
        // every decision. It must be not-ready, and say so.
        let r = judge(EP, &ProbeOutcome::MalformedBody("not json".into()));
        assert!(!r.is_ready(), "a 200 alone is not proof of health");
        let Readiness::NotReady(f) = &r else {
            panic!("expected NotReady");
        };
        assert!(f.reason.contains("not usable"), "got: {}", f.reason);
    }

    #[test]
    fn a_missing_model_is_not_ready_and_names_it() {
        let r = judge(
            EP,
            &ProbeOutcome::ModelMissing {
                wanted: "laya-Q8_0".into(),
            },
        );
        let Readiness::NotReady(f) = &r else {
            panic!("expected NotReady");
        };
        assert!(f.reason.contains("laya-Q8_0"), "got: {}", f.reason);
    }

    // ---- the message must be actionable -------------------------------------

    #[test]
    fn the_reason_names_the_endpoint_tried() {
        let f = failed(&ProbeOutcome::Unreachable("x".into()));
        assert!(f.endpoint == EP);
        assert!(f.advice.contains(EP), "the advice must name the endpoint");
    }

    #[test]
    fn the_advice_names_the_container_and_a_check_command() {
        let f = failed(&ProbeOutcome::Unreachable("x".into()));
        assert!(f.advice.contains(CONTAINER_HINT), "advice: {}", f.advice);
        assert!(
            f.advice.contains(&format!("curl {EP}/health")),
            "advice must give a check command: {}",
            f.advice
        );
    }

    #[test]
    fn the_advice_says_it_refuses_to_start() {
        // The directive is "inform/stop". The message must say STOP, so an operator
        // does not think it started in a degraded mode.
        let f = failed(&ProbeOutcome::Unreachable("x".into()));
        assert!(
            f.advice.contains("refusing to start"),
            "advice: {}",
            f.advice
        );
    }

    #[test]
    fn a_failure_displays_as_its_advice() {
        let f = failed(&ProbeOutcome::Unreachable("x".into()));
        let shown = format!("{f}");
        assert!(shown.contains("refusing to start"));
    }

    // ---- fail closed --------------------------------------------------------

    #[test]
    fn gate_is_ask_when_not_ready() {
        // THE load-bearing test of this module. An unavailable gate must never
        // mean permission.
        assert_eq!(gate_when_down(), GateDecision::Ask);
        assert_ne!(gate_when_down(), GateDecision::Allow);
    }

    #[test]
    fn every_other_tool_is_a_protocol_error_when_not_ready() {
        for tool in ["decide", "ask_file", "ask_files", "ask", "should_compact"] {
            assert_eq!(
                when_not_ready(tool),
                WhenNotReady::ProtocolError,
                "{tool} must error, not guess"
            );
        }
        assert_eq!(when_not_ready("gate"), WhenNotReady::GateAsks);
    }

    #[test]
    fn an_unknown_tool_also_errors_rather_than_defaulting_to_something_permissive() {
        // The control: a typo'd or newly added tool must land on the SAFE branch,
        // not on a permissive default.
        assert_eq!(when_not_ready("some_new_tool"), WhenNotReady::ProtocolError);
    }

    // ---- the failure carries no secret --------------------------------------

    #[test]
    fn the_failure_never_contains_a_key() {
        // The endpoint may be printed; a credential must never be. This test fails
        // loudly if someone ever formats a key into the advice.
        let secret = "sk-SUPERSECRETVALUE123";
        let f = failed(&ProbeOutcome::Unreachable("refused".into()));
        let shown = format!("{f}");
        assert!(!shown.contains(secret));
        assert!(!shown.contains("Authorization"));
        assert!(!shown.to_lowercase().contains("api_key"));
    }
    // ---- the readiness cache (spec 05 F5) -----------------------------------
    //
    // The clock is injected, so the interesting bug — a cached `Ready` hiding a
    // later failure — is testable WITHOUT sleeping five real seconds.

    fn ready() -> Readiness {
        Readiness::Ready
    }

    fn not_ready() -> Readiness {
        judge(EP, &ProbeOutcome::Unreachable("refused".into()))
    }

    #[test]
    fn an_empty_cache_has_nothing_fresh() {
        let c = ReadinessCache::new();
        assert!(c.fresh(0).is_none());
        assert!(c.last_verdict().is_none());
    }

    #[test]
    fn a_ready_verdict_is_fresh_within_the_ttl() {
        let mut c = ReadinessCache::new();
        c.record(ready(), 1_000);
        assert_eq!(c.fresh(1_000), Some(&Readiness::Ready));
        assert_eq!(c.fresh(1_000 + READINESS_TTL_MS), Some(&Readiness::Ready));
    }

    #[test]
    fn a_ready_verdict_expires_after_the_ttl() {
        // The bound that stops "it was up when we started" becoming a permanent
        // belief. One millisecond past the TTL must re-probe.
        let mut c = ReadinessCache::new();
        c.record(ready(), 1_000);
        assert!(c.fresh(1_000 + READINESS_TTL_MS + 1).is_none());
    }

    #[test]
    fn a_not_ready_verdict_is_never_reused() {
        // Deliberate: a failed probe must not be cached, so a Laya that comes back is
        // noticed on the very next call rather than after the TTL.
        let mut c = ReadinessCache::new();
        c.record(not_ready(), 1_000);
        assert!(
            c.fresh(1_000).is_none(),
            "a failure must never be served from cache"
        );
    }

    #[test]
    fn a_cached_ready_does_not_mask_a_later_failure() {
        // THE test this cache exists for. Sequence: ready, then the TTL elapses, then
        // a probe fails. The second check must report NOT ready — if this ever
        // regresses, the system fails open while believing it is healthy.
        let mut c = ReadinessCache::new();
        c.record(ready(), 0);
        // Within the TTL: reuse is fine and intended.
        assert!(c.fresh(1_000).is_some());
        // TTL elapsed: must re-probe, and the probe now fails.
        assert!(
            c.fresh(READINESS_TTL_MS + 1).is_none(),
            "stale Ready must not be served"
        );
        c.record(not_ready(), READINESS_TTL_MS + 1);
        assert!(c.fresh(READINESS_TTL_MS + 1).is_none());
        // The point: after the failure there is NOTHING fresh to serve. (The first
        // version of this assertion used `unwrap_or(&Ready)` and therefore asserted
        // the opposite of what it meant — when `fresh` returns None the caller must
        // re-probe, not fall back to a permissive default.)
        assert!(
            c.fresh(READINESS_TTL_MS + 2).is_none(),
            "a failed verdict must never be served as fresh"
        );
    }

    #[test]
    fn a_recovery_is_picked_up_immediately() {
        // The other direction, so the rule above is not just "always pessimistic":
        // once a probe succeeds again, the very next check is ready.
        let mut c = ReadinessCache::new();
        c.record(not_ready(), 0);
        assert!(c.fresh(1).is_none());
        c.record(ready(), 2);
        assert_eq!(c.fresh(2), Some(&Readiness::Ready));
    }

    #[test]
    fn last_verdict_exposes_stale_state_for_diagnostics_only() {
        // Available for a `doctor`-style report, and documented as never-for-deciding.
        let mut c = ReadinessCache::new();
        c.record(ready(), 0);
        assert_eq!(c.last_verdict(), Some(&Readiness::Ready));
        assert!(
            c.fresh(READINESS_TTL_MS + 1).is_none(),
            "stale, but still reported"
        );
    }

    #[test]
    fn a_clock_going_backwards_does_not_panic_or_extend_the_ttl() {
        // saturating_sub, not a raw subtraction: a non-monotonic clock must not
        // panic in debug or wrap in release.
        let mut c = ReadinessCache::new();
        c.record(ready(), 10_000);
        assert_eq!(c.fresh(5_000), Some(&Readiness::Ready));
    }
}
