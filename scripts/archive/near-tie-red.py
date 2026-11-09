#!/usr/bin/env python3
"""RED: the near-tie margin (spec 04 F3) and the inert noul threshold.

TWO DEFECTS, both the same shape as the ones the review already fixed — a mechanism that
reads as present while contributing nothing:

1. **`margin` is absent.** Spec 04 F3 requires `decide` to return "the route-shaped payload
   of spec 02, **including `margin`**". It does not. This matters because the two backends
   were measured to differ by **Δp = 0.0006** — enough to flip a 0.5001-vs-0.4999 decision.
   Without a margin a near-tie is decided SILENTLY, and a silent flip inside a graph is
   exactly the non-determinism the whole design claims to have removed.

2. **The noul threshold is hardcoded to `0.5` in the handler**, while
   `Policy::noul_threshold` (validated, configurable from TOML) and `Policy::noul_is_true()`
   exist and are called ONLY from their own tests. So changing `noul_threshold` in a config
   file changes nothing about the label a caller receives.

The tests are written against an API that does not exist yet, so RED is a compile failure —
which is the honest failure for "this capability is absent".
"""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-core/src/route_payload.rs")
t = p.read_text()

anchor = "#[cfg(test)]\nmod tests {"
assert anchor in t, "test module anchor not found"

new_tests = '''#[cfg(test)]
mod near_tie_tests {
    #![allow(clippy::panic)]

    use super::*;
    use laya_abi::QuestionType;

    fn choice(id: &str, value: &str, conf: f64, probs: Vec<f64>) -> Answer {
        let n = probs.len();
        Answer {
            id: id.into(),
            kind: QuestionType::Choice,
            value: value.into(),
            p: None,
            score: None,
            probabilities: probs,
            options: (0..n).map(|i| format!("opt{i}")).collect(),
            confidence: conf,
            latency_ms: None,
        }
    }

    fn noul(id: &str, p: f64) -> Answer {
        Answer {
            id: id.into(),
            kind: QuestionType::Noul,
            value: if p >= 0.5 { "yes".into() } else { "no".into() },
            p: Some(p),
            score: None,
            probabilities: Vec::new(),
            options: Vec::new(),
            confidence: p.max(1.0 - p),
            latency_ms: None,
        }
    }

    // ---- margin is reported ------------------------------------------------

    #[test]
    fn a_choice_reports_the_margin_between_the_top_two() {
        // 0.93 - 0.05 = 0.88. The number that says "this was not close".
        let a = choice("t", "opt0", 0.93, vec![0.93, 0.05, 0.02]);
        let pl = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(
            (pl.margin - 0.88).abs() < 1e-9,
            "margin must be top1 - top2: got {}",
            pl.margin
        );
        assert!(!pl.near_tie, "0.88 is not a near tie");
    }

    #[test]
    fn a_choice_margin_is_computed_regardless_of_the_declared_order() {
        // The probabilities arrive parallel to `options`, NOT necessarily sorted. If the
        // implementation used `v[0] - v[1]` instead of the top two, this would come out
        // negative and a near-tie would be missed.
        let a = choice("t", "opt2", 0.4, vec![0.3, 0.3, 0.4]);
        let pl = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(
            (pl.margin - 0.1).abs() < 1e-9,
            "margin must use the TOP TWO, not the first two: got {}",
            pl.margin
        );
    }

    #[test]
    fn a_two_option_near_tie_is_flagged() {
        // THE case the measured delta-0.0006 can flip.
        let a = choice("t", "opt0", 0.5001, vec![0.5001, 0.4999]);
        let pl = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(pl.near_tie, "a 0.0002 margin is a near tie");
        assert!(
            (pl.margin - 0.0002).abs() < 1e-6,
            "got margin {}",
            pl.margin
        );
    }

    #[test]
    fn a_single_option_has_no_margin_to_be_near() {
        // One option: there is no second place. The margin is the whole mass, and a
        // single-option choice must not be reported as a near tie just because the
        // "second highest" is absent.
        let a = choice("t", "only", 0.99, vec![0.99]);
        let pl = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(!pl.near_tie, "a one-option choice is not a near tie");
        assert!((pl.margin - 0.99).abs() < 1e-9, "got {}", pl.margin);
    }

    #[test]
    fn a_noul_margin_is_the_distance_from_the_decision_threshold() {
        // A noul has no second option, so "top two" is meaningless. Its decision boundary
        // is the threshold, so the margin is the distance from it — which is the quantity
        // that actually answers "how close was this to flipping?".
        let policy = Policy::default(); // noul_threshold 0.5
        let pl = DecisionPayload::from_answer(&noul("n", 0.52), &policy);
        assert!(
            (pl.margin - 0.02).abs() < 1e-9,
            "margin must be |p - threshold|: got {}",
            pl.margin
        );
        assert!(pl.near_tie, "0.02 from the boundary is a near tie");
    }

    #[test]
    fn a_confident_noul_is_not_a_near_tie() {
        let pl = DecisionPayload::from_answer(&noul("n", 0.95), &Policy::default());
        assert!((pl.margin - 0.45).abs() < 1e-9, "got {}", pl.margin);
        assert!(!pl.near_tie);
    }

    // ---- the consequence: a near tie must not auto-allow -------------------

    #[test]
    fn a_near_tie_cannot_claim_a_high_band() {
        // THE POINT OF THE WHOLE FEATURE. A near-tie inside a graph must route to an
        // ask/escalate edge, not be decided silently. Demoting the band is how that
        // happens, because `route on = "$.payload.band"` is what picks the edge.
        let mut policy = Policy::default();
        policy.escalate_on_near_tie = true;
        let a = choice("t", "opt0", 0.5001, vec![0.5001, 0.4999]);
        let pl = DecisionPayload::from_answer(&a, &policy);
        assert!(
            pl.near_tie,
            "precondition: this must be a near tie for the test to mean anything"
        );
        assert_ne!(
            pl.band, "high",
            "a near tie must not report High — it would auto-allow"
        );
    }

    #[test]
    fn a_near_tie_that_is_saturated_still_reports_the_tie() {
        // Near-tie and saturated are independent facts. Reporting one must not erase the
        // other, or a caller fixing one would not know the other existed.
        let a = choice("t", "opt0", 0.5, vec![0.5, 0.5]);
        let pl = DecisionPayload::from_answer(&a, &Policy::default());
        assert!(
            (pl.margin - 0.0).abs() < 1e-9,
            "a perfect tie has a zero margin: got {}",
            pl.margin
        );
        assert!(pl.near_tie);
    }

    #[test]
    fn the_near_tie_control_a_wide_margin_still_reaches_high_band() {
        // The near-miss CONTROL. Without it, an implementation that demoted EVERY band
        // would pass every test above while making the layer useless.
        let mut policy = Policy::default();
        policy.escalate_on_near_tie = true;
        let a = choice("t", "opt0", 0.97, vec![0.97, 0.02, 0.01]);
        let pl = DecisionPayload::from_answer(&a, &policy);
        assert!(!pl.near_tie, "precondition: not a tie");
        assert_eq!(pl.band, "high", "a clear answer must still be High");
    }
}

#[cfg(test)]
'''

t = t.replace(anchor, new_tests + "mod tests {", 1)
p.write_text(t)
print("  RED tests added for margin + near_tie + band demotion")
print("  NOTE: references Policy::escalate_on_near_tie, which does not exist yet")
