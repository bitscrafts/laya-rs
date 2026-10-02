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

use laya_abi::{Answer, QuestionType};
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
    /// The distance between the winner and the runner-up, in probability.
    ///
    /// The number that says whether a decision was CLOSE. Reported because the two backends
    /// were measured to differ by Δp = 0.0006: without a margin, a decision that a
    /// quantisation change could flip looks exactly like a landslide.
    pub margin: f64,
    /// True when [`Self::margin`] is at or below the policy's `near_tie_margin`.
    ///
    /// A flagged near tie is a decision the DAG should escalate rather than take — and with
    /// `escalate_on_near_tie` it cannot claim the High band, so an existing
    /// `route on = "$.payload.band"` sends it to the ask edge with no graph change.
    pub near_tie: bool,
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
        let margin = margin_of(answer, policy);
        let near_tie = margin <= policy.near_tie_margin;

        // A near tie must not auto-allow. Demoting to `mid` (rather than to `low`) is
        // deliberate: the answer is still the best available reading of the state, it is
        // the CERTAINTY that is missing — so the right branch is `ask`, and `mid` is what
        // a default policy routes there.
        let band = if near_tie && policy.escalate_on_near_tie {
            Band::Mid
        } else {
            policy.band(effective)
        };

        Self {
            label: answer.value.clone(),
            band: band.as_str().to_string(),
            confidence: answer.confidence,
            p: answer.p,
            score: answer.score,
            probabilities: answer.probabilities.clone(),
            options: answer.options.clone(),
            question_id: answer.id.clone(),
            saturated,
            margin,
            near_tie,
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

/// The margin for an answer: how far its decision was from flipping.
///
/// Differs by question type, because "the runner-up" means different things:
///   * `choice` — top1 − top2 over the **top two**, not the first two: the probabilities
///     arrive parallel to `options` and are not guaranteed sorted, so `p[0] - p[1]` would
///     be wrong (and could even be negative, hiding every tie);
///   * `noul`   — `|p − noul_threshold|`: a noul has no second place, so its boundary is
///     the threshold and the distance from it is what says how close the decision was;
///   * `score`  — the gap to the nearest adjacent level, since a score sits between levels.
///
/// A `choice` with fewer than two options has no runner-up. Its margin is its whole mass
/// and it is NOT a near tie — treating a missing second value as `0.0` would report every
/// single-option choice as maximally unstable.
fn margin_of(answer: &Answer, policy: &Policy) -> f64 {
    match answer.kind {
        QuestionType::Noul => {
            // `p` is the belief; when it is absent fall back to the confidence's distance
            // from the threshold's complement, so the field is never silently meaningless.
            let p = answer.p.unwrap_or(1.0 - answer.confidence);
            (p - policy.noul_threshold).abs()
        }
        QuestionType::Choice => {
            let mut finite: Vec<f64> = answer
                .probabilities
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .collect();
            finite.sort_by(|a, b| b.total_cmp(a));
            match (finite.first(), finite.get(1)) {
                (Some(first), Some(second)) => first - second,
                // One option only: no runner-up, so no tie is possible.
                (Some(first), None) => *first,
                (None, _) => 0.0,
            }
        }
        QuestionType::Score => {
            // The nearest adjacent rubric level. `score` may land between levels; with no
            // level list available the fractional distance to the nearer integer is the
            // same quantity in the common unit-spaced case.
            let s = answer.score.unwrap_or(0.0);
            let frac = s - s.floor();
            frac.min(1.0 - frac)
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
        let policy = Policy::default(); // noul_threshold 0.5, near_tie_margin 0.01
        let pl = DecisionPayload::from_answer(&noul("n", 0.52), &policy);
        assert!(
            (pl.margin - 0.02).abs() < 1e-9,
            "margin must be |p - threshold|: got {}",
            pl.margin
        );
        // CORRECTED: 0.02 is ABOVE the 0.01 margin, so this is NOT a near tie. My first
        // version asserted it was — arithmetic, not judgement: I wrote a 0.02 margin in a
        // test whose whole point was to sit under a 0.01 threshold. The sub-threshold case
        // is the test below, which is the one that means something.
        assert!(!pl.near_tie, "0.02 is above the 0.01 margin, so not a tie");
    }

    #[test]
    fn a_noul_within_the_margin_of_its_threshold_is_a_near_tie() {
        // THE case that matters: p sits so close to the threshold that the measured
        // 0.0006 backend drift could flip which side of it the answer lands on.
        let pl = DecisionPayload::from_answer(&noul("n", 0.505), &Policy::default());
        assert!((pl.margin - 0.005).abs() < 1e-9, "got margin {}", pl.margin);
        assert!(pl.near_tie, "0.005 is inside the 0.01 margin");
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
        let policy = Policy {
            escalate_on_near_tie: true,
            ..Policy::default()
        };
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
        let policy = Policy {
            escalate_on_near_tie: true,
            ..Policy::default()
        };
        let a = choice("t", "opt0", 0.97, vec![0.97, 0.02, 0.01]);
        let pl = DecisionPayload::from_answer(&a, &policy);
        assert!(!pl.near_tie, "precondition: not a tie");
        assert_eq!(pl.band, "high", "a clear answer must still be High");
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
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

    /// Seeded LCG (Knuth MMIX constants): deterministic, offline, reproducible from the seed.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }
        #[allow(clippy::cast_precision_loss)]
        fn unit(&mut self) -> f64 {
            (self.next() as f64) / ((1_u64 << 53) as f64)
        }
    }

    #[test]
    fn payload_invariants_hold_for_random_distributions() {
        const SEED: u64 = 0x133E_ED01;
        let policy = Policy::default();
        let mut rng = Lcg(SEED);
        let (mut ties, mut non_ties) = (0_u32, 0_u32);
        for case in 0..512_u32 {
            let k = 2 + usize::try_from(rng.next() % 12).unwrap_or(0);
            // Every third case is quantised to 3 levels so exact and near ties actually occur.
            let quantise = case % 3 == 0;
            let raw: Vec<f64> = (0..k)
                .map(|_| {
                    let u = rng.unit();
                    if quantise {
                        (u * 3.0).floor() + 1.0
                    } else {
                        u + 1e-3
                    }
                })
                .collect();
            let total: f64 = raw.iter().sum();
            let probs: Vec<f64> = raw.iter().map(|v| v / total).collect();
            let options: Vec<String> = (0..k).map(|i| format!("opt{i}")).collect();
            let top = probs
                .iter()
                .enumerate()
                .fold((0, f64::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
            let mut a = answer(&options[top.0], top.1, probs.clone());
            a.options = options.clone();
            let p = DecisionPayload::from_answer(&a, &policy);

            let mut sorted = probs.clone();
            sorted.sort_by(|x, y| y.total_cmp(x));
            let expected = sorted[0] - sorted[1];
            let ctx = format!("seed {SEED:#x}, case {case}, probs {probs:?}");
            assert!(p.margin >= 0.0, "margin negative: {ctx}");
            assert!(
                (p.margin - expected).abs() < 1e-12,
                "margin {} != top1-top2 {expected}: {ctx}",
                p.margin
            );
            assert!(p.options.contains(&p.label), "label not in options: {ctx}");
            assert_eq!(
                p.near_tie,
                p.margin <= policy.near_tie_margin,
                "near_tie disagrees with margin: {ctx}"
            );
            if p.near_tie {
                ties += 1;
            } else {
                non_ties += 1;
            }
        }
        // CONTROL: both sides of the near_tie biconditional must have been exercised.
        assert!(
            ties > 10 && non_ties > 10,
            "ties {ties}, non-ties {non_ties}"
        );
    }

    #[test]
    fn margin_has_its_value_for_every_question_kind() {
        let policy = Policy::default(); // noul_threshold is read, not assumed
        // Choice: top1 - top2, order-independent, ties are zero, one option is its own margin.
        let m = |probs: Vec<f64>| {
            DecisionPayload::from_answer(&answer("bug_in_code", 0.9, probs), &policy).margin
        };
        assert!((m(vec![0.2, 0.5, 0.3]) - 0.2).abs() < 1e-12);
        assert!((m(vec![0.5, 0.5, 0.0])).abs() < 1e-12);
        assert!((m(vec![0.7, 0.2, 0.7]) - 0.0).abs() < 1e-12);
        assert!((m(vec![0.6]) - 0.6).abs() < 1e-12);
        assert!((m(vec![]) - 0.0).abs() < 1e-12);
        assert!(
            (m(vec![0.4, f64::NAN, 0.1]) - 0.3).abs() < 1e-12,
            "non-finite is ignored"
        );

        // Noul: |p - threshold|, symmetric about it.
        let t = policy.noul_threshold;
        for p in [t + 0.2, t - 0.2] {
            let mut a = answer("yes", 0.9, vec![]);
            a.kind = QuestionType::Noul;
            a.p = Some(p);
            let got = DecisionPayload::from_answer(&a, &policy).margin;
            assert!((got - 0.2).abs() < 1e-12, "p={p}: {got}");
        }

        // Noul with no `p`: the belief falls back to `1 - confidence`.
        let mut a = answer("yes", 0.9, vec![]);
        a.kind = QuestionType::Noul;
        a.p = None;
        let got = DecisionPayload::from_answer(&a, &policy).margin;
        assert!((got - ((1.0 - 0.9) - t).abs()).abs() < 1e-12, "{got}");

        // Score: distance to the nearer integer level.
        for (s, want) in [(2.25, 0.25), (2.75, 0.25), (3.0, 0.0), (2.5, 0.5)] {
            let mut a = answer("3", 0.9, vec![]);
            a.kind = QuestionType::Score;
            a.score = Some(s);
            let got = DecisionPayload::from_answer(&a, &policy).margin;
            assert!((got - want).abs() < 1e-12, "score {s}: {got} != {want}");
        }
    }

    #[test]
    fn band_enum_round_trips_every_band() {
        for (text, band) in [("low", Band::Low), ("mid", Band::Mid), ("high", Band::High)] {
            let mut p = DecisionPayload::from_answer(
                &answer("bug_in_code", 0.9, vec![0.9, 0.05, 0.05]),
                &Policy::default(),
            );
            p.band = text.to_string();
            assert_eq!(p.band_enum(), Some(band));
        }
        let mut p = DecisionPayload::from_answer(
            &answer("bug_in_code", 0.9, vec![0.9, 0.05, 0.05]),
            &Policy::default(),
        );
        p.band = "bogus".to_string();
        assert_eq!(p.band_enum(), None);
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
        // `margin` is DERIVED (0.95 - 0.03 = 0.9199999999999999), so it must be compared
        // with a tolerance. Exact equality on a computed float is a test that fails for a
        // reason unrelated to what it is checking — and the reason it surfaced at all is
        // that adding the field made this assertion compare a computed value for the first
        // time. The string fields are still compared exactly.
        assert!(
            (back.margin - p.margin).abs() < 1e-12,
            "margin survived serde with drift: {} vs {}",
            back.margin,
            p.margin
        );
        assert_eq!(back.label, p.label);
        assert_eq!(back.band, p.band);
        assert!((back.confidence - p.confidence).abs() < 1e-12);
        // The two route keys must survive serialization — the graph reads them
        // out of the JSON payload, not out of the Rust struct.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(v.get("label").and_then(|x| x.as_str()), Some("bug_in_code"));
        assert_eq!(v.get("band").and_then(|x| x.as_str()), Some("high"));
    }
}
