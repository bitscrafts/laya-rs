//! `laya-head` — the Laya decision head in pure Rust.
//!
//! **STATUS: NOT STARTED.** Spec [`specs/03-rust-head.md`](../../../specs/03-rust-head.md)
//! is the contract, including every correction the adversarial review forced.
//!
//! Why this crate exists: the head is **26.5M parameters of pure linear algebra**
//! (2 layers; attention, `LayerNorm`, `GELU`, matmul, softmax — no exotic ops), so the
//! whole decision layer can be Rust with **no Python and no torch**. That also
//! removes the `tokenizers`/`transformers` version conflict that already bit us once.
//!
//! **Three corrections that must land before any code here** (all from the review,
//! `_docs/04-development/DESIGN-AMENDMENTS-2026-09-29.md`):
//!
//! * **`clamp_temperature` is mandatory, not optional** (blocker B2). The raw
//!   config value `choice:11+ = 0.1006` multiplies logits ~10×; Laya clamps to
//!   `[0.5, 5.0]` and warns that affected-bucket confidence is uncalibrated.
//!   Applying the raw value would publish a 0.24 probability as 0.99 and defeat
//!   the confidence gate the entire design depends on.
//! * **All 36 tensors**, including the four `act_head` tensors the original
//!   inventory omitted (finding M3) — and `nhead = max(1, d/64)` is computed in
//!   code, **not read from the config**, so "never hardcode" cannot be followed for
//!   the constant attention needs most.
//! * **The parity oracle is decision-level** (blocker B1 / finding M4): label
//!   agreement plus a margin histogram over a corpus, driven from **text** using
//!   this crate's own tokenizer and `build_sequence` — not `Δp ≤ 0.001` on one
//!   probability, which passes while labels flip on near-ties.
//!
//! The head must also emit the **`margin`** (top1 − top2 logit gap) so a near-tie
//! can be escalated instead of decided, and carry provenance hashes so a swapped
//! backbone fails loudly rather than deciding silently.

#![forbid(unsafe_code)]

pub mod decide;
pub mod end_to_end;
pub mod forward;
pub mod math;

pub mod safetensors;
pub mod tokenizer;

/// The temperature clamp Laya applies to its own config, which a faithful port
/// must reproduce (blocker B2).
///
/// Exposed as constants rather than magic numbers so the port's tests can assert
/// against them and a reader can see the values without opening Laya's source.
pub const TEMP_MIN: f64 = 0.5;
/// See [`TEMP_MIN`].
pub const TEMP_MAX: f64 = 5.0;

/// Clamp a configured temperature the way Laya does.
///
/// NaN becomes 1.0 (Laya's behaviour) — never a panic, never a NaN propagating into
/// a softmax. Out-of-range values are clamped rather than rejected, and the caller
/// is expected to **flag** that it happened, because confidence from a clamped
/// bucket is uncalibrated.
///
/// This function is small and trivial, and it is here rather than inlined for a
/// reason: it is the single most dangerous constant in the port. Getting it wrong
/// silently turns a coin flip into a reported 0.99.
#[must_use]
pub const fn clamp_temperature(configured: f64) -> f64 {
    if configured.is_nan() {
        return 1.0;
    }
    configured.clamp(TEMP_MIN, TEMP_MAX)
}

/// Whether a configured temperature was **changed** by [`clamp_temperature`] — and
/// therefore whether confidence from this bucket should be treated as uncalibrated.
///
/// `true` when the value was out of `[TEMP_MIN, TEMP_MAX]`, **or when it was `NaN`**
/// (replaced by 1.0). Both cases mean the value the author configured is not the
/// value being used, which is exactly what a consumer needs to know.
///
/// NaN is handled explicitly rather than by comparison, because every comparison
/// with NaN is false — including `!=` — so a naive difference check reports a NaN
/// as *unchanged*. That false negative would silently mark uncalibrated confidence
/// as trustworthy. (Found by this module's own test.)
#[must_use]
pub const fn temperature_was_clamped(configured: f64) -> bool {
    configured.is_nan() || (configured - clamp_temperature(configured)).abs() > f64::EPSILON
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;

    #[test]
    fn the_raw_thirteen_option_temperature_is_clamped_to_the_floor() {
        // THE case blocker B2 is about: the shipped config value 0.1006 would
        // multiply logits ~10x and publish a 0.24 probability as 0.99.
        assert!((clamp_temperature(0.100_582_808_256_149_29) - TEMP_MIN).abs() < f64::EPSILON);
        assert!(temperature_was_clamped(0.100_582_808_256_149_29));
    }

    #[test]
    fn an_in_range_temperature_passes_through_unchanged() {
        // The CONTROL: a legitimate config value must not be touched, or the clamp
        // becomes a silent re-calibration of every decision.
        for t in [0.5, 1.0, 1.906_356_334_686_279_3, 5.0] {
            assert!((clamp_temperature(t) - t).abs() < f64::EPSILON, "t={t}");
            assert!(
                !temperature_was_clamped(t),
                "t={t} must not report clamping"
            );
        }
    }

    #[test]
    fn a_high_temperature_is_clamped_to_the_ceiling() {
        assert!((clamp_temperature(9.9) - TEMP_MAX).abs() < f64::EPSILON);
        assert!(temperature_was_clamped(9.9));
    }

    #[test]
    fn a_nan_temperature_becomes_one_and_is_not_a_panic() {
        // A NaN reaching a softmax poisons the whole distribution; Laya maps it to
        // 1.0. We match, and report it as uncalibrated rather than pretending.
        assert!((clamp_temperature(f64::NAN) - 1.0).abs() < f64::EPSILON);
        assert!(temperature_was_clamped(f64::NAN));
    }

    #[test]
    fn the_boundary_values_are_not_reported_as_clamped() {
        // Exactly MIN and exactly MAX are in range — reporting them as clamped would
        // mark every boundary-bucket decision uncalibrated for no reason.
        assert!(!temperature_was_clamped(TEMP_MIN));
        assert!(!temperature_was_clamped(TEMP_MAX));
    }
}
