//! The end-to-end path: text in, a decision out — the join the whole port exists for.
//!
//! Everything before this module is a component with its own oracle: `tokenizer` reproduces the
//! reference exactly (91 strings / 2207 ids, 707 sequences / 83411 ids), `forward` reproduces
//! `PyTorch` (worst logit difference 4.05e-6), and `decide` reproduces the reference's
//! post-processing (worst difference 5.55e-17). This module is what turns those into the claim
//! that matters: **the whole path, from text, produces the same decision**.
//!
//! ## Why the embedder is a trait and not an HTTP call
//!
//! The head needs the backbone's `[seq, d]` output, and the backbone is llama.cpp over HTTP. Putting
//! `ureq` in this crate would mean the crate that must stay `#![forbid(unsafe_code)]` and
//! dependency-light grows a network stack to call one endpoint, and its tests would need a live
//! server to compile a decision.
//!
//! So the transport is a trait. [`Embedder`] is the seam:
//!
//! * the **live** implementation is in `laya-cli` (which already depends on `ureq`);
//! * the **tests** use a stub, so the join itself — sequence building, the embedding call, the
//!   2-D shape check, the decision — is testable with no network;
//! * and a caller can implement it over anything else (a batch endpoint, a cache, a recording).
//!
//! ## The shape contract, which is where a silent bug would live
//!
//! The backbone returns the embedding of **one sequence**, flat. The head needs `[seq, d]`. If the
//! row stride were wrong, the head would read plausible-looking rows of the wrong length and the
//! attention would produce a confident, meaningless answer — nothing would panic. So
//! [`Head::run`] derives `seq` from the token count and refuses an embedding whose length is not
//! exactly `tokens * d`, naming both numbers.
//!
//! It also refuses to silently accept a **short** embedding. llama.cpp truncates at its own
//! `max_len`, and if it truncated, the answer would be built from a hidden state covering only the
//! first part of the sequence while the marker positions still refer to the whole thing. That is a
//! wrong answer that computes cleanly, so it is an error rather than a warning.

use serde_json::Value;

use laya_abi::Question;

use crate::decide::{DecideOptions, Decision};
use crate::forward::{Head, HeadError};
use crate::safetensors::HeadConfig;
use crate::tokenizer::{Budget, Tokenizer, TokenizerError};

/// The backbone, reduced to the one operation the head needs.
///
/// Implemented by `laya-cli` over llama.cpp's `/embedding`. Kept to a single method so a caller (and
/// a test) has one thing to provide.
pub trait Embedder {
    /// Embed one token sequence and return its `[seq, d]` hidden states, row-major and flat.
    ///
    /// The length must be `tokens.len() * d`; [`Head::run`] checks that and refuses anything else
    /// rather than reshaping it optimistically.
    ///
    /// # Errors
    /// Free-form, because the transport's failure modes (connection refused, HTTP status, a
    /// malformed body) belong to the transport and not to this crate. The message is what a caller
    /// sees, so it should name the endpoint and what went wrong.
    fn embed(&self, tokens: &[u32]) -> Result<Vec<f32>, String>;
}

/// What [`Head::run`] did, with the intermediates that make a disagreement debuggable.
///
/// The token count and the embedding length are carried because they are exactly what a parity
/// failure needs to be diagnosed: a length mismatch means the two sides truncated differently, while
/// matching lengths with different labels is an arithmetic or policy difference.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// The decision itself.
    pub decision: Decision,
    /// How many tokens the sequence had, after `build_sequence`'s own budgeting.
    pub tokens: usize,
    /// How many rows the backbone returned (`embedding.len() / d`).
    pub rows: usize,
    /// The marker row indices the head was given.
    pub marker_pos: Vec<usize>,
}

/// A failure anywhere along the path, named by which stage failed.
///
/// One error type for the whole pipeline rather than three, because a caller running a corpus wants
/// to report "case 7 failed at the tokenizer" rather than downcast a boxed error. The stages are
/// distinct variants so the report can say which one.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The sequence could not be built or tokenized.
    #[error("tokenizer: {0}")]
    Tokenize(#[from] TokenizerError),
    /// The backbone could not be reached, or refused the request.
    #[error("backbone: {0}")]
    Backbone(String),
    /// The backbone's response was not the shape the head needs.
    #[error(
        "backbone returned {got} values for {tokens} tokens at d={d}, expected exactly {want} \
         ({tokens} rows of {d}) — a short response means the backbone truncated the sequence, and \
         the head would then be decided from a hidden state that does not cover it"
    )]
    BadEmbedding {
        /// Values returned.
        got: usize,
        /// Row width the head needs.
        d: usize,
        /// Values required.
        want: usize,
        /// Tokens sent.
        tokens: usize,
    },
    /// The forward pass or the post-processing failed.
    #[error("head: {0}")]
    Head(#[from] HeadError),
}

impl Head {
    /// Run the whole path for one question: build the sequence, embed it, decide.
    ///
    /// `state` is the same value the reference accepts — a string, a JSON object, or a list of
    /// turns — and it is passed to `build_sequence` unchanged, because that is what the reference
    /// does and the rendering rules (`serialize_state`) are part of the parity claim.
    ///
    /// # Errors
    /// [`RunError`] names the stage: `Tokenize`, `Backbone`, `BadEmbedding`, or `Head`.
    pub fn run<E: Embedder>(
        &self,
        config: &HeadConfig,
        tokenizer: &Tokenizer,
        embedder: &E,
        state: &Value,
        question: &Question,
    ) -> Result<Run, RunError> {
        self.run_with(
            config,
            tokenizer,
            embedder,
            state,
            question,
            DecideOptions::default(),
        )
    }

    /// [`Head::run`] with the caller's options supplied.
    ///
    /// # Errors
    /// As [`Head::run`].
    pub fn run_with<E: Embedder>(
        &self,
        config: &HeadConfig,
        tokenizer: &Tokenizer,
        embedder: &E,
        state: &Value,
        question: &Question,
        opts: DecideOptions,
    ) -> Result<Run, RunError> {
        // 1. Text to tokens, including the option-marker positions.
        //
        // `Budget::LAYA` is the reference's (`max_len` 512 / `head_max_len` 192) and no option
        // reordering or left-truncation is applied — the reference passes no order and truncates on
        // the RIGHT (`build_sequence`'s `truncate_left` is false for the state).
        // The ABI question is the caller's contract; `build_sequence` renders the reference's
        // internal shape. `from_abi` is the single documented bridge (it reproduces the
        // reference's `_to_internal`), so the conversion happens here rather than in the
        // tokenizer's public signature.
        let internal = crate::tokenizer::Question::from_abi(question)?;
        let seq = tokenizer.build_sequence(state, &internal, Budget::LAYA, None, false)?;
        let tokens = seq.ids.len();
        if tokens == 0 {
            return Err(RunError::Backbone(
                "build_sequence produced an empty sequence; there is nothing to embed".to_string(),
            ));
        }

        // 2. The backbone. One call for the whole sequence, which is what lets the head's attention
        //    see the option markers against the state — the reason the model is a head and not a
        //    classifier.
        let hidden = embedder.embed(&seq.ids).map_err(RunError::Backbone)?;

        // 3. The shape contract. Refused rather than reshaped: see the module docs.
        let d = self.d();
        let want = tokens * d;
        if hidden.len() != want {
            return Err(RunError::BadEmbedding {
                got: hidden.len(),
                d,
                want,
                tokens,
            });
        }

        // 4. The head and the post-processing.
        // `Sequence::markers` are the positions of the active markers; a built sequence has no
        // inactive ones, so the mask is all-true. It is built rather than assumed because the head
        // takes a mask and an empty mask would mean "no options" rather than "all of them".
        let marker_mask = vec![true; seq.markers.len()];
        let decision =
            self.decide_with(config, question, &hidden, &seq.markers, &marker_mask, opts)?;

        Ok(Run {
            decision,
            tokens,
            rows: tokens,
            marker_pos: seq.markers,
        })
    }
}
