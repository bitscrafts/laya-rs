//! Thresholds, confidence bands and composite weights — **all caller-side**.
//!
//! The video's central discipline is that the model never sees a threshold:
//! *"Jev never sees the weights. Tuning means changing a number, not a prompt."*
//! This module is that idea made concrete. Every number a decision depends on is
//! here, in one reviewable place, loadable from TOML.

use serde::{Deserialize, Serialize};

use laya_abi::QuestionType;

/// Apply a threshold pair to a confidence.
///
/// One place, so the two call sites (`band` and `band_for`) can never drift
/// apart. Boundary semantics: `>= high` is High, `< low` is Low, so the low
/// boundary belongs to the band above it.
fn classify(confidence: f64, low: f64, high: f64) -> Band {
    if confidence >= high {
        Band::High
    } else if confidence < low {
        Band::Low
    } else {
        Band::Mid
    }
}

/// Where an answer's confidence falls, per the caller's thresholds.
///
/// This is the value a DAG routes on. Keeping it a discrete band rather than a
/// float is deliberate: a float threshold on a model's own probability invites
/// silent drift, whereas a band is an explicitly-authored edge in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    /// Confidence is too low to act on. Route to a human, or to the LLM lane for
    /// a second opinion.
    Low,
    /// Confident enough to ask once / proceed with a checkpoint.
    Mid,
    /// Confident enough that a destructive action may run without confirmation.
    High,
}

impl Band {
    /// The lowercase wire name, so a `route` rule can write `{ eq = "low", … }`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
        }
    }
}

/// What a gate concluded — the three-way outcome the video's L4 describes.
///
/// Note there is **no `block` variant that implies safety**: a gate is one signal
/// among several (the video's own L6 was routed around; its author calls the fix
/// *"an instruction, not a control"*). `Deny` means "this signal says no", and
/// the caller is expected to pair it with other controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    /// Proceed without asking.
    Allow,
    /// The signal refuses. Make the branch terminal; do not retry around it.
    Deny,
    /// The pick is plausible but the confidence is not — escalate.
    Ask,
}

/// A policy error: a config that cannot be trusted.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PolicyError {
    /// A threshold lies outside `0.0..=1.0`.
    #[error("threshold `{name}` = {value} is outside 0.0..=1.0")]
    ThresholdOutOfRange {
        /// Which threshold.
        name: &'static str,
        /// The offending value.
        value: f64,
    },
    /// The two thresholds are inverted, so no band could ever be `Mid`.
    #[error("{which} thresholds are inverted: low ({low}) must be strictly below high ({high})")]
    ThresholdsInverted {
        /// Which threshold pair ("choice/score" or "noul").
        which: &'static str,
        /// The low threshold.
        low: f64,
        /// The high threshold.
        high: f64,
    },
    /// The near-tie margin must be in `0.0..=0.5`.
    #[error("near_tie_margin = {0} is outside 0.0..=0.5")]
    NearTieMarginOutOfRange(f64),
    /// A `noul` threshold lies outside `0.0..=1.0`.
    #[error("noul_threshold = {0} is outside 0.0..=1.0")]
    NoulThresholdOutOfRange(f64),
    /// A `gate_deny` entry is the empty string, which would match every command
    /// (spec 29: an empty-pattern deny is "deny everything" by accident).
    #[error("gate_deny[{index}] is empty: the empty pattern matches EVERY command")]
    EmptyDenyPattern {
        /// Which entry.
        index: usize,
    },
    /// The config could not be parsed.
    #[error("policy config is not valid TOML: {0}")]
    Toml(String),
}

/// The numbers every decision depends on.
///
/// Defaults come from the video's measured bands (below 0.5 a human decides,
/// above 0.9 a destructive action runs unconfirmed) and are intentionally
/// conservative: it is better to escalate than to act on a plausible guess.
///
/// **Per-type thresholds (finding m4 of the design review).** A `noul`'s
/// "confidence" and a `choice`/`score` one are different quantities, so they must
/// not share a boundary:
///
/// * `noul` confidence is `max(p, 1 − p)` — the belief *already folded* so that
///   0.5 means maximally uncertain.
/// * `choice`/`score` confidence is `1 − H(p)/log(k)` — a normalised entropy.
///
/// A single global band therefore made "0.6 confident" mean two different things.
/// [`Policy::band_for`] takes the question type and applies the right threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// At or above this, a `choice`/`score` confidence is `High`.
    pub high_threshold: f64,
    /// Below this, a `choice`/`score` confidence is `Low`.
    pub low_threshold: f64,
    /// [`Self::high_threshold`] for a `noul`.
    pub noul_high_threshold: f64,
    /// [`Self::low_threshold`] for a `noul`.
    pub noul_low_threshold: f64,
    /// For a `noul`, the `p` at or above which the answer counts as true.
    ///
    /// Separate from the confidence bands because a `noul` has no separate
    /// confidence — the probability *is* the belief (Laya's documented shape).
    pub noul_threshold: f64,
    /// When one option takes more than this share of the mass, treat the answer
    /// as suspect rather than as strong evidence.
    ///
    /// This exists because we measured the deployed checkpoint returning exactly
    /// `1.000/0.000/0.000` on a real `choice` — the same degenerate-softmax smell
    /// as the inert `act_probability` head.
    pub saturation_threshold: f64,
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
    pub escalate_on_near_tie: bool,
    /// Whether to refuse to act on a saturated answer at all (`Ask` instead of
    /// acting), or merely record it. Default: record only.
    pub escalate_on_saturation: bool,
    /// Command prefixes `gate_verdict` may allow, matched on whitespace-tokenised words
    /// (spec 08, R1). An **empty** list is a valid policy and means "never allow" — it must
    /// never fall back to a default, because a caller that deliberately locked the gate down
    /// must stay locked down.
    ///
    /// The default is a small read-only set: enough to let an agent orient itself without a
    /// human in the loop, and nothing that writes or deletes.
    pub gate_allow: Vec<String>,
    /// Patterns `gate_verdict` HARD-DENIES, matched as **case-sensitive literal
    /// substrings** of the command text — no globs, no regex, spec 29 F3. (Globs and regex
    /// make a rule a puzzle to audit; a literal needle is greppable and provable.)
    ///
    /// Evaluated BEFORE the allowlist question (spec 29 F2): the deny fires even when the
    /// input is also allowlisted, and even when the model says yes. **Deny beats ask** —
    /// the allowlist otherwise forces `Ask` on everything it does not allow, which made a
    /// deny branch checked later structurally unreachable in production (G2: 5083
    /// decisions, 0 denies).
    ///
    /// An **empty** list is a valid policy and means "deny nothing" (spec 29 F5, same
    /// precedent as an empty allowlist): a deployment that has not deliberately enabled
    /// hard-deny must see today's ask-not-deny behaviour, unchanged. The deployed enable
    /// (`rm -rf`, `of=/dev/`, `git reset --hard`, `git push --force`) is a separate,
    /// orchestrator-reviewed config change, decoupled from this code ship.
    pub gate_deny: Vec<String>,
    /// How long `ask` lets its command run before killing it and reporting `timed_out: true`
    /// (spec 08, R8). A hung command must not stall the serial server.
    pub ask_deadline_secs: u64,
}

/// [`Policy::gate_allow`]'s default: read-only commands an agent needs to orient itself.
fn default_gate_allow() -> Vec<String> {
    [
        "ls",
        "pwd",
        "cat",
        "head",
        "tail",
        "wc",
        "git status",
        "git diff",
        "git log",
        "git show",
        "cargo check",
        "cargo test",
        "cargo clippy",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            high_threshold: 0.9,
            low_threshold: 0.5,
            // Lower than the choice/score pair, DELIBERATELY (spec 08). 0.9 was tuned for
            // the OLD design, where this was the ONLY gate on a destructive action. Measured
            // live (2026-10-01) against allowlisted, read-only commands: `ls -la /tmp`
            // 0.8314, `git status` 0.8722, `cargo test` 0.8327 — all below 0.9, and the
            // model's own separation from destructive commands is NEGATIVE at this range
            // (several destructive commands measured 0.90-0.99 "safe"). So 0.9 would keep
            // `gate_verdict` asking on every legitimate, already-allowlisted command, while
            // not keeping out a single unsafe one the allowlist does not already stop. Safety
            // now lives in `gate_allow` and `destructive_signals`, not here — this threshold
            // only decides whether an ALREADY-vetted command additionally needs a human.
            noul_high_threshold: 0.8,
            noul_low_threshold: 0.5,
            noul_threshold: 0.5,
            saturation_threshold: 0.999,
            // 0.01: an order of magnitude above the measured 0.0006 backend drift, so a
            // decision has to be closer than that to be considered unstable.
            near_tie_margin: 0.01,
            escalate_on_near_tie: false,
            escalate_on_saturation: false,
            gate_allow: default_gate_allow(),
            // F5, spec 29: the default policy hard-denies NOTHING. Enablement is the
            // orchestrator's separate, reviewed config change.
            gate_deny: Vec::new(),
            ask_deadline_secs: 60,
        }
    }
}

impl Policy {
    /// Parse from TOML text.
    ///
    /// # Errors
    /// [`PolicyError::Toml`] if the text is not valid TOML for this shape.
    pub fn from_toml(text: &str) -> Result<Self, PolicyError> {
        toml::from_str(text).map_err(|e| PolicyError::Toml(e.to_string()))
    }

    /// Check that this policy is internally coherent.
    ///
    /// # Errors
    /// A [`PolicyError`] naming the first problem found.
    pub fn validate(&self) -> Result<(), PolicyError> {
        // The band boundaries and the saturation cutoff are all probabilities.
        for (name, value) in [
            ("high_threshold", self.high_threshold),
            ("low_threshold", self.low_threshold),
            ("noul_high_threshold", self.noul_high_threshold),
            ("noul_low_threshold", self.noul_low_threshold),
            ("saturation_threshold", self.saturation_threshold),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(PolicyError::ThresholdOutOfRange { name, value });
            }
        }
        // Checked separately from the loop above, and it must come BEFORE any
        // range check that would also cover it: `noul_threshold` is not a band
        // boundary, so a bad value deserves the narrower error that names it.
        // (Found by test: with it inside the loop, this variant was unreachable.)
        // A near-tie margin of 0 or less would flag nothing (or everything below zero),
        // and one above 0.5 would flag every possible two-option answer. Both are
        // configuration errors that would otherwise be silent.
        if !(0.0..=0.5).contains(&self.near_tie_margin) {
            return Err(PolicyError::NearTieMarginOutOfRange(self.near_tie_margin));
        }
        if !(0.0..=1.0).contains(&self.noul_threshold) {
            return Err(PolicyError::NoulThresholdOutOfRange(self.noul_threshold));
        }
        // A deny pattern is a LITERAL SUBSTRING needle (spec 29, F3). The empty string is
        // a substring of every command, so one empty entry would turn the deny list into
        // "deny everything" — a misconfiguration that must be caught at load, not observed
        // as every call being refused.
        if let Some(ix) = self.gate_deny.iter().position(String::is_empty) {
            return Err(PolicyError::EmptyDenyPattern { index: ix });
        }
        // BOTH threshold pairs must be well-ordered, not just the choice/score
        // pair — a noul pair with low >= high makes the Mid band unreachable for
        // every noul, which is the same silent-hole bug one type over.
        for (which, low, high) in [
            ("choice/score", self.low_threshold, self.high_threshold),
            ("noul", self.noul_low_threshold, self.noul_high_threshold),
        ] {
            if low >= high {
                return Err(PolicyError::ThresholdsInverted { which, low, high });
            }
        }
        Ok(())
    }

    /// Classify a `choice`/`score` confidence into a band.
    ///
    /// Use [`Self::band_for`] when the question type is known — a `noul`'s
    /// confidence is a different quantity and has its own boundaries.
    #[must_use]
    pub fn band(&self, confidence: f64) -> Band {
        classify(confidence, self.low_threshold, self.high_threshold)
    }

    /// Classify a confidence into a band, using the boundaries for `kind`.
    ///
    /// This is the entry point callers should prefer: a `noul` reports
    /// `max(p, 1 − p)` while a `choice`/`score` reports `1 − H(p)/log(k)`, so the
    /// same boundary would mean two different things (design-review finding m4).
    #[must_use]
    pub fn band_for(&self, kind: QuestionType, confidence: f64) -> Band {
        match kind {
            QuestionType::Noul => classify(
                confidence,
                self.noul_low_threshold,
                self.noul_high_threshold,
            ),
            QuestionType::Choice | QuestionType::Score => {
                classify(confidence, self.low_threshold, self.high_threshold)
            }
        }
    }

    /// Decide a gate from a confidence and whether the proposition is one we must
    /// never get wrong.
    ///
    /// `irreversible` is accepted for the caller's record but cannot change the outcome:
    /// the *only* confidence that allows ANY action here is `High`, so an irreversible one
    /// is already held to that bar. (It was matched in a guard whose arm did the same as the
    /// arm after it — a branch no test could tell from its absence, found by mutation.) The video's measured example is the
    /// reason — `rm -rf node_modules && npm install` was classified "reversible"
    /// at confidence 0.35, and 0.35 must ask a human however the pick reads.
    ///
    /// **Superseded by [`crate::agentic::gate_verdict`] for anything that runs a real
    /// command.** This function can reach `Allow` from confidence alone, which is exactly
    /// the defect spec 08 fixed: `allow` now requires a positive allowlist match, which this
    /// method has no notion of. Kept for the generic confidence-to-decision mapping other
    /// callers still use; `laya-mcp`'s `gate`/`ask` tools no longer call it directly.
    #[must_use]
    pub fn gate(&self, confidence: f64, _irreversible: bool, saturated: bool) -> GateDecision {
        if saturated && self.escalate_on_saturation {
            return GateDecision::Ask;
        }
        match self.band(confidence) {
            Band::High => GateDecision::Allow,
            Band::Low => GateDecision::Deny,
            // Mid: the pick may be plausible, but the confidence is what we are acting on.
            Band::Mid => GateDecision::Ask,
        }
    }

    /// Apply a `noul` threshold — the "smart if", with the comparison in code.
    #[must_use]
    pub fn noul_is_true(&self, p: f64) -> bool {
        p >= self.noul_threshold
    }

    /// Combine weighted sub-scores into one number.
    ///
    /// The weights live in the caller's config, never in the prompt: the model
    /// scores each factor in isolation and code does the arithmetic. Returns the
    /// weighted mean, or `None` when the weights are empty or sum to zero (there
    /// is no meaningful score in that case).
    #[must_use]
    pub fn weighted_score(pairs: &[(f64, f64)]) -> Option<f64> {
        let total_weight: f64 = pairs.iter().map(|(w, _)| *w).sum();
        if total_weight.abs() < f64::EPSILON {
            return None;
        }
        // `mul_add` is both more accurate and fewer instructions; clippy is
        // right to want it here.
        let acc = pairs.iter().fold(0.0_f64, |a, (w, s)| w.mul_add(*s, a));
        Some(acc / total_weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the band boundaries ------------------------------------------------

    #[test]
    fn bands_split_at_the_configured_thresholds() {
        let p = Policy::default(); // low 0.5, high 0.9
        assert_eq!(p.band(0.0), Band::Low);
        assert_eq!(p.band(0.49), Band::Low);
        // EXACTLY the low threshold is Mid, not Low: `< low` is Low, so the
        // boundary belongs to the band above it. Pinned so a refactor cannot
        // silently move it.
        assert_eq!(p.band(0.5), Band::Mid);
        assert_eq!(p.band(0.89), Band::Mid);
        assert_eq!(p.band(0.9), Band::High);
        assert_eq!(p.band(1.0), Band::High);
    }

    #[test]
    fn band_names_are_route_ready() {
        assert_eq!(Band::Low.as_str(), "low");
        assert_eq!(Band::Mid.as_str(), "mid");
        assert_eq!(Band::High.as_str(), "high");
    }

    // ---- the video's measured cases ----------------------------------------

    #[test]
    fn the_measured_reversible_case_asks_a_human() {
        // The video: `rm -rf node_modules && npm install` -> "reversible" at
        // confidence 0.35 -> ASKS A HUMAN, because the pick is plausible and the
        // confidence is not. 0.35 is Low here, so it Denies rather than Asks —
        // either way it does NOT allow, which is the property that matters.
        let p = Policy::default();
        let d = p.gate(0.35, false, false);
        assert_ne!(d, GateDecision::Allow, "0.35 must never auto-allow");
    }

    #[test]
    fn mid_confidence_always_asks_never_denies() {
        // The control for the case above: a genuinely Mid confidence must ASK,
        // not DENY — refusing outright on a plausible answer would push callers
        // to bypass the gate.
        let p = Policy::default();
        assert_eq!(p.gate(0.7, false, false), GateDecision::Ask);
        assert_eq!(p.gate(0.7, true, false), GateDecision::Ask);
    }

    #[test]
    fn irreversible_needs_high_confidence() {
        let p = Policy::default();
        assert_eq!(p.gate(0.95, true, false), GateDecision::Allow);
        // The same confidence on a reversible action is also Allow — the flag
        // only ever *tightens*, never loosens.
        assert_eq!(p.gate(0.95, false, false), GateDecision::Allow);
        assert_eq!(p.gate(0.7, true, false), GateDecision::Ask);
    }

    #[test]
    fn irreversible_never_loosens_at_any_confidence() {
        // `irreversible` cannot change the outcome (only High allows, either way), so the
        // two flags must agree everywhere — and the outcome must be exactly the band's.
        let p = Policy::default();
        for i in 0..=100_u32 {
            let c = f64::from(i) / 100.0;
            let expected = match p.band(c) {
                Band::High => GateDecision::Allow,
                Band::Mid => GateDecision::Ask,
                Band::Low => GateDecision::Deny,
            };
            assert_eq!(p.gate(c, true, false), expected, "irreversible, c={c}");
            assert_eq!(p.gate(c, false, false), expected, "reversible, c={c}");
        }
    }

    #[test]
    fn weighted_score_divides_by_a_non_unit_total() {
        // Weights sum to 2, not 1: `/ 2` and `* 2` differ, as do `/` and `%`.
        assert_eq!(Policy::weighted_score(&[(2.0, 1.0), (2.0, 0.0)]), Some(0.5));
        assert_eq!(Policy::weighted_score(&[(1.0, 3.0), (3.0, 7.0)]), Some(6.0));
        // CONTROL: empty and zero-sum weights still have no score.
        assert_eq!(Policy::weighted_score(&[]), None);
        assert_eq!(Policy::weighted_score(&[(0.0, 1.0)]), None);
        // A total of exactly EPSILON is not "zero": it is the boundary, and it scores.
        assert_eq!(Policy::weighted_score(&[(f64::EPSILON, 1.0)]), Some(1.0));
    }

    /// Seeded LCG (Knuth MMIX constants). A failure reproduces from the printed seed.
    struct Lcg(u64);
    impl Lcg {
        #[allow(clippy::cast_precision_loss)]
        fn unit(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64) / ((1_u64 << 53) as f64)
        }
    }

    const fn rank(b: Band) -> u8 {
        match b {
            Band::Low => 0,
            Band::Mid => 1,
            Band::High => 2,
        }
    }

    #[test]
    fn band_is_monotone_in_confidence() {
        const SEED: u64 = 0x13B0_0001;
        let p = Policy::default();
        let mut rng = Lcg(SEED);
        let mut distinct = std::collections::HashSet::new();
        for case in 0..512_u32 {
            let (a, b) = (rng.unit(), rng.unit());
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            for kind in [QuestionType::Choice, QuestionType::Noul] {
                let (bl, bh) = (p.band_for(kind, lo), p.band_for(kind, hi));
                assert!(
                    rank(bl) <= rank(bh),
                    "not monotone: seed {SEED:#x}, case {case}, {kind:?}: band({lo})={bl:?} > band({hi})={bh:?}"
                );
                distinct.insert(format!("{kind:?}/{bl:?}"));
            }
        }
        // CONTROL: every band was reached for both kinds, so "monotone" is not vacuous.
        assert_eq!(distinct.len(), 6, "{distinct:?}");
    }

    // ---- saturation: the flag we added from our own measurement -------------

    #[test]
    fn saturated_answers_ask_only_when_configured() {
        let mut p = Policy::default();
        assert_eq!(
            p.gate(0.99, false, true),
            GateDecision::Allow,
            "record-only by default"
        );
        p.escalate_on_saturation = true;
        assert_eq!(
            p.gate(0.99, false, true),
            GateDecision::Ask,
            "when configured, saturation escalates"
        );
    }

    // ---- noul ---------------------------------------------------------------

    #[test]
    fn noul_threshold_is_inclusive_and_in_code() {
        let p = Policy::default(); // 0.5
        assert!(p.noul_is_true(0.5));
        assert!(!p.noul_is_true(0.499));
        assert!(p.noul_is_true(0.99));
    }

    // ---- composite scoring --------------------------------------------------

    #[test]
    fn composite_score_applies_weights() {
        // The video's ticket example: severity .6, frustration .3, quality .1.
        let pairs = [(0.6, 0.9), (0.3, 1.0), (0.1, 0.2)];
        let got = Policy::weighted_score(&pairs);
        let want = 0.6_f64.mul_add(0.9, 0.3_f64.mul_add(1.0, 0.1 * 0.2));
        assert!((got.unwrap_or_default() - want).abs() < 1e-12);
    }

    #[test]
    fn composite_score_rejects_degenerate_weights() {
        // No weights, and weights summing to zero: both mean "no score", and
        // returning 0.0 would silently read as "definitely not important".
        assert!(Policy::weighted_score(&[]).is_none());
        assert!(Policy::weighted_score(&[(0.0, 1.0), (0.0, 0.5)]).is_none());
    }

    // ---- config -------------------------------------------------------------

    #[test]
    fn toml_round_trip_and_defaults() {
        let p = Policy::default();
        let text = toml::to_string(&p).unwrap_or_default();
        let back = Policy::from_toml(&text).unwrap_or_default();
        assert_eq!(back, p);
    }

    #[test]
    fn a_partial_toml_takes_defaults_for_the_rest() {
        let p = Policy::from_toml("high_threshold = 0.95").unwrap_or_default();
        assert!((p.high_threshold - 0.95).abs() < f64::EPSILON);
        assert!(
            (p.low_threshold - 0.5).abs() < f64::EPSILON,
            "unspecified keys keep defaults"
        );
    }

    #[test]
    fn inverted_thresholds_are_rejected() {
        let p = Policy {
            low_threshold: 0.9,
            high_threshold: 0.5,
            ..Policy::default()
        };
        assert!(matches!(
            p.validate(),
            Err(PolicyError::ThresholdsInverted {
                which: "choice/score",
                ..
            })
        ));
    }

    #[test]
    fn equal_thresholds_are_rejected() {
        // Equal values make Mid unreachable — a whole band silently disappears,
        // which is exactly the kind of config bug that must fail loudly.
        let p = Policy {
            low_threshold: 0.7,
            high_threshold: 0.7,
            ..Policy::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn out_of_range_thresholds_are_rejected() {
        let p = Policy {
            high_threshold: 1.5,
            ..Policy::default()
        };
        assert!(matches!(
            p.validate(),
            Err(PolicyError::ThresholdOutOfRange { .. })
        ));
        let p = Policy {
            noul_threshold: -0.1,
            ..Policy::default()
        };
        assert!(matches!(
            p.validate(),
            Err(PolicyError::NoulThresholdOutOfRange(_))
        ));
    }

    #[test]
    fn the_default_policy_is_valid() {
        assert!(Policy::default().validate().is_ok());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // A typo'd key must not be silently ignored: `hihg_threshold = 0.99`
        // would otherwise leave the gate at the default and nobody would notice.
        assert!(Policy::from_toml("hihg_threshold = 0.99").is_err());
    }
    // ---- per-type thresholds (design-review finding m4) ---------------------
    //
    // A noul reports max(p, 1-p) and a choice/score reports 1 - H(p)/log(k).
    // Those are different quantities, so they get different boundaries — these
    // tests pin that they are read from the RIGHT pair, which is the bug the
    // review found (one global threshold made "0.6 confident" mean two things).

    #[test]
    fn a_noul_uses_its_own_thresholds() {
        let p = Policy {
            low_threshold: 0.5,
            high_threshold: 0.9,
            noul_low_threshold: 0.7,
            noul_high_threshold: 0.99,
            ..Policy::default()
        };
        // 0.6 is Mid for a choice/score (>= 0.5, < 0.9) but LOW for a noul
        // (< 0.7). Same number, different answer: the whole point.
        assert_eq!(p.band_for(QuestionType::Choice, 0.6), Band::Mid);
        assert_eq!(p.band_for(QuestionType::Noul, 0.6), Band::Low);
        assert_eq!(p.band_for(QuestionType::Noul, 0.995), Band::High);
        assert_eq!(p.band_for(QuestionType::Score, 0.6), Band::Mid);
    }

    #[test]
    fn band_and_band_for_agree_for_choice_and_score() {
        // `band` is the convenience form for the choice/score pair; if the two
        // ever disagree, one of them is using the wrong thresholds.
        let p = Policy::default();
        for c in [0.0, 0.3, 0.5, 0.7, 0.9, 0.95, 1.0] {
            assert_eq!(p.band(c), p.band_for(QuestionType::Choice, c), "c={c}");
            assert_eq!(p.band(c), p.band_for(QuestionType::Score, c), "c={c}");
        }
    }

    #[test]
    fn an_inverted_noul_pair_is_rejected() {
        // The same silent-hole bug as the choice/score pair, one type over: with
        // low >= high the Mid band is unreachable for every noul.
        let p = Policy {
            noul_low_threshold: 0.9,
            noul_high_threshold: 0.5,
            ..Policy::default()
        };
        assert!(matches!(
            p.validate(),
            Err(PolicyError::ThresholdsInverted { which: "noul", .. })
        ));
    }

    #[test]
    fn equal_noul_thresholds_are_rejected() {
        let p = Policy {
            noul_low_threshold: 0.8,
            noul_high_threshold: 0.8,
            ..Policy::default()
        };
        assert!(matches!(
            p.validate(),
            Err(PolicyError::ThresholdsInverted { which: "noul", .. })
        ));
    }

    #[test]
    fn out_of_range_noul_band_thresholds_are_rejected() {
        for p in [
            Policy {
                noul_high_threshold: 2.0,
                ..Policy::default()
            },
            Policy {
                noul_low_threshold: -0.5,
                ..Policy::default()
            },
        ] {
            assert!(matches!(
                p.validate(),
                Err(PolicyError::ThresholdOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn per_type_thresholds_survive_a_toml_round_trip() {
        // The per-type boundaries must be configurable, not just constructible —
        // otherwise a deployment cannot tune them.
        let p = Policy::from_toml("noul_low_threshold = 0.7\nnoul_high_threshold = 0.99")
            .unwrap_or_default();
        assert!((p.noul_low_threshold - 0.7).abs() < f64::EPSILON);
        assert!((p.noul_high_threshold - 0.99).abs() < f64::EPSILON);
        assert!(
            (p.low_threshold - 0.5).abs() < f64::EPSILON,
            "choice pair keeps defaults"
        );
    }

    // ---- gate_allow (spec 08) -----------------------------------------------

    #[test]
    fn the_default_allowlist_is_the_small_read_only_set() {
        let p = Policy::default();
        for entry in [
            "ls",
            "pwd",
            "cat",
            "head",
            "tail",
            "wc",
            "git status",
            "git diff",
            "git log",
            "git show",
            "cargo check",
            "cargo test",
            "cargo clippy",
        ] {
            assert!(
                p.gate_allow.iter().any(|e| e == entry),
                "`{entry}` must be in the default allowlist"
            );
        }
        assert_eq!(p.gate_allow.len(), 13, "no more, no fewer than specified");
    }

    #[test]
    fn an_empty_allowlist_is_valid_and_is_not_replaced_by_the_default() {
        // The control: a caller who deliberately locks the gate down must stay locked
        // down. A fallback to the default here would make "never allow" unreachable.
        let p = Policy {
            gate_allow: Vec::new(),
            ..Policy::default()
        };
        assert!(p.gate_allow.is_empty());
        assert!(p.validate().is_ok(), "an empty allowlist is a valid policy");
    }

    #[test]
    fn the_default_ask_deadline_is_sixty_seconds() {
        assert_eq!(Policy::default().ask_deadline_secs, 60);
    }

    #[test]
    fn gate_allow_survives_a_toml_round_trip() {
        let p = Policy::from_toml("gate_allow = [\"ls\", \"pwd\"]\nask_deadline_secs = 5")
            .unwrap_or_default();
        assert_eq!(p.gate_allow, vec!["ls".to_string(), "pwd".to_string()]);
        assert_eq!(p.ask_deadline_secs, 5);
    }

    // ---- gate_deny (spec 29) -----------------------------------------------

    #[test]
    fn default_policy_denies_nothing() {
        let p = Policy::default();
        assert!(
            p.gate_deny.is_empty(),
            "F5: the default policy ships with an EMPTY deny list — deny nothing"
        );
        assert!(
            p.validate().is_ok(),
            "an empty deny list must pass validation (a config that ships broken is worse \
             than nothing)"
        );
    }

    #[test]
    fn an_empty_deny_list_is_valid_and_is_not_replaced() {
        // The control: a deployment that deliberately sets `gate_deny = []` must not
        // silently regain a default deny set. Empty stays empty, whatever the default is.
        let p = Policy {
            gate_deny: Vec::new(),
            ..Policy::default()
        };
        assert_eq!(p.gate_deny.len(), 0);
        assert!(p.validate().is_ok(), "an empty deny list is a valid policy");
    }

    #[test]
    fn empty_vec_valid_and_toml_roundtrip() {
        // Both directions BOTH ways: an empty `gate_deny` survives a TOML write-back
        // (and does not appear as a surprise key), and a populated one round-trips
        // byte-for-byte. (policy.rs:760-765 is the round-trip precedent.)
        let p = Policy::from_toml("gate_deny = []").unwrap_or_default();
        assert_eq!(p.gate_deny, Vec::<String>::new());
        let text = toml::to_string(&p).unwrap_or_default();
        assert_eq!(p, Policy::from_toml(&text).unwrap_or_default());

        let p = Policy::from_toml("gate_deny = [\"rm -rf\", \"of=/dev/\"]").unwrap_or_default();
        assert_eq!(
            p.gate_deny,
            vec!["rm -rf".to_string(), "of=/dev/".to_string()]
        );
        let text = toml::to_string(&p).unwrap_or_default();
        let back = Policy::from_toml(&text).unwrap_or_default();
        assert_eq!(back, p, "a populated deny list round-trips");
    }

    #[test]
    fn gate_deny_rejects_an_empty_pattern_entry() {
        // Same bar as the allowlist's entries (F1): a pattern with no content matches
        // everything, so a config that contains one is a configuration error that must
        // be caught at load, not observed at judge time.
        let p = Policy {
            gate_deny: vec![String::new()],
            ..Policy::default()
        };
        assert!(p.validate().is_err(), "an empty pattern entry is rejected");
    }
}
