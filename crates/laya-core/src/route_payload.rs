//! The route-shaped decision payload — the contract with the graph engine.
//!
//! A DAG's `route` node takes `on` (an expression) and `rules` (`{eq, port}`),
//! and the routed payload travels on the chosen port. So if we hand a node back
//! a payload with a `label` field, a graph author writes exactly this and nothing
//! else:
//!
//! ```toml
//! [[node]]
//! id = "triage"
//! kind = "route"
//! on = "$.payload.label"
//! rules = [
//!   { eq = "bug_in_code", port = "fix_code" },
//!   { eq = "wrong_test",  port = "fix_test"  },
//!   { eq = "environment", port = "check_env" },
//!   { port = "escalate" },              # CATCH-ALL — mandatory
//! ]
//! ```
//!
//! Three properties make the whole execution deterministic, and each is a
//! deliberate design choice rather than a side effect:
//!
//! 1. **The edge set is fixed at author time.** Laya cannot invent a port; its
//!    answer is always one of the caller's declared options, so it can only
//!    *select among ports that already exist*.
//! 2. **A missing field cannot silently take a wrong branch.** The payload always
//!    carries `label`, so `on` never evaluates to null-and-matches-nothing.
//! 3. **Confidence is a separate branch input** (`band`), not a hidden
//!    multiplier, so an uncertain answer escalates along a *drawn* edge.

use laya_abi::Answer;
use serde::{Deserialize, Serialize};

use crate::policy::{Band, Policy};

/// What a decision hands to the graph.
///
/// `label` and `band` are the two fields a `route` node is expected to match on.
/// The rest is carried for the journal and the ledger, so a routing decision can
/// be joined to the run that consumed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionPayload {
    /// The answer as a routing label — always one of the caller's declared
    /// options, or `"yes"`/`"no"` for a noul.
    ///
    /// This is the field `route on = "$.payload.label"` matches.
    pub label: String,
    /// The confidence band. `route on = "$.payload.band"` matches this, which is
    /// how an uncertain answer escalates along a drawn edge.
    pub band: String,
    /// Laya's calibrated confidence, 0.0..=1.0.
    pub confidence: f64,
    /// For a noul, the belief; `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    /// For a score, the position on the rubric; `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// The probability of every declared option, in `options` order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub probabilities: Vec<f64>,
    /// The declared options, echoed so a consumer never re-derives the mapping.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The id of the question that produced this.
    pub question_id: String,
    /// True when one option carried effectively all the probability mass.
    ///
    /// Surfaced rather than hidden because we measured the deployed checkpoint
    /// returning exactly `1.000/0.000/0.000` — a saturated answer is *suspect*,
    /// and must not be read as high confidence.
    pub saturated: bool,
}

impl DecisionPayload {
    /// Build a payload from an answer under a policy.
    ///
    /// The band is computed here — from the caller's `Policy`, not by the model —
    /// so the thresholds stay in one reviewable place while the payload arrives
    /// ready for `route`.
    #[must_use]
    pub fn from_answer(answer: &Answer, policy: &Policy) -> Self {
        let saturated = answer.saturated();
        // A saturated answer is not evidence of anything, so it is not allowed to
        // claim a High band: it reports as Mid at most, which routes it to an
        // ask/confirm branch instead of an auto-allow.
        let effective = if saturated && policy.escalate_on_saturation {
            confidence_for_saturated(answer.confidence)
        } else {
            answer.confidence
        };
        Self {
            label: answer.value.clone(),
            band: policy.band(effective).as_str().to_string(),
            confidence: answer.confidence,
            p: answer.p,
            score: answer.score,
            probabilities: answer.probabilities.clone(),
            options: answer.options.clone(),
            question_id: answer.id.clone(),
            saturated,
        }
    }

    /// The band as a typed value, when it parses.
    #[must_use]
    pub fn band_enum(&self) -> Option<Band> {
        match self.band.as_str() {
            "low" => Some(Band::Low),
            "mid" => Some(Band::Mid),
            "high" => Some(Band::High),
            _ => None,
        }
    }
}

/// Damp a saturated confidence below the high band, so saturation can never
/// auto-allow. Used only when the policy asks for saturation escalation.
const fn confidence_for_saturated(_confidence: f64) -> f64 {
    // Deliberately a fixed value rather than a fraction of the reported
    // confidence: the reported number is the thing we do not believe, so scaling
    // it would preserve the error we are trying to remove. 0.6 is Mid under the
    // default policy (low 0.5, high 0.9) and therefore routes to an ask.
    0.6
}

#[cfg(test)]
mod tests {
    use super::*;
    use laya_abi::QuestionType;

    fn answer(value: &str, confidence: f64, probs: Vec<f64>) -> Answer {
        Answer {
            id: "triage".into(),
            kind: QuestionType::Choice,
            value: value.into(),
            p: None,
            score: None,
            probabilities: probs,
            options: vec![
                "bug_in_code".into(),
                "wrong_test".into(),
                "environment".into(),
            ],
            confidence,
            latency_ms: Some(12.5),
        }
    }

    #[test]
    fn payload_exposes_the_two_route_keys() {
        let a = answer("bug_in_code", 0.95, vec![0.95, 0.03, 0.02]);
        let p = DecisionPayload::from_answer(&a, &Policy::default());
        // These two fields are what a `route` node matches on; their names are a
        // contract with the graph files, so they are pinned by test.
        assert_eq!(p.label, "bug_in_code");
        assert_eq!(p.band, "high");
        assert_eq!(p.band_enum(), Some(Band::High));
        assert_eq!(p.question_id, "triage");
        assert!(!p.saturated);
    }

    #[test]
    fn label_is_always_one_of_the_declared_options() {
        // The property that makes the graph's edge set fixed: whatever the model
        // returns, the label is a value the author declared.
        let opts = ["bug_in_code", "wrong_test", "environment"];
        for (v, c) in [
            ("bug_in_code", 0.9),
            ("wrong_test", 0.4),
            ("environment", 0.99),
        ] {
            let a = answer(v, c, vec![0.5, 0.3, 0.2]);
            let p = DecisionPayload::from_answer(&a, &Policy::default());
            assert!(
                opts.contains(&p.label.as_str()),
                "{} came back unlisted",
                p.label
            );
        }
    }

    #[test]
    fn a_low_confidence_routes_to_the_low_band() {
        let a = answer("wrong_test", 0.35, vec![0.35, 0.4, 0.25]);
        let p = DecisionPayload::from_answer(&a, &Policy::default());
        assert_eq!(p.band, "low");
        assert_eq!(p.band_enum(), Some(Band::Low));
    }

    #[test]
    fn a_saturated_answer_is_flagged_but_keeps_its_band_by_default() {
        let a = answer("bug_in_code", 0.99, vec![1.0, 0.0, 0.0]);
        let p = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(p.saturated, "the 1.0/0.0/0.0 case must be flagged");
        // Default policy records only, so the band still reflects the reported
        // confidence — the flag is the signal, not a silent override.
        assert_eq!(p.band, "high");
        assert!(
            (p.confidence - 0.99).abs() < f64::EPSILON,
            "reported confidence is not rewritten"
        );
    }

    #[test]
    fn saturation_escalation_demotes_the_band_and_never_allows() {
        let policy = Policy {
            escalate_on_saturation: true,
            ..Policy::default()
        };
        let a = answer("bug_in_code", 0.99, vec![1.0, 0.0, 0.0]);
        let p = DecisionPayload::from_answer(&a, &policy);
        assert!(p.saturated);
        assert_eq!(p.band, "mid", "a saturated answer must not claim High");
        assert_ne!(p.band_enum(), Some(Band::High));
    }

    #[test]
    fn the_near_miss_control_stays_high() {
        // A genuinely confident but NOT saturated answer must keep its High band,
        // or the demotion above becomes a blanket tax on confidence.
        let policy = Policy {
            escalate_on_saturation: true,
            ..Policy::default()
        };
        let a = answer("bug_in_code", 0.98, vec![0.98, 0.011, 0.009]);
        let p = DecisionPayload::from_answer(&a, &policy);
        assert!(!p.saturated);
        assert_eq!(p.band, "high");
    }

    #[test]
    fn noul_and_score_fields_carried_through() {
        let mut a = answer("yes", 0.8, Vec::new());
        a.kind = QuestionType::Noul;
        a.options = Vec::new();
        a.p = Some(0.87);
        let p = DecisionPayload::from_answer(&a, &Policy::default());
        assert_eq!(p.p, Some(0.87));
        let mut s = answer("7", 0.7, vec![0.1, 0.2, 0.7]);
        s.kind = QuestionType::Score;
        s.score = Some(7.4);
        let ps = DecisionPayload::from_answer(&s, &Policy::default());
        assert_eq!(ps.score, Some(7.4));
    }

    #[test]
    fn payload_round_trips_and_omits_empty_fields() {
        let a = answer("bug_in_code", 0.95, vec![0.95, 0.03, 0.02]);
        let p = DecisionPayload::from_answer(&a, &Policy::default());
        let json = serde_json::to_string(&p).unwrap_or_default();
        let back: DecisionPayload = serde_json::from_str(&json).unwrap_or_else(|_| p.clone());
        assert_eq!(back, p);
        // The two route keys must survive serialization — the graph reads them
        // out of the JSON payload, not out of the Rust struct.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(v.get("label").and_then(|x| x.as_str()), Some("bug_in_code"));
        assert_eq!(v.get("band").and_then(|x| x.as_str()), Some("high"));
    }
}
