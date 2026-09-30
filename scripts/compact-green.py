#!/usr/bin/env python3
"""GREEN: implement `compact`, prepended above the test module."""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-core/src/compact.rs")
t = p.read_text()

impl = '''
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
                "notice = 0 makes `nothing` unreachable: every context is at least 0 \
                 tokens, so the agent could never be told to do nothing"
                    .to_string(),
            ));
        }
        if !(self.notice < self.recommend && self.recommend < self.request) {
            return Err(CompactError(format!(
                "thresholds must be strictly increasing (notice {} < recommend {} < \
                 request {}); equal or inverted values make a level unreachable",
                self.notice, self.recommend, self.request
            )));
        }
        Ok(())
    }
}

/// The advice, with the reasoning attached.
#[derive(Debug, Clone, PartialEq)]
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
                "the next step needs earlier work (needs_history {needs} of 2), so a \\
                 Request is dampened to Recommend"
            ));
            urgency = CompactUrgency::Recommend;
        }

        // Rule 1 again, as a veto rather than a rule — checked LAST so no earlier rule can
        // have raised the answer above it.
        if input.mid_edit {
            if urgency != CompactUrgency::Nothing {
                reasons.push(
                    "mid-edit: a compaction here would discard the working state the edit \\
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

'''

anchor = "#[cfg(test)]\nmod tests {"
assert anchor in t, "test anchor not found"
t = t.replace(anchor, impl.lstrip("\n") + anchor, 1)
p.write_text(t)
print("  implementation added above the tests")
