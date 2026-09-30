//! From logits to an [`Answer`]: the post-processing the reference performs after the forward
//! pass, ported so the Rust path produces the *same answer object* the Python path did.
//!
//! ## Why this is a separate step from [`Head::forward`](crate::forward::Head::forward)
//!
//! The forward pass is a pure function of vectors. This step is where the **report-shaped**
//! decisions happen — which temperature applies, how confidence is derived, and what the typed
//! value is. Keeping them apart means the arithmetic can be checked against `PyTorch` on its own
//! (it is, by `tests/forward_differential.rs`) without these rules in the way.
//!
//! ## The reference, verbatim
//!
//! From `laya/agent.py::system_one` (lines 325-362), reproduced field by field:
//!
//! ```text
//! t_scale = temperature_by_options.get(temp_bucket(qt, k), temperature[qt])
//! z = logits[:k] / t_scale
//! p = exp(z - z.max()); p = p / p.sum()
//! confidence = confidence_from_probs(p, k)          # 1 - H(p)/log(k), clipped to [0,1]
//! choice: value = the option at argmax(p);  probabilities = p per option
//! score:  score = sum(arange(k) * p);       probabilities = p per level
//! noul:   p_yes = p[1]; confidence = max(p[1], 1 - p[1])
//! ```
//!
//! ## Two facts that decide correctness here
//!
//! **F10 — the label does not depend on temperature, and the clamp is why.** The reference
//! softmaxes with `t_scale` and *then* takes the argmax. Because softmax is monotonic and
//! `t_scale >= 0.5 > 0`, that argmax equals the argmax of the raw logits — so the reported label
//! is temperature-invariant and only the *probabilities* move. Every in-range temperature gives
//! the same answer; a temperature of zero or below would break it, and
//! [`clamp_temperature`](crate::clamp_temperature) is what prevents that. The two guarantees are
//! the same guarantee.
//!
//! **The clamp is per bucket, and a clamped bucket makes confidence uncalibrated.** One bucket in
//! the deployed checkpoint (`choice:11+ = 0.10058`) multiplies logits ~10x, which would publish a
//! 0.24 top probability as 0.99 and defeat the confidence gate the whole design rests on. So the
//! decision records that the temperature it used was clamped, and a consumer gating on confidence
//! has to treat that answer differently.
//!
//! ## What is deliberately NOT done here
//!
//! **The `noul` label is not decided here.** The reference returns the probability and the
//! *caller* applies its threshold; the threshold is validated, configurable, and lives in
//! `laya-core`'s `Policy`. [`decide_with`](Head::decide_with) therefore takes it as an argument
//! rather than defaulting to `0.5` in a second place.
//!
//! **The routing margin is not computed here either, and the two margins are named apart.** The
//! spec's F8 called the margin a "top1 - top2 logit gap", but the implemented
//! `DecisionPayload` (in `laya-core`) computes it in **probability space** — and that
//! one is what the near-tie band acts on. Rather than have two quantities both called "margin",
//! this module reports [`Decision::logit_gap`] (how far apart the model's scores were) and leaves
//! `margin` to the payload (how close the decision was to flipping). See the F8 note in
//! `specs/03-rust-head.md`.
//!
//! **And no band, no payload, and no threshold but the `noul` one.** Turning a [`Decision`] into a
//! route-shaped payload is `laya-core`'s job: it owns `Policy`, and this crate must not learn what
//! a band is — that split is what keeps the head checkable against `PyTorch` on its own.

use laya_abi::{Answer, Question, QuestionType};

use crate::clamp_temperature;
use crate::forward::{Head, HeadError};
use crate::safetensors::HeadConfig;

/// The temperature bucket key for a question type and option count, matching
/// `laya.common.temp_bucket`.
///
/// The bucket sizes are the reference's: `2`, `3-5`, `6-10`, `11+`. The `11+` bucket is the one
/// that exists in the deployed checkpoint and is out of range, so this function is on the path to
/// the single most dangerous constant in the port.
#[must_use]
pub fn temp_bucket(kind: QuestionType, options: usize) -> String {
    let size = match options {
        0..=2 => "2",
        3..=5 => "3-5",
        6..=10 => "6-10",
        _ => "11+",
    };
    format!("{}:{size}", kind.temp_tag())
}

/// The `qtype` index the head's `type_emb` and temperature array are indexed by.
///
/// **Measured, not inferred:** `laya.common.QTYPES` is `{'choice': 0, 'score': 1, 'noul': 2}`, and
/// the reference indexes `temperature[qt]` with exactly this. So the stored array
/// `[1.6369, 1.2514, 1.9834]` is `[choice, score, noul]`. A reader who assumed the array was in the
/// order the *names* happen to read would swap `choice` with `noul` — and those two temperatures
/// differ by 0.35, which is a different reported confidence for every answer.
#[must_use]
pub const fn qtype_index(kind: QuestionType) -> usize {
    match kind {
        QuestionType::Choice => 0,
        QuestionType::Score => 1,
        QuestionType::Noul => 2,
    }
}

/// Normalised Shannon-entropy confidence, matching `laya.common.confidence_from_probs`.
///
/// `1 - H(p) / log(k)`, clipped to `[0,1]`, and exactly `1.0` when `k < 2` (a single option cannot
/// be uncertain). The `1e-12` floor on `p` inside the log is the reference's, and it is what makes
/// a zero probability contribute `0 * log(1e-12) == 0` rather than a NaN.
#[must_use]
pub fn confidence_from_probs(probs: &[f64], options: usize) -> f64 {
    if options < 2 {
        return 1.0;
    }
    let entropy: f64 = probs
        .iter()
        .take(options)
        .map(|&p| -p * p.max(1e-12).ln())
        .sum();
    // `options >= 2`, so the log is positive and the division is defined.
    #[allow(clippy::cast_precision_loss)]
    let k = options as f64;
    (1.0 - entropy / k.ln()).clamp(0.0, 1.0)
}

/// Softmax with the reference's exact numerically-stable form: subtract the max, exponentiate,
/// divide by the sum.
///
/// A named function because subtracting the max is not cosmetic — without it the exponent
/// overflows for the ~3000-magnitude act logits this checkpoint produces.
#[must_use]
pub fn softmax(values: &[f64]) -> Vec<f64> {
    let mut out = values.to_vec();
    let Some(&peak) = out.iter().max_by(|a, b| a.total_cmp(b)) else {
        return out;
    };
    let mut total = 0.0_f64;
    for value in &mut out {
        *value = (*value - peak).exp();
        total += *value;
    }
    if total > 0.0 {
        for value in &mut out {
            *value /= total;
        }
    }
    out
}

/// The top two values of a slice, as `(top1, top2)`, with `top2` as `NEG_INFINITY` when there is
/// no second entry — the same convention the reference's `topk(2)` pads with.
#[must_use]
fn top_two(values: &[f64]) -> (f64, f64) {
    values.iter().fold(
        (f64::NEG_INFINITY, f64::NEG_INFINITY),
        |(first, second), &v| {
            if v > first {
                (v, first)
            } else if v > second {
                (first, v)
            } else {
                (first, second)
            }
        },
    )
}

/// The caller-side knobs [`Head::decide_with`] needs that are not the model.
///
/// A struct rather than positional scalars so the call site reads, and so a later knob (a
/// confidence floor, a rule about saturated answers) has a home that is not another argument.
#[derive(Debug, Clone, Copy)]
pub struct DecideOptions {
    /// The caller's `noul` threshold: the probability of "yes" at or above which the answer is
    /// `yes`.
    ///
    /// Passed in rather than defaulted, because the threshold is validated and configurable in
    /// `laya-core`'s `Policy` and was once hardcoded to `0.5` here while the config was read only
    /// by its own tests — so a config change had no effect on the label a caller got.
    pub noul_threshold: f64,
}

impl Default for DecideOptions {
    fn default() -> Self {
        Self {
            noul_threshold: 0.5,
        }
    }
}

/// A decision and the parts of it that the shared [`Answer`] shape cannot carry.
///
/// [`Answer`] is the ABI: it is what a Python path and a Rust path both must produce, so a field
/// only one of them can fill would be a lie in the type. Everything the Rust port knows *in
/// addition* lives here instead.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The ABI-shaped answer, identical in shape to what the Python path returned.
    pub answer: Answer,
    /// The temperature actually used, after clamping.
    pub temperature: f64,
    /// Whether the configured temperature was **changed** to get there, i.e. whether the
    /// confidence on this answer is uncalibrated (F4).
    pub bucket_clamped: bool,
    /// The bucket key the temperature came from, e.g. `choice:11+` — recorded so a reader can see
    /// *which* bucket was clamped rather than only that one was.
    pub bucket: String,
    /// The act head's softmaxed first logit.
    ///
    /// **Inert in the deployed checkpoint** (measured exactly `1.0` on everything, including
    /// gibberish), which is why it is not surfaced on [`Answer`]. Carried because it is what the
    /// reference reports as `action.act_probability`, so a parity check over the live corpus can
    /// compare it rather than skip a field.
    pub act_probability: f64,
    /// The gap between the top two **raw logits** over the markers.
    ///
    /// Deliberately not called `margin`: the payload's `margin` is a probability-space distance,
    /// and two quantities sharing one name is how a near-tie rule ends up reading the wrong one.
    pub logit_gap: f64,
}

impl Head {
    /// Run a question end to end and produce the same [`Answer`] the Python path produced.
    ///
    /// `hidden` is the backbone's `[seq, d]` output for the sequence `build_sequence` built;
    /// `marker_pos` and `marker_mask` come from the same builder. The number of markers is `k`,
    /// and it is the reference's `k` in every formula above.
    ///
    /// # Errors
    /// [`HeadError::NoMarkers`] when the question yielded no option markers at all, and any error
    /// [`Head::forward`] returns for a malformed hidden state.
    pub fn decide(
        &self,
        config: &HeadConfig,
        question: &Question,
        hidden: &[f32],
        marker_pos: &[usize],
        marker_mask: &[bool],
    ) -> Result<Decision, HeadError> {
        self.decide_with(
            config,
            question,
            hidden,
            marker_pos,
            marker_mask,
            DecideOptions::default(),
        )
    }

    /// [`Head::decide`] with the caller's threshold supplied.
    ///
    /// # Errors
    /// As [`Head::decide`].
    pub fn decide_with(
        &self,
        config: &HeadConfig,
        question: &Question,
        hidden: &[f32],
        marker_pos: &[usize],
        marker_mask: &[bool],
        opts: DecideOptions,
    ) -> Result<Decision, HeadError> {
        let qtype = qtype_index(question.kind);
        let (logits, act) = self.forward(hidden, qtype, marker_pos, marker_mask)?;

        // `k` is the marker count: the reference slices `logits[r, :k]` and uses `k` as the option
        // count in every formula. A question with no markers has no distribution to report, and
        // dividing by it would be a NaN rather than an error, so it is refused by name.
        let k = marker_pos.len();
        if k == 0 {
            return Err(HeadError::NoMarkers {
                question: question.id.clone(),
            });
        }
        let k_logits = logits
            .iter()
            .take(k)
            .map(|&l| f64::from(l))
            .collect::<Vec<_>>();

        // Which temperature applies: the per-bucket override when one exists, else the per-type
        // value — then clamped, exactly as the reference clamps its whole table at load.
        let bucket = temp_bucket(question.kind, k);
        let configured = config
            .temperature_by_options
            .get(&bucket)
            .copied()
            .or_else(|| config.temperature.get(qtype).copied())
            .unwrap_or(1.0);
        let temperature = clamp_temperature(configured);
        // A clamped temperature is what F4 is about: confidence from it is uncalibrated.
        let bucket_clamped = crate::temperature_was_clamped(configured);

        let probs = softmax(
            &k_logits
                .iter()
                .map(|&l| l / temperature)
                .collect::<Vec<_>>(),
        );

        let (top1, top2) = top_two(&k_logits);
        let logit_gap = if top2.is_finite() { top1 - top2 } else { top1 };

        let act_probability = softmax(&act.iter().map(|&v| f64::from(v)).collect::<Vec<_>>())
            .first()
            .copied()
            .unwrap_or(0.0);

        let confidence = confidence_from_probs(&probs, k);
        let answer = Self::report(question, &probs, confidence, opts);
        Ok(Decision {
            answer,
            temperature,
            bucket_clamped,
            bucket,
            act_probability,
            logit_gap,
        })
    }

    /// Turn the probability vector into the typed [`Answer`], per question type.
    ///
    /// Split out of [`Head::decide_with`] because it is a self-contained rule table over the same
    /// inputs, and because the three branches are what a reader wants to compare side by side —
    /// especially `score` (an expectation) against `noul` (a single probability plus an OVERRIDDEN
    /// confidence, `agent.py:360`).
    fn report(
        question: &Question,
        probs: &[f64],
        entropy_confidence: f64,
        opts: DecideOptions,
    ) -> Answer {
        #[allow(clippy::cast_precision_loss)]
        let (value, p, score, probabilities, options, confidence) = match question.kind {
            QuestionType::Choice => {
                // argmax over the probabilities. Equivalent to the argmax of the raw logits
                // because softmax is monotonic and `temperature > 0` — which is F10, and it is
                // the clamp that guarantees the second half of that.
                let chosen = probs
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.total_cmp(b))
                    .map_or(0, |(index, _)| index);
                let value = question
                    .options
                    .get(chosen)
                    .cloned()
                    .unwrap_or_else(|| chosen.to_string());
                (
                    value,
                    None,
                    None,
                    probs.to_vec(),
                    question.options.clone(),
                    entropy_confidence,
                )
            }
            QuestionType::Score => {
                // The expected level: `sum(arange(k) * p)`. A fractional score is the point — it
                // carries more information than the bucket would.
                #[allow(clippy::cast_precision_loss)]
                let expected: f64 = probs.iter().enumerate().map(|(i, &p)| i as f64 * p).sum();
                let value = format!("{expected:.4}");
                (
                    value,
                    None,
                    Some(expected),
                    probs.to_vec(),
                    Vec::new(),
                    entropy_confidence,
                )
            }
            QuestionType::Noul => {
                // Option 1 is "true": `render_options` returns `[false, true]` for a noul, so
                // `p[1]` is the probability that the statement holds, as the reference reads it.
                let p_yes = probs
                    .get(1)
                    .copied()
                    .unwrap_or_else(|| probs.first().copied().unwrap_or(0.0));
                let value = if p_yes >= opts.noul_threshold {
                    "yes"
                } else {
                    "no"
                };
                // The reference OVERRIDES the entropy confidence for a noul with
                // `max(p[1], 1 - p[1])`: a two-outcome belief has a direct confidence and does not
                // need the entropy proxy.
                (
                    value.to_string(),
                    Some(p_yes),
                    None,
                    Vec::new(),
                    Vec::new(),
                    p_yes.max(1.0 - p_yes),
                )
            }
        };

        Answer {
            id: question.id.clone(),
            kind: question.kind,
            value,
            p,
            score,
            probabilities,
            options,
            confidence,
            latency_ms: None,
        }
    }
}
