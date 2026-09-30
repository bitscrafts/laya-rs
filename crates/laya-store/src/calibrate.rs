//! `calibrate` — whether the recorded confidence is worth gating on.
//!
//! ## What this is for
//!
//! The ledger records a decision's `confidence`. It does not, by itself, say whether that number
//! *means* anything. Expected Calibration Error is the measurement that answers it: of the
//! decisions made at ~0.9 confidence, what fraction were actually right?
//!
//! ## The oracle
//!
//! This reproduces `ece_score` from the pinned `laya` package exactly — 15 equal-width bins over
//! `(0, 1]`, each contributing `bin_mass × |mean_confidence − accuracy|`. The reference is the
//! authority, and the tests pin against values produced by **running** it rather than by reading
//! it:
//!
//! | conf | correct | ECE |
//! |---|---|---|
//! | `[1,1,1,1]` | `[1,1,1,1]` | `0.0` |
//! | `[1,1,1,1]` | `[0,0,0,0]` | `1.0` |
//! | `[0.9,0.8,0.7,0.95,0.6]` | `[1,1,1,0,1]` | `0.39` |
//! | `[0.1,0.2,0.9]` | `[0,1,1]` | `0.3333333333333333` |
//! | `[]` | `[]` | `nan` |
//!
//! ## Two things this refuses to do
//!
//! **It will not compute an ECE without a sample size.** With 15 bins and three decisions the
//! number is noise, and a bare "ECE = 0.21" invites exactly the wrong conclusion. The result
//! carries the count and a `conclusive` flag, and the flag is computed rather than asserted.
//!
//! **It will not produce `0` for no data.** An empty input is `nan`, because zero reads as
//! "perfectly calibrated" — a measurement nobody made.

// Clicks: the casts below are `usize <-> f64`. `clippy::pedantic` flags both by default, and both
// are safe HERE for a reason that does not vary by site: a count is bounded by the number of rows
// in the ledger (lossless below 2^53 -- nine quadrillion), and the bin index is bounded by BINS
// because the value was already checked to be inside (0, 1]. The allowance is scoped to this
// module and carries its reason, rather than being scattered over each arithmetic line.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

/// The number of equal-width confidence bins. 15 is the reference's default.
pub const BINS: usize = 15;

/// The fewest samples from which an ECE is worth quoting.
///
/// **A convention, and named as one.** With 15 bins it averages two samples per bin, which is
/// weak; the point of the flag is that a number labelled `conclusive: false` cannot be quoted as
/// if it were a finding. It is not a statistical bound and does not pretend to be.
pub const MIN_SAMPLES: usize = 30;

/// One bin's contribution, so a caller can see where an error comes from rather than only its
/// total.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Bin {
    /// The bin's lower edge (exclusive, as the reference defines it).
    pub lo: f64,
    /// The bin's upper edge (inclusive).
    pub hi: f64,
    /// How many samples fell in it.
    pub count: usize,
    /// Their mean confidence, or `None` when the bin is empty.
    pub mean_confidence: Option<f64>,
    /// The fraction of them that were correct, or `None` when the bin is empty.
    pub accuracy: Option<f64>,
    /// This bin's contribution to the total, `mass × |mean_confidence − accuracy|`.
    pub contribution: f64,
}

/// The result of a calibration pass.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Calibration {
    /// Expected Calibration Error, or `None` when there were no samples at all.
    ///
    /// `None` rather than `NaN` so the type does the work: a caller must handle "no data"
    /// explicitly instead of comparing against `nan` and getting `false` either way.
    pub ece: Option<f64>,
    /// How many `(confidence, correct)` pairs went in.
    pub samples: usize,
    /// Decisions that have no outcome recorded, and therefore could not be judged.
    pub unjudged: usize,
    /// Whether the sample is large enough to conclude anything from (`samples >= MIN_SAMPLES`).
    pub conclusive: bool,
    /// The threshold that flags it, so a reader does not have to find it in code.
    pub min_samples: usize,
    /// The fraction of judged decisions that were correct.
    pub accuracy: Option<f64>,
    /// The mean reported confidence across judged decisions.
    pub mean_confidence: Option<f64>,
    /// Every bin, including the empty ones, so a gap in coverage is visible.
    pub bins: Vec<Bin>,
    /// The saturation rate over **all** decisions — the share whose answer was saturated. A rate
    /// near 1.0 means the confidence carries almost no information, which matters more than the
    /// ECE when deciding whether to gate on it at all.
    pub saturation_rate: Option<f64>,
}

impl Calibration {
    /// Compute the calibration of a set of `(confidence, correct)` pairs.
    ///
    /// # Precision
    /// Sample counts are converted to `f64`, which is lossless below 2^53 (nine
    /// quadrillion) samples -- a corpus no calibration will ever approach, and the same
    /// conversion the reference makes.
    ///
    /// `unjudged` and `saturation_rate` are passed in from the ledger rather than derived here:
    /// decisions without outcomes cannot be judged, and the saturation rate is a property of all
    /// decisions rather than of the judged subset. Keeping them as inputs means this function
    /// stays a pure function of what it is given.
    #[must_use]
    pub fn compute(pairs: &[(f64, bool)], unjudged: usize, saturation_rate: Option<f64>) -> Self {
        let samples = pairs.len();
        if samples == 0 {
            // Every field is honest about there being nothing: `None`, not zero.
            return Self {
                ece: None,
                samples: 0,
                unjudged,
                conclusive: false,
                min_samples: MIN_SAMPLES,
                accuracy: None,
                mean_confidence: None,
                bins: empty_bins(),
                saturation_rate,
            };
        }

        let mut tallies = [Tally::default(); BINS];

        for &(conf, correct) in pairs {
            if let Some(i) = bin_index(conf)
                && let Some(tally) = tallies.get_mut(i)
            {
                tally.count += 1;
                tally.confidence += conf;
                if correct {
                    tally.correct += 1.0;
                }
            }
            // A confidence outside every bin (0.0, or not finite) is counted in `samples` and in
            // the overall means but lands in no bin — which is what the reference does, and the
            // consequence is that such a sample contributes nothing to the ECE.
        }

        let mut bins = Vec::with_capacity(BINS);
        let mut ece = 0.0;
        for i in 0..BINS {
            let (lo, hi) = edges(i);
            let tally = tallies.get(i).copied().unwrap_or_default();
            let count = tally.count;
            let (mean_confidence, accuracy, contribution) = if count == 0 {
                (None, None, 0.0)
            } else {
                // Lossless below 2^53 samples; see the module note.
                let n = count as f64;
                let mc = tally.confidence / n;
                let acc = tally.correct / n;
                // `count / samples` is the bin's mass, exactly as the reference computes it.
                let mass = n / samples as f64;
                let contribution = mass * (mc - acc).abs();
                (Some(mc), Some(acc), contribution)
            };
            ece += contribution;
            bins.push(Bin {
                lo,
                hi,
                count,
                mean_confidence,
                accuracy,
                contribution,
            });
        }

        let total = samples as f64;
        let mean_confidence = Some(pairs.iter().map(|(c, _)| c).sum::<f64>() / total);
        let accuracy = Some(pairs.iter().filter(|(_, k)| *k).count() as f64 / total);

        Self {
            ece: Some(ece),
            samples,
            unjudged,
            conclusive: samples >= MIN_SAMPLES,
            min_samples: MIN_SAMPLES,
            accuracy,
            mean_confidence,
            bins,
            saturation_rate,
        }
    }

    /// A one-line statement of the result, including the reason to distrust it when it is small.
    ///
    /// The wording is part of the contract: a caller that prints this cannot accidentally present
    /// an ECE as a finding without the sample size next to it.
    #[must_use]
    pub fn verdict(&self) -> String {
        let Some(ece) = self.ece else {
            return "no judged decisions: nothing to calibrate. Record outcomes with \
                    `laya ledger outcome <id> --correct|--wrong --source <who>`"
                .to_string();
        };
        let base = format!(
            "ECE {ece:.4} over {} judged decision(s){}",
            self.samples,
            if self.unjudged > 0 {
                format!(" ({} still unjudged)", self.unjudged)
            } else {
                String::new()
            }
        );
        if self.conclusive {
            base
        } else {
            format!(
                "{base} — INCONCLUSIVE: fewer than {} samples, so this number is noise and must \
                 not be quoted as a finding",
                self.min_samples
            )
        }
    }
}

/// One bin's running totals during accumulation.
///
/// Kept together so the three counters cannot drift out of step, and so accumulating a sample
/// needs ONE indexed access rather than three that must agree.
#[derive(Default, Clone, Copy)]
struct Tally {
    count: usize,
    confidence: f64,
    correct: f64,
}

/// The bin a confidence falls in, or `None` when it falls in no bin.
///
/// **The first edge is exclusive**, matching the reference's `(conf > lo) & (conf <= hi)`. A
/// confidence of exactly `0.0` is therefore in NO bin — it is not clamped into the first one. That
/// is a real behaviour with a real consequence (such a sample contributes nothing to the ECE), and
/// it is reproduced rather than "fixed", because diverging from the oracle silently is the defect
/// this whole module is built to avoid.
#[must_use]
pub fn bin_index(confidence: f64) -> Option<usize> {
    if !confidence.is_finite() {
        return None;
    }
    // Right edge is inclusive only for the LAST bin.
    if confidence <= 0.0 || confidence > 1.0 {
        return None;
    }
    let idx = ((confidence * BINS as f64).ceil() as usize).saturating_sub(1);
    Some(idx.min(BINS - 1))
}

/// The `[lo, hi]` edges of bin `i`.
#[must_use]
pub fn edges(i: usize) -> (f64, f64) {
    let step = 1.0 / BINS as f64;
    (step * i as f64, step * (i + 1) as f64)
}

/// Every bin, empty. Shared so the empty result and a computed result describe bins identically.
fn empty_bins() -> Vec<Bin> {
    (0..BINS)
        .map(|i| {
            let (lo, hi) = edges(i);
            Bin {
                lo,
                hi,
                count: 0,
                mean_confidence: None,
                accuracy: None,
                contribution: 0.0,
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    fn ece_of(pairs: &[(f64, bool)]) -> f64 {
        Calibration::compute(pairs, 0, None).ece.expect("some ece")
    }

    /// The five vectors, with the values produced by RUNNING the reference implementation.
    /// Pinning against the oracle rather than against my own arithmetic is the whole point.
    #[test]
    fn ece_matches_the_reference_on_the_pinned_vectors() {
        let yes = true;
        let no = false;
        assert_eq!(
            ece_of(&[(1.0, yes), (1.0, yes), (1.0, yes), (1.0, yes)]),
            0.0,
            "perfect: all confident, all correct"
        );
        assert_eq!(
            ece_of(&[(1.0, no), (1.0, no), (1.0, no), (1.0, no)]),
            1.0,
            "all confident, all wrong"
        );
        let small = ece_of(&[(0.9, yes), (0.8, yes), (0.7, yes), (0.95, no), (0.6, yes)]);
        assert!(
            (small - 0.39).abs() < 1e-12,
            "the reference gives 0.39, got {small}"
        );
        let two = ece_of(&[(0.1, no), (0.2, yes), (0.9, yes)]);
        assert!(
            (two - 0.333_333_333_333_333_3).abs() < 1e-12,
            "the reference gives 0.3333333333333333, got {two}"
        );
    }

    #[test]
    fn an_empty_input_is_none_not_zero() {
        // Zero would read as "perfectly calibrated", which is a measurement nobody made.
        let c = Calibration::compute(&[], 3, None);
        assert_eq!(c.ece, None);
        assert_eq!(c.samples, 0);
        assert!(!c.conclusive);
        assert!(
            c.verdict().contains("no judged decisions"),
            "{}",
            c.verdict()
        );
        // And it still reports how many decisions are waiting, so the gap is actionable.
        assert_eq!(c.unjudged, 3);
    }

    #[test]
    fn confidence_zero_falls_in_no_bin_exactly_as_the_reference_does() {
        // The reference's first edge is EXCLUSIVE: `(conf > lo) & (conf <= hi)`. Reproduced, not
        // "fixed" -- diverging from the oracle silently is the defect class this module avoids.
        assert_eq!(bin_index(0.0), None, "0.0 is in no bin");
        assert_eq!(bin_index(1.0), Some(BINS - 1), "1.0 is in the last bin");
        assert_eq!(bin_index(0.5), Some(7), "0.5 lands at the 8th edge");
        assert_eq!(bin_index(-0.1), None);
        assert_eq!(bin_index(1.5), None);
        assert_eq!(bin_index(f64::NAN), None);

        // The consequence, stated: a 0.0 sample is counted but contributes nothing to the ECE.
        let c = Calibration::compute(&[(0.0, false)], 0, None);
        assert_eq!(c.samples, 1);
        assert_eq!(c.ece, Some(0.0), "it cannot contribute, so the ECE is 0");
        assert_eq!(
            c.bins.iter().map(|b| b.count).sum::<usize>(),
            0,
            "in no bin"
        );
    }

    #[test]
    fn a_small_sample_is_marked_inconclusive_and_the_number_is_still_given() {
        // The flag is the point: an ECE with `conclusive: false` cannot be quoted as a finding.
        let pairs: Vec<(f64, bool)> = (0..MIN_SAMPLES - 1).map(|_| (0.9, true)).collect();
        let c = Calibration::compute(&pairs, 0, None);
        assert!(!c.conclusive, "below the threshold");
        assert!(c.ece.is_some(), "the number is still computed");
        assert!(c.verdict().contains("INCONCLUSIVE"), "{}", c.verdict());
        assert!(
            c.verdict().contains(&MIN_SAMPLES.to_string()),
            "{}",
            c.verdict()
        );
    }

    #[test]
    fn at_the_threshold_it_becomes_conclusive() {
        // The boundary, both sides. MIN_SAMPLES is a convention, but a boundary still has to be
        // exactly where it says it is.
        let pairs: Vec<(f64, bool)> = (0..MIN_SAMPLES).map(|_| (0.9, true)).collect();
        let c = Calibration::compute(&pairs, 0, None);
        assert!(c.conclusive);
        assert!(!c.verdict().contains("INCONCLUSIVE"), "{}", c.verdict());
    }

    #[test]
    fn the_saturation_rate_is_carried_through_because_it_matters_more_than_the_ece() {
        // A saturation rate near 1.0 means the confidence carries almost no information. A
        // reader deciding whether to gate on confidence at all needs it.
        let c = Calibration::compute(&[(0.99, true)], 0, Some(1.0));
        assert_eq!(c.saturation_rate, Some(1.0));
    }

    #[test]
    fn the_bin_table_explains_where_the_error_comes_from() {
        // A total with no breakdown tells a reader the model is miscalibrated but not HOW, and
        // "how" is what decides whether a threshold can be fixed.
        let c = Calibration::compute(&[(0.9, false), (0.9, false), (0.1, true)], 0, None);
        let i = bin_index(0.9).expect("bin");
        let bin = c.bins.get(i).expect("the bin exists");
        assert_eq!(bin.count, 2);
        assert_eq!(bin.accuracy, Some(0.0), "both wrong");
        assert!((bin.mean_confidence.unwrap() - 0.9).abs() < 1e-12);
        assert!(bin.contribution > 0.0, "this bin is where the error is");
        // Every bin is present, including the empty ones, so a coverage gap is visible.
        assert_eq!(c.bins.len(), BINS);
    }

    #[test]
    fn a_perfectly_calibrated_set_has_near_zero_ece() {
        // The control at the other end: the metric must be able to report "fine", or it only
        // ever proves that things are wrong.
        // 0.8 confidence, 80% right -- the calibration the number claims.
        let mut pairs: Vec<(f64, bool)> = Vec::new();
        for i in 0..40 {
            pairs.push((0.75, i < 30)); // 75% correct at 0.75 confidence
        }
        let c = Calibration::compute(&pairs, 0, None);
        assert!(c.ece.expect("ece") < 0.02, "got {:?}", c.ece);
        assert!(c.conclusive);
    }

    #[test]
    fn the_overall_means_are_reported_alongside_the_ece() {
        let c = Calibration::compute(&[(0.8, true), (0.4, false)], 0, None);
        assert!((c.mean_confidence.unwrap() - 0.6).abs() < 1e-12);
        assert!((c.accuracy.unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn edges_are_equal_width_and_cover_the_unit_interval() {
        assert_eq!(edges(0), (0.0, 1.0 / 15.0));
        let (lo, hi) = edges(BINS - 1);
        assert!((hi - 1.0).abs() < 1e-12, "the last bin ends at 1.0");
        assert!(lo < hi);
        // Adjacent bins touch: no gap, no overlap.
        for i in 1..BINS {
            assert!(
                (edges(i).0 - edges(i - 1).1).abs() < 1e-12,
                "bins must be contiguous"
            );
        }
    }
}
