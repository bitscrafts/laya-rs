//! `should_compact` — L7: should the context be compacted now?
//!
//! ## The level, from the source research (measured lines, not paraphrase)
//!
//! Level 7 asks **four questions in one call**, after every turn:
//!
//! 1. did the request switch gears?
//! 2. did the last turn finish a unit of work?
//! 3. how much earlier work does the next step need?
//! 4. is the agent mid multi-step edit?
//!
//! ...and **the token thresholds stay in code**, never in the model. The demo's own
//! measured line is the shape of a correct answer:
//!
//! ```text
//! switched_gears yes 0.97 · at_boundary no 0.13 · needs_history 0.22 of 2
//!   -> request at 16k -> compact_now
//! ```
//!
//! The agent only ever hears one of four words: `nothing` / `notice` / `recommend` /
//! `request`. The model supplies the four judgements; **this module owns the arithmetic**.
//!
//! ## Why the thresholds are ours, not the model's
//!
//! The demo's thresholds (6k notice, 10k recommend, 14k request) belong to a 1M-window
//! model and a specific lab. They are defaults here, configurable, and documented as
//! inherited rather than derived — claiming a measured basis for them in our stack would
//! be exactly the kind of borrowed authority this project keeps removing.
//!
//! ## The safety property this module exists to enforce
//!
//! **A mid-edit turn must never be told to compact.** Question 4 exists for that. A
//! compaction mid-multi-step-edit discards the working state the edit depends on, which is
//! worse than a large context. So `mid_edit` is a **veto**, not a weighting: no combination
//! of the other three judgements may produce `compact_now` while it is true.

/// The four judgements L7 asks for, plus the token count the CALLER measured.
///
/// Every field is supplied by the caller. The three booleans and the 0..=2 score are the
/// model's answers; `context_tokens` is arithmetic and stays in code.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactInput {
    /// How full the context is. Counted by the caller — never estimated by a model.
    pub context_tokens: u64,
    /// Q1: did the request switch gears?
    pub switched_gears: bool,
    /// Q2: did the last turn finish a unit of work?
    pub at_boundary: bool,
    /// Q3: how much earlier work does the next step need? Scored `0..=2`.
    pub needs_history: f64,
    /// Q4: is the agent mid multi-step edit? **A veto — see [`CompactAdvice::decide`].**
    pub mid_edit: bool,
}

/// The four words the agent may hear. Fixed by the contract, not chosen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactUrgency {
    /// Say nothing.
    Nothing,
    /// Mention it; keep working.
    Notice,
    /// Recommend compacting at the next clean point.
    Recommend,
    /// Compact now.
    Request,
}

impl CompactUrgency {
    /// The wire string. The agent must recognise the word without a lookup table.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Notice => "notice",
            Self::Recommend => "recommend",
            Self::Request => "request",
        }
    }
}

/// Where the levels begin.
///
/// **Inherited defaults, not derived ones.** These are the values the source demo used
/// against a 1M-window model (6k notice, 10k recommend, 14k request). They are configurable
/// because the right numbers depend on the model's window, and they are labelled inherited
/// because claiming a measured basis for them in our stack would be false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactThresholds {
    /// At or above this, `notice`.
    pub notice: u64,
    /// At or above this, `recommend`.
    pub recommend: u64,
    /// At or above this, `request`.
    pub request: u64,
}

impl Default for CompactThresholds {
    fn default() -> Self {
        Self {
            notice: 6_000,
            recommend: 10_000,
            request: 14_000,
        }
    }
}

/// Why the thresholds are unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactError(pub String);

impl std::fmt::Display for CompactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CompactError {}

impl CompactThresholds {
    /// Check the thresholds can actually be reached.
    ///
    /// Strictly increasing is required. Equal or inverted thresholds make a level
    /// unreachable — the same silent hole the band thresholds in `Policy` are validated
    /// against. A `notice` of 0 is rejected for the same reason: every context is at least
    /// 0, so `Nothing` would never be returned and the agent would lose its quiet case.
    ///
    /// # Errors
    /// [`CompactError`] when the thresholds are not strictly increasing from a positive
    /// `notice`.
    pub fn validate(&self) -> Result<(), CompactError> {
        if self.notice == 0 {
            return Err(CompactError(
                "notice = 0 makes `nothing` unreachable: every context is at least 0                  tokens, so the agent could never be told to do nothing"
                    .to_string(),
            ));
        }
        if !(self.notice < self.recommend && self.recommend < self.request) {
            return Err(CompactError(format!(
                "thresholds must be strictly increasing (notice {} < recommend {} <                  request {}); equal or inverted values make a level unreachable",
                self.notice, self.recommend, self.request
            )));
        }
        Ok(())
    }
}

/// The advice, with the reasoning attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactAdvice {
    /// The word the agent hears.
    pub urgency: CompactUrgency,
    /// Whether to compact now. Only ever true for [`CompactUrgency::Request`].
    pub compact_now: bool,
    /// Every judgement that moved the answer, so a caller never has to guess why.
    pub reasons: Vec<String>,
}

impl CompactAdvice {
    /// Turn the four judgements plus the token count into advice.
    ///
    /// Three rules, in order:
    ///
    /// 1. **`mid_edit` is a VETO.** A compaction mid-multi-step-edit discards the working
    ///    state the edit depends on, which is worse than a large context. So it is checked
    ///    first and it caps the answer at `Recommend`, no matter how full the context is.
    /// 2. **`switched_gears` AND `at_boundary` together escalate one level.** The ideal
    ///    compaction point is "that work is finished AND the next thing is different".
    ///    Either alone is not enough: a gear switch mid-unit-of-work is not a clean cut.
    /// 3. **`needs_history` dampens a `Request`.** If the next step needs the earlier work,
    ///    this is the wrong moment to throw that work away.
    #[must_use]
    pub fn decide(input: &CompactInput, thresholds: &CompactThresholds) -> Self {
        let mut reasons = Vec::new();

        // Rule 1 — the token level.
        let mut urgency = if input.context_tokens >= thresholds.request {
            CompactUrgency::Request
        } else if input.context_tokens >= thresholds.recommend {
            CompactUrgency::Recommend
        } else if input.context_tokens >= thresholds.notice {
            CompactUrgency::Notice
        } else {
            CompactUrgency::Nothing
        };
        reasons.push(format!(
            "context {} tokens -> {} (notice {} / recommend {} / request {})",
            input.context_tokens,
            urgency.as_str(),
            thresholds.notice,
            thresholds.recommend,
            thresholds.request
        ));

        // Rule 2 — a clean gear change escalates one level.
        if input.switched_gears && input.at_boundary {
            let escalated = match urgency {
                CompactUrgency::Nothing => CompactUrgency::Notice,
                CompactUrgency::Notice => CompactUrgency::Recommend,
                CompactUrgency::Recommend | CompactUrgency::Request => CompactUrgency::Request,
            };
            if escalated != urgency {
                reasons.push(format!(
                    "gear switch at a work boundary: {} -> {}",
                    urgency.as_str(),
                    escalated.as_str()
                ));
                urgency = escalated;
            }
        }

        // Rule 3 — a next step that needs the history dampens a Request.
        //
        // Clamped to the declared 0..=2 range first: without that, a caller passing 99 could
        // move the decision, and the judgement's scale would quietly become the caller's.
        let needs = input.needs_history.clamp(0.0, 2.0);
        if needs > 1.0 && urgency == CompactUrgency::Request {
            reasons.push(format!(
                "the next step needs earlier work (needs_history {needs} of 2), so a \
                 Request is dampened to Recommend"
            ));
            urgency = CompactUrgency::Recommend;
        }

        // Rule 1 again, as a veto rather than a rule — checked LAST so no earlier rule can
        // have raised the answer above it.
        if input.mid_edit {
            if urgency != CompactUrgency::Nothing {
                reasons.push(
                    "mid-edit: a compaction here would discard the working state the edit \
                     depends on, so the advice is capped"
                        .to_string(),
                );
            }
            // Cap at Notice, not Recommend: mid-edit is the one state where a recommendation
            // to stop and compact is also wrong. The agent should finish the edit first and
            // be revisited, so it is told only that the context is filling.
            if urgency == CompactUrgency::Request || urgency == CompactUrgency::Recommend {
                urgency = CompactUrgency::Notice;
            }
        }

        Self {
            urgency,
            compact_now: urgency == CompactUrgency::Request,
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;

    fn input(context_tokens: u64) -> CompactInput {
        CompactInput {
            context_tokens,
            switched_gears: false,
            at_boundary: false,
            needs_history: 0.0,
            mid_edit: false,
        }
    }

    // ---- the four questions, one at a time --------------------------------

    #[test]
    fn a_low_context_asks_for_nothing() {
        let advice = CompactAdvice::decide(&input(1_000), &CompactThresholds::default());
        assert_eq!(advice.urgency, CompactUrgency::Nothing);
        assert!(!advice.compact_now);
    }

    #[test]
    fn the_urgency_climbs_with_context_in_the_thresholds_order() {
        let t = CompactThresholds::default(); // notice 6k, recommend 10k, request 14k
        for (tokens, want) in [
            (5_999, CompactUrgency::Nothing),
            (6_000, CompactUrgency::Notice),
            (9_999, CompactUrgency::Notice),
            (10_000, CompactUrgency::Recommend),
            (13_999, CompactUrgency::Recommend),
            (14_000, CompactUrgency::Request),
            (100_000, CompactUrgency::Request),
        ] {
            assert_eq!(
                CompactAdvice::decide(&input(tokens), &t).urgency,
                want,
                "at {tokens} tokens"
            );
        }
    }

    // ---- question 1 + 2 escalate together, and only together --------------

    #[test]
    fn a_gear_switch_at_a_clean_boundary_escalates_one_level() {
        // The ideal compaction point: the work just finished AND the next thing is
        // different. Either alone is not enough — this is the pair the demo measures
        // (`switched_gears yes 0.97` with `at_boundary`).
        let t = CompactThresholds::default();
        let mut i = input(6_000); // Notice
        i.switched_gears = true;
        i.at_boundary = true;
        assert_eq!(
            CompactAdvice::decide(&i, &t).urgency,
            CompactUrgency::Recommend
        );
    }

    #[test]
    fn a_gear_switch_without_a_boundary_does_not_escalate() {
        // THE CONTROL for the rule above. Without it, an implementation that escalated on
        // `switched_gears` alone would pass while compacting mid-unit-of-work.
        let t = CompactThresholds::default();
        let mut i = input(6_000);
        i.switched_gears = true;
        i.at_boundary = false;
        assert_eq!(
            CompactAdvice::decide(&i, &t).urgency,
            CompactUrgency::Notice
        );
    }

    #[test]
    fn a_boundary_without_a_gear_switch_does_not_escalate() {
        // The other half of the control.
        let t = CompactThresholds::default();
        let mut i = input(6_000);
        i.at_boundary = true;
        assert_eq!(
            CompactAdvice::decide(&i, &t).urgency,
            CompactUrgency::Notice
        );
    }

    // ---- question 4: the veto ---------------------------------------------

    #[test]
    fn a_mid_edit_turn_is_never_told_to_compact() {
        // THE safety property. Compacting mid-multi-step-edit discards the working state the
        // edit depends on — worse than a large context.
        let t = CompactThresholds::default();
        let mut i = input(500_000); // far past every threshold
        i.mid_edit = true;
        i.switched_gears = true;
        i.at_boundary = true;
        let advice = CompactAdvice::decide(&i, &t);
        assert!(
            !advice.compact_now,
            "a mid-edit turn must never be told to compact: {advice:?}"
        );
        assert!(
            advice
                .reasons
                .iter()
                .any(|r| r.contains("mid-edit") || r.contains("mid edit")),
            "the reason must name the veto so the caller knows why: {:?}",
            advice.reasons
        );
    }

    #[test]
    fn the_mid_edit_veto_holds_across_every_other_combination() {
        // An invariant, swept. A single case could pass by luck; this cannot.
        let t = CompactThresholds::default();
        for tokens in [0, 6_000, 14_000, 1_000_000] {
            for gears in [false, true] {
                for boundary in [false, true] {
                    for needs in [0.0, 1.0, 2.0] {
                        let i = CompactInput {
                            context_tokens: tokens,
                            switched_gears: gears,
                            at_boundary: boundary,
                            needs_history: needs,
                            mid_edit: true,
                        };
                        assert!(
                            !CompactAdvice::decide(&i, &t).compact_now,
                            "veto broke at {i:?}"
                        );
                    }
                }
            }
        }
    }

    // ---- question 3: how much earlier work the next step needs ------------

    #[test]
    fn a_next_step_that_needs_lots_of_history_dampens_the_urgency() {
        // `needs_history` is scored 0..=2 in the demo ("0.22 of 2"). A next step that needs
        // the earlier work is the WRONG moment to throw that work away.
        let t = CompactThresholds::default();
        let mut i = input(14_000); // Request on context alone
        i.needs_history = 2.0;
        let advice = CompactAdvice::decide(&i, &t);
        assert_ne!(
            advice.urgency,
            CompactUrgency::Request,
            "a next step needing full history must not be answered with Request"
        );
        assert!(
            advice.reasons.iter().any(|r| r.contains("history")),
            "the reason must name it: {:?}",
            advice.reasons
        );
    }

    #[test]
    fn a_next_step_needing_little_history_does_not_dampen() {
        // The CONTROL: without it, dampening unconditionally would pass the test above
        // while making `request` unreachable.
        let t = CompactThresholds::default();
        let mut i = input(14_000);
        i.needs_history = 0.2; // the demo's own measured value
        assert_eq!(
            CompactAdvice::decide(&i, &t).urgency,
            CompactUrgency::Request,
            "0.22 of 2 must not block a request"
        );
    }

    #[test]
    fn needs_history_is_clamped_to_its_declared_range() {
        // A caller passing 99 must not be able to force a different decision than 2.0 —
        // otherwise the judgement's scale silently becomes the caller's.
        let t = CompactThresholds::default();
        let mut high = input(14_000);
        high.needs_history = 99.0;
        let mut max = input(14_000);
        max.needs_history = 2.0;
        assert_eq!(
            CompactAdvice::decide(&high, &t).urgency,
            CompactAdvice::decide(&max, &t).urgency
        );
    }

    // ---- compact_now ------------------------------------------------------

    #[test]
    fn compact_now_is_true_exactly_when_the_urgency_is_request() {
        // The invariant, rather than a size threshold: `compact_now` is a CONSEQUENCE of the
        // urgency, never an independent decision. Pinned so a caller can rely on either.
        let t = CompactThresholds::default();
        for tokens in [0, 6_000, 9_999, 13_999, 14_000, 500_000] {
            for gears in [false, true] {
                for boundary in [false, true] {
                    let i = CompactInput {
                        context_tokens: tokens,
                        switched_gears: gears,
                        at_boundary: boundary,
                        needs_history: 0.0,
                        mid_edit: false,
                    };
                    let a = CompactAdvice::decide(&i, &t);
                    assert_eq!(
                        a.compact_now,
                        a.urgency == CompactUrgency::Request,
                        "compact_now must track Request exactly: {i:?} -> {a:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn one_gear_switch_escalates_by_at_most_one_level() {
        // CORRECTED. An earlier version of this test asserted `compact_now` was impossible
        // below the request threshold even with a gear switch — and it failed at 13,999
        // tokens, because `Recommend` + a clean boundary DOES escalate to `Request`. That
        // escalation is the intended rule (a perfect compaction point just under the
        // threshold is the ideal moment), so the TEST was wrong, not the code.
        //
        // What is actually worth pinning is the bound: escalation moves one level, never two,
        // so `Nothing` and `Notice` can never jump straight to `Request`.
        let t = CompactThresholds::default();
        for (tokens, base) in [
            (0u64, CompactUrgency::Nothing),
            (6_000, CompactUrgency::Notice),
        ] {
            let mut i = input(tokens);
            i.switched_gears = true;
            i.at_boundary = true;
            let a = CompactAdvice::decide(&i, &t);
            assert_ne!(
                a.urgency,
                CompactUrgency::Request,
                "a single gear switch must not jump two levels from {base:?}"
            );
            assert!(!a.compact_now, "and therefore must not compact_now: {i:?}");
        }
        // At request with no veto: yes.
        assert!(CompactAdvice::decide(&input(14_000), &t).compact_now);
    }

    #[test]
    fn the_urgency_vocabulary_is_exactly_the_four_the_agent_hears() {
        // The agent must recognise the word without a lookup table, so the strings are
        // fixed by the contract and pinned here.
        assert_eq!(CompactUrgency::Nothing.as_str(), "nothing");
        assert_eq!(CompactUrgency::Notice.as_str(), "notice");
        assert_eq!(CompactUrgency::Recommend.as_str(), "recommend");
        assert_eq!(CompactUrgency::Request.as_str(), "request");
    }

    // ---- thresholds are validated, not trusted ----------------------------

    #[test]
    fn unordered_thresholds_are_rejected() {
        // Equal or inverted thresholds make a level unreachable — a silent hole, the same
        // class as the band thresholds in `Policy`.
        for t in [
            CompactThresholds {
                notice: 10_000,
                recommend: 6_000,
                request: 14_000,
            },
            CompactThresholds {
                notice: 6_000,
                recommend: 6_000,
                request: 14_000,
            },
            CompactThresholds {
                notice: 6_000,
                recommend: 10_000,
                request: 10_000,
            },
        ] {
            assert!(
                t.validate().is_err(),
                "unordered thresholds must be rejected: {t:?}"
            );
        }
    }

    #[test]
    fn the_default_thresholds_are_valid_and_ordered() {
        assert!(CompactThresholds::default().validate().is_ok());
    }

    #[test]
    fn a_zero_notice_threshold_would_make_nothing_unreachable_and_is_rejected() {
        // If notice is 0, every context is at least notice, so the agent can never be told
        // "nothing" — the word becomes dead and the caller loses the quiet case.
        let t = CompactThresholds {
            notice: 0,
            recommend: 10_000,
            request: 14_000,
        };
        assert!(t.validate().is_err(), "{t:?}");
    }
}
