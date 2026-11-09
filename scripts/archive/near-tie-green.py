#!/usr/bin/env python3
"""GREEN: implement the margin, the near-tie flag, and the band demotion.

THE DESIGN DECISION THIS ENCODES (stated, because it is the load-bearing one):

  A near tie must not be decided silently. Two ways to achieve that:
    (a) demote the BAND so `route on = "$.payload.band"` sends it to an ask edge;
    (b) leave the band alone and make the caller check `near_tie`.

  (a) is chosen, and made OPT-IN (`Policy::escalate_on_near_tie`, default FALSE), because:
    * the band is already the documented escalation channel — a graph author writes
      `route on = "$.payload.band"` and gets the behaviour with no new edge logic;
    * demoting by DEFAULT would change every existing caller's behaviour silently, which is
      exactly the class of defect this project keeps finding. Opt-in means the change is
      visible in a config diff.
  The `near_tie` flag is reported either way, so the fact is never hidden — only the
  automatic consequence is opt-in.

MARGIN SEMANTICS, and why they differ by question type:
    * choice -> top1 - top2 (computed over the TOP TWO, not the first two, because the
      probabilities arrive parallel to `options` and need not be sorted);
    * noul   -> |p - noul_threshold| (a noul has no second place; its boundary is the
      threshold, so the distance from it is the quantity that answers "how close was this
      to flipping?");
    * score  -> the gap to the nearest adjacent rubric level (a score sits between levels).

A single-option choice has no second place: the margin is its whole mass and it is NOT a
near tie. Treating an absent second value as 0.0 would report it as maximally close.
"""
import pathlib

# ---- 1. Policy: the near-tie threshold and the opt-in ----------------------
pol = pathlib.Path("/projects/laya-rs/crates/laya-core/src/policy.rs")
t = pol.read_text()


def sub(text, old, new):
    if old not in text:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:200]}")
    return text.replace(old, new, 1)


t = sub(t, '''    pub saturation_threshold: f64,''',
'''    pub saturation_threshold: f64,
    /// A margin at or below this is a **near tie**.
    ///
    /// The measured drift between the two backends is Δp = 0.0006, so a decision whose top
    /// two probabilities are closer than this can flip when the backend, the quantisation
    /// or the batch shape changes. Such a decision must not be made silently.
    pub near_tie_margin: f64,
    /// When true, a near tie cannot report the High band, so `route on =
    /// "$.payload.band"` sends it to an ask/escalate edge instead of acting.
    ///
    /// **Default false, deliberately.** Demoting by default would change every existing
    /// caller's routing without anything in their config changing — silent behaviour
    /// change is the defect class this project keeps finding. Opt-in puts the change in a
    /// diff. The `near_tie` flag is reported regardless, so nothing is hidden.
    pub escalate_on_near_tie: bool,''')

t = sub(t, '''            saturation_threshold: 0.999,''',
'''            saturation_threshold: 0.999,
            // 0.01: an order of magnitude above the measured 0.0006 backend drift, so a
            // decision has to be closer than that to be considered unstable.
            near_tie_margin: 0.01,
            escalate_on_near_tie: false,''')

# validation, next to the saturation check
t = sub(t, '''        if !(0.0..=1.0).contains(&self.noul_threshold) {''',
'''        // A near-tie margin of 0 or less would flag nothing (or everything below zero),
        // and one above 0.5 would flag every possible two-option answer. Both are
        // configuration errors that would otherwise be silent.
        if !(0.0..=0.5).contains(&self.near_tie_margin) {
            return Err(PolicyError::NearTieMarginOutOfRange(self.near_tie_margin));
        }
        if !(0.0..=1.0).contains(&self.noul_threshold) {''')

t = sub(t, '''    #[error("noul_threshold = {0} is outside 0.0..=1.0")]''',
'''    /// The near-tie margin must be in `0.0..=0.5`.
    #[error("near_tie_margin = {0} is outside 0.0..=0.5")]
    NearTieMarginOutOfRange(f64),
    #[error("noul_threshold = {0} is outside 0.0..=1.0")]''')

pol.write_text(t)
print("  Policy: near_tie_margin + escalate_on_near_tie + validation")

# ---- 2. the margin computation, as pure functions --------------------------
rp = pathlib.Path("/projects/laya-rs/crates/laya-core/src/route_payload.rs")
t = rp.read_text()

t = sub(t, '''    /// True when one option carried effectively all the probability mass.''',
'''    /// The distance between the winner and the runner-up, in probability.
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
    /// True when one option carried effectively all the probability mass.''')

t = sub(t, '''        Self {
            label: answer.value.clone(),
            band: policy.band(effective).as_str().to_string(),''',
'''        let margin = margin_of(answer, policy);
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
            band: band.as_str().to_string(),''')

t = sub(t, '''            question_id: answer.id.clone(),
            saturated,
        }
    }''',
'''            question_id: answer.id.clone(),
            saturated,
            margin,
            near_tie,
        }
    }''')

# the margin function, beside the saturation helper
t = sub(t, '''/// Damp a saturated confidence below the high band,''',
'''/// The margin for an answer: how far its decision was from flipping.
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
            let mut top = answer.probabilities.iter().copied().filter(|v| v.is_finite());
            let Some(first) = top.next() else {
                return 0.0;
            };
            let mut sorted = [first, f64::NEG_INFINITY];
            for v in top {
                if v > sorted[0] {
                    sorted[1] = sorted[0];
                    sorted[0] = v;
                } else if v > sorted[1] {
                    sorted[1] = v;
                }
            }
            if sorted[1].is_finite() {
                sorted[0] - sorted[1]
            } else {
                // One option only: no runner-up, so no tie is possible.
                sorted[0]
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

/// Damp a saturated confidence below the high band,''')

rp.write_text(t)
print("  DecisionPayload: margin + near_tie + band demotion + margin_of()")
