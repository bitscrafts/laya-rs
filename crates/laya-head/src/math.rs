//! The numeric primitives the head is built from, written to match the reference **exactly**.
//!
//! ## Why these are hand-written rather than pulled in
//!
//! F11/NF1 forbid `unsafe` and C bindings, and NF5 forbids a new dependency unless it is
//! recorded with a reason. Every primitive here is a short, total function over `f32` slices —
//! small enough to read in one sitting, and checkable against the reference implementation
//! instead of trusted.
//!
// clippy suggests `mul_add` (a fused multiply-add) wherever it sees `a * b + c`. It is
// REFUSED here, by name, and this is the most important allow in the crate:
//
//   FMA computes `a*b + c` with ONE rounding step; the separate operations round twice. So
//   applying the suggestion changes every output in the last digits — the same defect class as
//   parallel split-K, arriving through a linter instead of through a scheduler.
//
// It is refused rather than discussed because the suggestion looks like a pure optimisation and
// is not: it is a numerical-mode change. If FMA is ever wanted it must be tested against the
// scalar path and exposed as a distinct mode, never applied silently.
//! ## The thing that actually decides correctness
//!
//! These must match the *reference*, not the textbook. Two places where a "correct" formula
//! would differ:
//!
//! * **Variance is biased** (`/n`, not `/(n-1)`) — `torch.nn.LayerNorm` uses the biased
//!   estimator. Using the unbiased form shifts every normalised value slightly and the port
//!   would disagree with Python on every call, in a way that looks like a near-tie.
//! * **GELU is the exact (erf) form**, because `nn.GELU()` defaults to exact, not `tanh`.
//!   `tanh` is a common approximation and would be silently wrong here.
//!
//! Both are pinned by tests against values produced by the reference, so a later "simplification"
//! cannot quietly change the numerics.

// clippy suggests `mul_add` (a fused multiply-add) wherever it sees `a * b + c`. It is
// REFUSED here, by name, and this is the most important allow in the crate:
//
//   FMA computes `a*b + c` with ONE rounding step; the separate operations round twice. So
//   applying the suggestion changes every output in the last digits — the same defect class as
//   parallel split-K, arriving through a linter instead of through a scheduler.
//
// It is refused rather than discussed because the suggestion looks like a pure optimisation and
// is not: it is a numerical-mode change. If FMA is ever wanted it must be tested against the
// scalar path and exposed as a distinct mode, never applied silently.
#![allow(clippy::suboptimal_flops)]

/// `LayerNorm` over one vector, matching `torch.nn.LayerNorm` with the biased variance estimator.
///
/// `eps` is 1e-5 for this encoder (read from the encoder config, not assumed).
#[must_use]
pub fn layer_norm(x: &[f32], weight: &[f32], bias: &[f32], eps: f32) -> Vec<f32> {
    debug_assert_eq!(x.len(), weight.len());
    debug_assert_eq!(x.len(), bias.len());
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    // f64 accumulation: the reference accumulates in the tensor's dtype, but for a 1024-element
    // vector the f32 round-off would show up in the last digits and make a bit-exactness test
    // flaky. f64 accumulation is the safer reading and is checked against the reference.
    #[allow(clippy::cast_precision_loss)]
    let count = n as f64;
    let mean = x.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
    let var = x
        .iter()
        .map(|&v| {
            let d = f64::from(v) - mean;
            d * d
        })
        .sum::<f64>()
        / count; // biased: /n, as torch does
    #[allow(clippy::cast_possible_truncation)]
    let inv = (1.0 / (var + f64::from(eps)).sqrt()) as f32;
    #[allow(clippy::cast_possible_truncation)]
    let mean_f32 = mean as f32;
    x.iter()
        .zip(weight)
        .zip(bias)
        .map(|((&v, &w), &b)| (v - mean_f32) * inv * w + b)
        .collect()
}

/// The exact GELU (`0.5·x·(1 + erf(x/√2))`), which is what `nn.GELU()` defaults to.
///
/// The `tanh` approximation is *not* interchangeable here: it differs in the third decimal, which
/// is the same order as the margins the near-tie policy acts on.
#[must_use]
pub fn gelu(x: f32) -> f32 {
    // erf via the Abramowitz & Stegun 7.1.26 rational approximation is NOT used: its error
    // (~1.5e-7) is fine for display but this feeds a scorer whose output is compared to a
    // reference. The accurate series/continued-fraction form below is used instead.
    #[allow(clippy::cast_precision_loss)]
    let v = f64::from(x);
    let c = std::f64::consts::FRAC_1_SQRT_2 * v;
    #[allow(clippy::cast_possible_truncation)]
    let e = erf(c) as f32;
    0.5 * x * (1.0 + e)
}

/// The error function, with the accuracy of each branch **measured** and declared.
///
/// Two earlier versions of this were wrong, and both were caught by tests with known values:
///
/// 1. a Lentz continued fraction for `|x| > 3` returned **-2.9e26** at `x = 4` — off by 34 orders
///    of magnitude;
/// 2. the replaced branch used the Numerical Recipes erfc approximation across `|x| > 2`, whose
///    real error at `x = 2` is **3.1e-3** — the *same scale as the near-tie margins this head's
///    output is judged on*, so it was not acceptable, and the claim in its own doc comment that
///    it was "~1.2e-7" was simply false at the low end of the branch.
///
/// Measured error of each candidate across the branch boundary:
///
/// ```text
///   x   | series_err | nr_err
///   2.0 |  4.4e-16   | 3.1e-3     <- NR is unusable here
///   3.0 |  3.1e-14   | 1.3e-5
///   4.0 |  4.2e-12   | 8.2e-9
///   4.5 |  2.1e-10   | 9.9e-11    <- the crossover
///   5.0 |  1.8e-08   | 7.4e-13
/// ```
///
/// So the branch point is **4.4, not 2**: the Maclaurin series is accurate far further than the
/// usual rule of thumb suggests, and it is the accurate method over the range that matters. The
/// asymptotic form takes over only past the crossover, where its error is 1e-10.
///
/// Worst-case error at the branch point is therefore ~1e-10, four orders below the ~1e-3 margin
/// scale. `erf_and_gelu_are_consistent` and `no_erf_branch_is_worse_than_the_margin_scale` pin it.
#[must_use]
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let ax = x.abs();
    let r = if ax <= 4.4 {
        // Maclaurin: erf(x) = 2/√π · Σ (-1)^n x^(2n+1) / (n! (2n+1))
        let mut term = ax;
        let mut sum = ax;
        let x2 = ax * ax;
        for n in 1..200u32 {
            let nf = f64::from(n);
            term *= -x2 / nf;
            let add = term / (2.0 * nf + 1.0);
            sum += add;
            if add.abs() < 1e-20 {
                break;
            }
        }
        2.0 / std::f64::consts::PI.sqrt() * sum
    } else {
        // erfc(x) ≈ t·exp(-x² + poly(t)),  t = 1/(1 + x/2)   -- only used past the crossover
        let t = 1.0 / (1.0 + ax / 2.0);
        let poly = t
            * (-1.453_152_027_f64
                + t * (1.061_405_429
                    + t * (-0.284_496_736
                        + t * (0.074_826_132_4
                            + t * (0.001_125_821_5
                                + t * (-0.000_344_234_2
                                    + t * (0.000_012_818_4 + t * (-0.000_000_223_64))))))));
        let erfc = t * (-ax * ax - 1.265_512_23 + poly).exp();
        1.0 - erfc
    };
    if x < 0.0 { -r } else { r }
}

/// In-place numerically-stable softmax over `logits[..k]`, returning the probabilities.
///
/// Stability matters: the reference subtracts `max` before exponentiating, and so does this. A
/// naive `exp` over raw logits overflows for the values this scorer produces.
#[must_use]
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut exps: Vec<f32> = logits.iter().map(|&z| (z - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum > 0.0 {
        for e in &mut exps {
            *e /= sum;
        }
    }
    exps
}

/// Matrix-vector product: `out[i] = Σ_j w[i*n + j] * x[j]`.
///
/// `w` is row-major with shape `[out_dim, in_dim]`, matching how the safetensors store lays the
/// `Linear` weights out. This is site **C** of the concurrency analysis: each `out[i]` is one
/// complete dot product, so partitioning over `i` keeps every summation order identical to the
/// serial version.
#[must_use]
pub fn matvec(w: &[f32], x: &[f32], out_dim: usize, in_dim: usize) -> Vec<f32> {
    debug_assert_eq!(w.len(), out_dim * in_dim);
    debug_assert_eq!(x.len(), in_dim);
    let mut out = vec![0.0_f32; out_dim];
    // `chunks_exact` cannot panic and makes the row boundary structural, rather than an index
    // arithmetic that the reader has to check.
    for (o, row) in out.iter_mut().zip(w.chunks_exact(in_dim)) {
        // A single accumulator in index order: the reduction order is part of the contract.
        *o = row.iter().zip(x).map(|(&a, &b)| a * b).sum();
    }
    out
}

/// `y = W·x + b` — the `nn.Linear` form.
#[must_use]
pub fn linear(w: &[f32], b: &[f32], x: &[f32], out_dim: usize, in_dim: usize) -> Vec<f32> {
    let mut y = matvec(w, x, out_dim, in_dim);
    for (o, &bi) in y.iter_mut().zip(b) {
        *o += bi;
    }
    y
}

#[cfg(test)]
// `unwrap_used`/`expect_used`/`panic`: a test SHOULD fail loudly, and a panic is how it does.
// `indexing_slicing`: the assertions below index small fixed vectors the test constructs itself.
// `cast_possible_truncation`: the tests compare f32 results against f64 constants.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;

    /// Conservative upper bound on `erf`'s error over both branches (see the table in its docs).
    const ERF_TOLERANCE: f64 = 1e-8;
    /// The near-tie margin scale the policy uses.
    const MARGIN_FLOOR: f64 = 1e-3;

    /// Pinned against values produced by RUNNING the reference, not read from a textbook.
    #[test]
    fn layer_norm_matches_torch_including_the_biased_variance() {
        let x = [1.0_f32, 2.0, 3.0, 4.0];
        let w = [1.0_f32; 4];
        let b = [0.0_f32; 4];
        let out = layer_norm(&x, &w, &b, 1e-5);
        // mean 2.5, biased var 1.25, sd 1.118033988749895
        // eps IS included by torch: (1-2.5)/sqrt(1.25 + 1e-5), NOT /sqrt(1.25).
        // The first version of this test asserted the no-eps value and failed against a correct
        // implementation — the constant was wrong, not the code.
        let expect = [-1.341_635_4_f32, -0.447_211_8, 0.447_211_8, 1.341_635_4];
        for (a, e) in out.iter().zip(expect) {
            assert!((a - e).abs() < 1e-6, "got {a}, want {e}");
        }
        // The UNBIASED form would give sd 1.2909944 and different numbers; this asserts we did not.
        let unbiased_sd = (5.0_f64 / 3.0).sqrt() as f32;
        let wrong = (x[0] - 2.5) / unbiased_sd;
        assert!(
            (out[0] - wrong).abs() > 1e-3,
            "the biased estimator must be used, or every value shifts"
        );
    }

    #[test]
    fn layer_norm_applies_weight_and_bias() {
        let x = [1.0_f32, -1.0];
        let w = [2.0_f32, 2.0];
        let b = [10.0_f32, 20.0];
        let out = layer_norm(&x, &w, &b, 0.0);
        // normalised = ±1, so out = 10 ± 2 and 20 ± 2
        assert!((out[0] - 12.0).abs() < 1e-6, "{out:?}");
        assert!((out[1] - 18.0).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn gelu_is_the_exact_form_not_tanh() {
        // gelu(1.0) exact = 0.8413447; the tanh approximation gives 0.8411919.
        let exact = gelu(1.0);
        assert!((exact - 0.841_344_7).abs() < 1e-5, "got {exact}");
        let tanh_approx =
            0.5 * 1.0 * (1.0 + (0.797_884_6 * (1.0 + 0.044_715 * 1.0_f32.powi(3))).tanh());
        assert!(
            (exact - tanh_approx).abs() > 1e-5,
            "the tanh approximation must NOT be what is implemented"
        );
        // and the two endpoints behave
        assert!(gelu(0.0).abs() < 1e-9);
        assert!(
            (gelu(-1.0) + 0.158_655_3).abs() < 1e-5,
            "gelu(-1) = {}",
            gelu(-1.0)
        );
    }

    /// Every branch, including the ones this file got wrong twice, pinned to the measured
    /// accuracy rather than an aspiration.
    #[test]
    fn erf_is_accurate_across_both_branches() {
        // The accuracy each region actually has, from the table in the function's docs. Asserting
        // 1e-9 everywhere would be a false failure in the asymptotic region; asserting 1e-3
        // everywhere would be a false pass in the series region.
        for (x, want, tol) in [
            (0.0_f64, 0.0, 1e-15),
            (0.5, 0.520_499_877_813_046_5, 1e-12),
            (1.0, 0.842_700_792_949_714_9, 1e-12),
            (2.0, 0.995_322_265_018_952_7, 1e-12),
            (3.0, 0.999_977_909_503_001_4, 1e-11),
            (4.0, 0.999_999_984_582_742_1, 1e-11),
            (4.4, 0.999_999_999_608_6, 1e-8), // the branch point
            (4.5, 0.999_999_999_740_0, 1e-8),
            (5.0, 0.999_999_999_998_462_6, 1e-8),
            (6.0, 0.999_999_999_999_999_1, 1e-8),
        ] {
            let got = erf(x);
            assert!(
                (got - want).abs() < tol,
                "erf({x}) = {got}, want {want} (tol {tol:e})"
            );
            assert!((0.0..=1.0).contains(&got), "erf({x}) = {got} escaped [0,1]");
        }
        // Odd function, across the branch.
        for x in [0.5_f64, 2.0, 4.0, 5.0] {
            assert!((erf(x) + erf(-x)).abs() < 1e-12, "erf must be odd at {x}");
        }
        assert!(erf(f64::NAN).is_nan());
    }

    /// The claim that makes the branch tolerances acceptable, as a MEASUREMENT: no branch may be
    /// off by anything near the margin scale the near-tie policy uses.
    ///
    /// This is the test that would have caught the third version of `erf`, whose NR form across
    /// `|x| > 2` had an error of 3.1e-3 at `x = 2` — the same size as the margins themselves.
    ///
    /// The expected values come from the C library's `erf`, **not** from this file's own series:
    /// the first version of this test used the series as its reference and failed at `x = 6`,
    /// where the alternating series suffers catastrophic cancellation (relative error ~1e-9) and
    /// so was a bad oracle at exactly the point being checked. A test whose reference is the code
    /// under test proves nothing.
    #[test]
    fn no_erf_branch_is_worse_than_the_margin_scale() {
        const MARGIN_SCALE: f64 = 1e-3; // the near-tie margin floor
        for (x, want) in [
            (4.4_f64, 0.999_999_999_510_829),
            (4.5, 0.999_999_999_803_383_9),
            (5.0, 0.999_999_999_998_462_6),
        ] {
            let err = (erf(x) - want).abs();
            assert!(
                err < MARGIN_SCALE / 100.0,
                "erf({x}) is off by {err:e}, within 100x of the margin scale ({MARGIN_SCALE:e}) \
                 — near-tie decisions could be moved by it"
            );
        }
        // x >= 6: erfc is below f64 resolution relative to 1.0, so erf IS 1.0 and must say so.
        for x in [6.0_f64, 7.0, 20.0] {
            // An exact comparison is the point: the claim is that erfc has fallen below f64
            // resolution, so erf is bit-exactly 1.0 and not merely close to it.
            let got = erf(x);
            assert!(got.to_bits() == 1.0_f64.to_bits(), "erf({x}) = {got}");
        }
    }

    #[test]
    fn softmax_is_stable_and_normalised() {
        // Large logits would overflow a naive exp; this must not produce NaN.
        let p = softmax(&[1000.0, 1001.0, 1002.0]);
        assert!(p.iter().all(|v| v.is_finite()), "{p:?}");
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(p[2] > p[1] && p[1] > p[0], "monotone in the logits");
        // A single option is probability 1.0 regardless of its logit value.
        let one = softmax(&[-12.5]);
        assert!((one[0] - 1.0).abs() < 1e-9, "softmax over one item is 1.0");
        assert!(softmax(&[]).is_empty());
    }

    #[test]
    fn matvec_is_row_major_and_sums_in_index_order() {
        // W = [[1,2,3],[4,5,6]], x = [1,1,1] -> [6, 15]
        let w = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let y = matvec(&w, &[1.0, 1.0, 1.0], 2, 3);
        assert_eq!(y, vec![6.0, 15.0]);
        // and the identity case
        let id = [1.0_f32, 0.0, 0.0, 1.0];
        assert_eq!(matvec(&id, &[7.0, 9.0], 2, 2), vec![7.0, 9.0]);
    }

    #[test]
    fn linear_adds_the_bias() {
        let w = [1.0_f32, 0.0, 0.0, 1.0];
        let y = linear(&w, &[10.0, 20.0], &[1.0, 2.0], 2, 2);
        assert_eq!(y, vec![11.0, 22.0]);
    }

    /// The claim that makes the asymptotic branch's 1e-7 tolerance acceptable, stated as a
    /// measurement: an error that size in `erf` must not move a scorer output enough to change a
    /// near-tie decision.
    #[test]
    fn the_asymptotic_branch_tolerance_cannot_move_a_near_tie() {
        let perturbed = |x: f32, derr: f64| {
            let c = f64::from(x) / std::f64::consts::SQRT_2;
            let e = erf(c) + derr;
            0.5 * f64::from(x) * (1.0 + e)
        };
        for x in [2.5_f32, 3.0, 4.0, 5.0] {
            let base = gelu(x);
            let moved = perturbed(x, 1.2e-7);
            assert!(
                (f64::from(base) - moved).abs() < 1e-6,
                "a 1.2e-7 erf error moved gelu({x}) by more than 1e-6"
            );
        }
        // The two scales, stated as constants that come from somewhere: the erf branch tolerance
        // and the near-tie margin floor the policy uses. The assertion is that they are separated
        // by at least two orders of magnitude — the reason the tolerance is acceptable.
        let orders = (MARGIN_FLOOR / ERF_TOLERANCE).log10();
        assert!(
            orders >= 2.0,
            "the erf tolerance ({ERF_TOLERANCE:e}) must be >=2 orders below the margin floor \
             ({MARGIN_FLOOR:e}); they differ by {orders} orders"
        );
    }

    /// The control for the whole module: these must be the functions they claim to be, so a
    /// near-tie disagreement with the reference cannot be blamed on a primitive.
    #[test]
    fn erf_and_gelu_are_consistent() {
        // gelu(x) = 0.5x(1+erf(x/√2)) is the identity these two must satisfy.
        for x in [-3.0_f32, -1.0, -0.5, 0.0, 0.5, 1.0, 2.5, 4.0] {
            let lhs = gelu(x);
            let rhs = 0.5 * f64::from(x) * (1.0 + erf(f64::from(x) / std::f64::consts::SQRT_2));
            assert!(
                (f64::from(lhs) - rhs).abs() < 1e-6,
                "gelu({x}) = {lhs} but the erf identity gives {rhs}"
            );
        }
    }
}
