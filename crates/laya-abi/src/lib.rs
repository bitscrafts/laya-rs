//! The three System-1 question types, exactly as Laya defines them.
//!
//! Laya's API is `state` + typed `questions` → typed `answers`, in one parallel
//! pass, with **zero text generation**. Everything downstream in this workspace
//! depends only on these three shapes, which is what lets a caller swap Laya for
//! Jev (or a mock) without touching call sites.

use serde::{Deserialize, Serialize};

/// One of Laya's three question types.
///
/// The type is part of the question's identity: it decides how the head's
/// probability vector is read, and — see `laya_core::policy` — which temperature
/// bucket applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    /// Yes/no, returned as `p` (the probability of yes, 0.0..=1.0).
    ///
    /// The number *is* the belief — there is no separate confidence field. Use for
    /// gates and flags: the reference design's "smart if". **The threshold lives in the
    /// caller, never here.**
    Noul,
    /// Pick exactly one of `options` (Laya allows up to 255).
    ///
    /// Returns the chosen option, the probability of *every* option, and a
    /// confidence. Add an `other` option whenever the list may not cover every
    /// input — that is the model's declared exit.
    Choice,
    /// Place the state on an ordinal rubric of 2..=10 described levels.
    ///
    /// The score **may land between levels**, which is the point: it carries
    /// more information than a bucket. Weights stay in the caller.
    Score,
}

impl QuestionType {
    /// The lowercase wire name Laya uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }

    /// The short tag Laya's `temperature_by_options` keys are built from
    /// (verified: `choice:2`, `choice:3-5`, `score:3-5`, `noul:2`, …).
    #[must_use]
    pub const fn temp_tag(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }
}

/// A question about a state.
///
/// `id` is **caller-chosen** and is echoed back on the answer — it is the join
/// key between a question and its answer, and the reference design's rule is to use it to
/// *fan out*: ask every question you might need in one call, then decide in code
/// which answers to actually use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// Caller-chosen identifier, echoed back on [`Answer`].
    pub id: String,
    /// Which of the three question types this is.
    #[serde(rename = "type")]
    pub kind: QuestionType,
    /// The question text, or the rubric for [`QuestionType::Score`].
    pub text: String,
    /// The declared options. **Required** for [`QuestionType::Choice`], omitted
    /// for the other two (Laya allows up to 255).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Extra criteria, mainly for [`QuestionType::Noul`] (what makes it true /
    /// false). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<String>,
}

impl Question {
    /// A `noul` question: `p` is the probability that `text` is true.
    #[must_use]
    pub fn noul(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: QuestionType::Noul,
            text: text.into(),
            options: Vec::new(),
            criteria: None,
        }
    }

    /// A `choice` question over `options`. Must be non-empty.
    #[must_use]
    pub fn choice<I, S>(id: impl Into<String>, text: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            id: id.into(),
            kind: QuestionType::Choice,
            text: text.into(),
            options: options.into_iter().map(Into::into).collect(),
            criteria: None,
        }
    }

    /// A `score` question on a 2..=10 level rubric described by `text`.
    #[must_use]
    pub fn score(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: QuestionType::Score,
            text: text.into(),
            options: Vec::new(),
            criteria: None,
        }
    }

    /// Attach the optional criteria block.
    #[must_use]
    pub fn with_criteria(mut self, criteria: impl Into<String>) -> Self {
        self.criteria = Some(criteria.into());
        self
    }
}

/// A typed answer, with the probabilities that produced it.
///
/// The invariants the rest of the workspace relies on:
/// * `probabilities` is **parallel to** `options` for a choice, and to the rubric
///   levels for a score. Empty for a noul.
/// * `value` is always one of the caller's declared options (never an invented
///   label) — see `Answer::saturated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The id of the [`Question`] this answers.
    pub id: String,
    /// Which question type produced it.
    #[serde(rename = "type")]
    pub kind: QuestionType,
    /// The typed result: the chosen option, the level, or `"yes"`/`"no"`.
    pub value: String,
    /// For [`QuestionType::Noul`]: the probability that the answer is yes.
    /// `None` for the other types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    /// For a score: the numeric position on the rubric (may be fractional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Probability of each declared option (choice) or level (score).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probabilities: Vec<f64>,
    /// The options the probabilities correspond to — echoed back so a consumer
    /// never has to re-derive the index mapping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Laya's calibrated confidence in this answer, 0.0..=1.0.
    ///
    /// **This is the field to gate on.** The `act_probability` head is inert in
    /// the deployed checkpoint (measured: exactly 1.0 on everything, including
    /// gibberish), so it is deliberately not surfaced here at all.
    pub confidence: f64,
    /// Wall-clock milliseconds the decision took (backbone + head), measured by
    /// the caller — used by the ledger to keep the latency habit visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
}

impl Answer {
    /// For a noul, the belief as `p`; for the other types, the confidence.
    ///
    /// This is the "how sure is the model" number in both cases, which is what a
    /// gate wants. Use [`Self::confidence`] directly when you want only the
    /// calibrated probability of the *chosen* answer.
    #[must_use]
    pub fn belief(&self) -> f64 {
        self.p.unwrap_or(self.confidence)
    }

    /// True when one option carries effectively all the probability mass.
    ///
    /// We measured Laya returning exactly `1.000/0.000/0.000` on a real
    /// `choice`, which is the same degenerate-softmax smell as the inert
    /// `act_probability` head. Such an answer must **not** be read as "very
    /// confident": it is suspicious, and the ledger records it as such.
    ///
    /// A noul is never saturated — `p` is a single number, not a distribution.
    #[must_use]
    pub fn saturated(&self) -> bool {
        match self.kind {
            QuestionType::Noul => false,
            QuestionType::Choice | QuestionType::Score => {
                self.probabilities.len() > 1
                    && self.probabilities.iter().copied().fold(0.0_f64, f64::max)
                        > SATURATION_THRESHOLD
            }
        }
    }
}

/// A distribution where one option holds more than this counts as saturated.
///
/// Chosen at 0.999 rather than 1.0 so floating-point noise does not hide the
/// case; the observed saturation was exact, so this only ever fires on the real
/// thing.
pub const SATURATION_THRESHOLD: f64 = 0.999;

#[cfg(test)]
mod tests {
    use super::*;

    fn choice_answer(probs: Vec<f64>) -> Answer {
        Answer {
            id: "department".into(),
            kind: QuestionType::Choice,
            value: "billing".into(),
            p: None,
            score: None,
            probabilities: probs,
            options: vec!["billing".into(), "tech".into(), "other".into()],
            confidence: 0.85,
            latency_ms: None,
        }
    }

    #[test]
    fn question_type_wire_names_match_laya() {
        assert_eq!(QuestionType::Noul.as_str(), "noul");
        assert_eq!(QuestionType::Choice.as_str(), "choice");
        assert_eq!(QuestionType::Score.as_str(), "score");
    }

    #[test]
    fn constructors_set_the_right_kind_and_options() {
        let n = Question::noul("q1", "is this reversible?");
        assert_eq!(n.kind, QuestionType::Noul);
        assert!(n.options.is_empty());

        let c = Question::choice("q2", "which team?", ["billing", "tech"]);
        assert_eq!(c.kind, QuestionType::Choice);
        assert_eq!(c.options, vec!["billing", "tech"]);

        let s = Question::score("q3", "1=calm 2=minor 10=outage");
        assert_eq!(s.kind, QuestionType::Score);
        assert!(s.options.is_empty());
    }

    #[test]
    fn noul_is_never_saturated() {
        let a = Answer {
            id: "x".into(),
            kind: QuestionType::Noul,
            value: "yes".into(),
            p: Some(1.0),
            score: None,
            probabilities: Vec::new(),
            options: Vec::new(),
            confidence: 1.0,
            latency_ms: None,
        };
        assert!(
            !a.saturated(),
            "a noul carries one number, not a distribution"
        );
        assert!((a.belief() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_real_saturated_choice_is_flagged() {
        // Exactly what we measured from the deployed checkpoint.
        let a = choice_answer(vec![1.000, 0.000, 0.000]);
        assert!(
            a.saturated(),
            "1.000/0.000/0.000 must be flagged as suspect"
        );
    }

    #[test]
    fn a_normal_distribution_is_not_saturated() {
        // The near-miss CONTROL: a highly-but-honestly confident answer must NOT
        // be flagged, or the flag becomes noise and gets ignored.
        let a = choice_answer(vec![0.998, 0.001, 0.001]);
        assert!(!a.saturated(), "0.998 is confident, not degenerate");
    }

    #[test]
    fn a_single_option_distribution_is_not_saturated() {
        // Degenerate for a different reason (nothing to distribute over), so the
        // saturation flag must not fire — it means "one option took all the mass
        // of a real choice", not "we forgot to pass options".
        let a = choice_answer(vec![1.0]);
        assert!(!a.saturated());
    }

    #[test]
    fn belief_prefers_p_then_falls_back_to_confidence() {
        let mut a = choice_answer(vec![0.5, 0.3, 0.2]);
        assert!((a.belief() - 0.85).abs() < f64::EPSILON);
        a.p = Some(0.42);
        assert!((a.belief() - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn answers_round_trip_through_json() {
        let a = choice_answer(vec![0.7, 0.2, 0.1]);
        let s = serde_json::to_string(&a).unwrap_or_default();
        let back: Answer = serde_json::from_str(&s).unwrap_or_else(|_| choice_answer(vec![]));
        assert_eq!(back, a);
        assert!(
            s.contains("\"type\":\"choice\""),
            "wire form keeps `type`: {s}"
        );
    }
}
