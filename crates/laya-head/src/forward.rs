//! The head's forward pass, ported from the reference and checked against it.
//!
//! ## The oracle
//!
//! `tests/fixtures/head_forward.json` is produced by running the **real `PyTorch` modules** —
//! the same `nn.TransformerEncoderLayer` the checkpoint was trained with — over synthetic
//! hidden states (`scripts/make-head-fixture.py`). The Rust port must reproduce those logits.
//!
//! This is possible because the head is a **pure function** of `(hidden, qtype, markers)` plus
//! the weights; only the *backbone* needs the live model. So the head is tested exactly and
//! offline, which is the strongest oracle available for it.
//!
//! ## What the reference required that the spec never said
//!
//! * **The encoder block's activation is `ReLU`, not `GELU`.** `nn.TransformerEncoderLayer` is
//!   constructed without an `activation` argument, and torch's default is `F.relu`. The
//!   `scorer` and `act_head`, by contrast, contain explicit `nn.GELU()` layers. Two different
//!   activations in one model, and the distinction is invisible unless you read the default.
//! * **`norm_first=True`**, so both `LayerNorm`s come *before* their sublayer, and the block is
//!   `x + attn(ln1(x))` then `x + mlp(ln2(x))` — pre-LN, not post-LN.
//! * The attention is **bidirectional** (a plain encoder), with no causal mask, so the
//!   sequence is pooled only through the marker gather and the `[CLS]` position.
//! * `src_key_padding_mask` is all-false when there is no padding, and the block adds the
//!   `type_emb` broadcast **before** the blocks run, not after.

use crate::math::{gelu, layer_norm, linear, linear_relu, softmax};
use crate::safetensors::SafetensorsFile;
use std::collections::BTreeMap;

/// Errors from loading the head weights or running it.
#[derive(Debug, thiserror::Error)]
pub enum HeadError {
    /// A tensor the forward pass needs is absent.
    #[error("the checkpoint has no tensor `{0}`")]
    MissingTensor(String),
    /// A tensor's shape is not what the forward pass requires.
    #[error("tensor `{name}` has {got} elements, expected {want}")]
    BadShape {
        /// The tensor.
        name: String,
        /// How many elements it has.
        got: usize,
        /// How many the pass needs.
        want: usize,
    },
    /// The hidden state does not match the dimensions the weights imply.
    #[error("hidden state is {got} elements, expected seq*{d} = {want} (seq is derived from it)")]
    BadHidden {
        /// Elements supplied.
        got: usize,
        /// Model dimension.
        d: usize,
        /// Expected count.
        want: usize,
    },
}

/// The head's weights, laid out for a forward pass.
///
/// Every field is an `f32` vector in the same row-major order the checkpoint stores, so a
/// tensor can be read straight in with no transposition — which is one fewer place for the
/// port to disagree with the reference in a way that looks like a numerics problem.
pub struct Head {
    d: usize,
    nhead: usize,
    layers: Vec<Block>,
    type_emb: Vec<f32>,
    scorer: Scorer,
    action: ActHead,
}

struct Block {
    // self-attention: in_proj is q,k,v fused as [3d, d]; out_proj is [d, d]
    in_proj_weight: Vec<f32>,
    in_proj_bias: Vec<f32>,
    out_proj_weight: Vec<f32>,
    out_proj_bias: Vec<f32>,
    norm1_weight: Vec<f32>,
    norm1_bias: Vec<f32>,
    norm2_weight: Vec<f32>,
    norm2_bias: Vec<f32>,
    linear1_weight: Vec<f32>,
    linear1_bias: Vec<f32>,
    linear2_weight: Vec<f32>,
    linear2_bias: Vec<f32>,
}

struct Scorer {
    norm_weight: Vec<f32>,
    norm_bias: Vec<f32>,
    lin_weight: Vec<f32>,
    lin_bias: Vec<f32>,
    out_weight: Vec<f32>,
    out_bias: Vec<f32>,
}

struct ActHead {
    lin_weight: Vec<f32>,
    lin_bias: Vec<f32>,
    out_weight: Vec<f32>,
    out_bias: Vec<f32>,
}

/// `LayerNorm` eps: torch's `nn.TransformerEncoderLayer` default, which equals the encoder
/// config's `layer_norm_eps`. Read from the config earlier; pinned here with its source.
pub const LAYER_NORM_EPS: f32 = 1e-5;

fn weights_of(file: &SafetensorsFile, name: &str) -> Result<Vec<f32>, HeadError> {
    file.tensor_f32(name)
        .map_err(|_| HeadError::MissingTensor(name.to_string()))
}

fn shaped(file: &SafetensorsFile, name: &str, want: usize) -> Result<Vec<f32>, HeadError> {
    let v = weights_of(file, name)?;
    if v.len() != want {
        return Err(HeadError::BadShape {
            name: name.to_string(),
            got: v.len(),
            want,
        });
    }
    Ok(v)
}

impl Head {
    /// Load every tensor the forward pass needs from the checkpoint.
    ///
    /// F1 requires **all 36** tensors to be read, including the four `act_head` ones. This
    /// constructor refuses rather than proceeding if any is missing, so a partial load cannot
    /// decide with a silently-zeroed sub-head.
    ///
    /// # Errors
    /// [`HeadError::MissingTensor`] for an absent tensor, or [`HeadError::BadShape`] when one is
    /// present with the wrong number of elements. Both are refusals: a load that cannot see every
    /// weight must not produce a head that decides anyway.
    pub fn load(file: &SafetensorsFile) -> Result<Self, HeadError> {
        // d comes from `type_emb.weight`, which is [3, d].
        let te = weights_of(file, "type_emb.weight")?;
        let d = te.len() / 3;
        let nhead = crate::safetensors::HeadConfig::nhead(d);
        let ff = d * 4; // the block's feed-forward is 4*d, per the reference constructor

        let mut layers = Vec::new();
        for i in 0..2 {
            let p = format!("head.layers.{i}");
            layers.push(Block {
                in_proj_weight: shaped(file, &format!("{p}.self_attn.in_proj_weight"), 3 * d * d)?,
                in_proj_bias: shaped(file, &format!("{p}.self_attn.in_proj_bias"), 3 * d)?,
                out_proj_weight: shaped(file, &format!("{p}.self_attn.out_proj.weight"), d * d)?,
                out_proj_bias: shaped(file, &format!("{p}.self_attn.out_proj.bias"), d)?,
                norm1_weight: shaped(file, &format!("{p}.norm1.weight"), d)?,
                norm1_bias: shaped(file, &format!("{p}.norm1.bias"), d)?,
                norm2_weight: shaped(file, &format!("{p}.norm2.weight"), d)?,
                norm2_bias: shaped(file, &format!("{p}.norm2.bias"), d)?,
                linear1_weight: shaped(file, &format!("{p}.linear1.weight"), ff * d)?,
                linear1_bias: shaped(file, &format!("{p}.linear1.bias"), ff)?,
                linear2_weight: shaped(file, &format!("{p}.linear2.weight"), d * ff)?,
                linear2_bias: shaped(file, &format!("{p}.linear2.bias"), d)?,
            });
        }

        Ok(Self {
            d,
            nhead,
            layers,
            type_emb: te,
            scorer: Scorer {
                norm_weight: shaped(file, "scorer.0.weight", d)?,
                norm_bias: shaped(file, "scorer.0.bias", d)?,
                lin_weight: shaped(file, "scorer.1.weight", d * d)?,
                lin_bias: shaped(file, "scorer.1.bias", d)?,
                out_weight: shaped(file, "scorer.3.weight", d)?,
                out_bias: shaped(file, "scorer.3.bias", 1)?,
            },
            action: ActHead {
                lin_weight: shaped(file, "act_head.0.weight", 256 * (d + 4))?,
                lin_bias: shaped(file, "act_head.0.bias", 256)?,
                out_weight: shaped(file, "act_head.2.weight", 2 * 256)?,
                out_bias: shaped(file, "act_head.2.bias", 2)?,
            },
        })
    }

    /// The model dimension.
    #[must_use]
    pub const fn d(&self) -> usize {
        self.d
    }

    /// The number of attention heads, derived as `max(1, d/64)` (F5).
    #[must_use]
    pub const fn nhead(&self) -> usize {
        self.nhead
    }

    /// The forward pass: `(hidden, qtype, markers) -> (logits, act_logits)`.
    ///
    /// `hidden` is row-major `[seq, d]`. `marker_pos[i]` is the row whose hidden state scores
    /// option `i`, and `marker_mask[i]` says whether option `i` exists.
    ///
    /// **Pure**: `&self`, no interior mutability (F11), so it can be shared across threads
    /// without a lock.
    ///
    /// # Errors
    /// [`HeadError::BadHidden`] when `hidden` is empty or is not a whole number of `d`-sized
    /// rows, since the sequence length is derived from it rather than passed separately.
    pub fn forward(
        &self,
        hidden: &[f32],
        qtype: usize,
        marker_pos: &[usize],
        marker_mask: &[bool],
    ) -> Result<(Vec<f32>, Vec<f32>), HeadError> {
        let dim = self.d;
        if dim == 0 || !hidden.len().is_multiple_of(dim) {
            return Err(HeadError::BadHidden {
                got: hidden.len(),
                d: dim,
                want: hidden.len() / dim.max(1) * dim,
            });
        }
        let seq = hidden.len() / dim;
        if seq == 0 {
            return Err(HeadError::BadHidden {
                got: 0,
                d: dim,
                want: dim,
            });
        }

        // type_emb broadcast add, BEFORE the blocks (the reference does this first).
        // `type_emb.weight` is [3, d], so there are three rows and `qtype` selects one. `nth`
        // rather than a range because `nth` is total: an out-of-range `qtype` yields None instead
        // of panicking on a slice that does not exist.
        //
        // NOTE: an earlier version wrote `.nth(qtype.saturating_sub(1))` — an off-by-one making
        // qtype=1 read row 0. The differential test caught it on the qtype=1 case, which is why
        // that test covers all three question types rather than one.
        let n_rows = self.type_emb.len() / dim.max(1);
        let te_row: Vec<f32> = self
            .type_emb
            .chunks_exact(dim)
            .nth(qtype.min(n_rows.saturating_sub(1)))
            .unwrap_or(&[])
            .to_vec();
        let mut x: Vec<f32> = hidden
            .iter()
            .enumerate()
            .map(|(i, &v)| v + te_row.get(i % dim).copied().unwrap_or(0.0))
            .collect();

        for block in &self.layers {
            x = self.block(block, &x, seq);
        }

        // Gather the marker rows: `marker_pos.clamp(min=0)` then index the sequence.
        let n_markers = marker_pos.len();
        let mut gathered = Vec::with_capacity(n_markers * dim);
        for &pos in marker_pos {
            let clamped = pos.min(seq.saturating_sub(1));
            if let Some(row) = x.chunks_exact(dim).nth(clamped) {
                gathered.extend_from_slice(row);
            }
        }

        // scorer: LayerNorm -> Linear -> GELU -> Linear, per row. GELU HERE (explicit in the
        // reference's nn.Sequential), unlike the encoder block's ReLU.
        let mut logits = Vec::with_capacity(n_markers);
        for row in gathered.chunks_exact(dim) {
            let normed = layer_norm(
                row,
                &self.scorer.norm_weight,
                &self.scorer.norm_bias,
                LAYER_NORM_EPS,
            );
            let mid = linear(
                &self.scorer.lin_weight,
                &self.scorer.lin_bias,
                &normed,
                dim,
                dim,
            );
            let activated: Vec<f32> = mid.iter().map(|&v| gelu(v)).collect();
            let out = linear(
                &self.scorer.out_weight,
                &self.scorer.out_bias,
                &activated,
                1,
                dim,
            );
            logits.push(out.first().copied().unwrap_or(0.0));
        }
        // The mask, applied ONCE and after the values are computed. `masked_fill(~mask, -1e4)`
        // in the reference. Doing it inline in the loop was the first version of this function
        // and the index arithmetic was wrong in a way that read as correct.
        for (i, &present) in marker_mask.iter().enumerate() {
            if !present && let Some(value) = logits.get_mut(i) {
                *value = -1e4;
            }
        }

        let pooled = x.chunks_exact(dim).next().unwrap_or(&[]);
        let act = self.act_features(pooled, &logits, marker_mask);
        Ok((logits, act))
    }

    /// The act head's four features and its two logits, from the detached softmax.
    ///
    /// Extracted from [`Self::forward`] because it is a self-contained computation over the
    /// block output, the logits and the marker mask — and because the reference computes it
    /// from the *detached* softmax, so the value is a signal about the answer rather than part
    /// of how the answer was reached.
    fn act_features(&self, pooled: &[f32], logits: &[f32], marker_mask: &[bool]) -> Vec<f32> {
        let dim = self.d;
        // The act head's features, computed from the DETACHED softmax, as the reference does.
        let probs = softmax(logits);
        // A marker count, bounded by the option cap, so the conversion is exact; the reason is
        // the same one the `math` module records for its counts.
        #[allow(clippy::cast_precision_loss)]
        let k_eff = marker_mask.iter().filter(|&&b| b).count().max(2) as f32;
        let entropy: f32 = {
            let total: f32 = probs
                .iter()
                .map(|&prob| {
                    let clamped = prob.max(1e-9);
                    -prob * clamped.ln()
                })
                .sum();
            total / k_eff.ln()
        };
        // The top two probabilities, in one pass and without sorting the whole vector. A single
        // option has no second entry to take: the reference pads it with 0.0 there, giving
        // top1 - top2 == top1, the same "fully decided" signal an unambiguous gap would give.
        let (top1, top2) = probs
            .iter()
            .fold((0.0_f32, 0.0_f32), |(first, second), &p| {
                if p > first {
                    (p, first)
                } else if p > second {
                    (first, p)
                } else {
                    (first, second)
                }
            });
        let feats = vec![top1, top1 - top2, entropy, k_eff / 255.0];

        // pooled = the first row of the block output, in f32.
        let act_in: Vec<f32> = pooled.iter().copied().chain(feats).collect();
        let a1 = linear_relu(
            &self.action.lin_weight,
            &self.action.lin_bias,
            &act_in,
            256,
            dim + 4,
        );
        linear(&self.action.out_weight, &self.action.out_bias, &a1, 2, 256)
    }

    /// One pre-LN encoder block: `x + attn(ln1(x))` then `x + relu_mlp(ln2(x))`.
    fn block(&self, weights: &Block, x: &[f32], seq: usize) -> Vec<f32> {
        let dim = self.d;
        // norm_first=true: normalise, compute the sublayer, add the residual.
        let mut normed = Vec::with_capacity(seq * dim);
        for row in x.chunks_exact(dim) {
            normed.extend(layer_norm(
                row,
                &weights.norm1_weight,
                &weights.norm1_bias,
                LAYER_NORM_EPS,
            ));
        }
        let attn = self.self_attention(weights, &normed, seq);
        let mut out: Vec<f32> = x.iter().zip(&attn).map(|(&a, &c)| a + c).collect();

        let mut normed2 = Vec::with_capacity(seq * dim);
        for row in out.chunks_exact(dim) {
            normed2.extend(layer_norm(
                row,
                &weights.norm2_weight,
                &weights.norm2_bias,
                LAYER_NORM_EPS,
            ));
        }
        for (row_index, row) in normed2.chunks_exact(dim).enumerate() {
            let mid = linear(
                &weights.linear1_weight,
                &weights.linear1_bias,
                row,
                4 * dim,
                dim,
            );
            // ReLU here: the reference leaves `activation` at torch's default.
            let activated: Vec<f32> = mid.iter().map(|&v| v.max(0.0)).collect();
            let back = linear(
                &weights.linear2_weight,
                &weights.linear2_bias,
                &activated,
                dim,
                4 * dim,
            );
            // The residual add, by chunks rather than index arithmetic.
            if let Some(dst) = out.chunks_exact_mut(dim).nth(row_index) {
                for (slot, &v) in dst.iter_mut().zip(&back) {
                    *slot += v;
                }
            }
        }
        out
    }

    /// Bidirectional multi-head self-attention with the fused `in_proj`.
    fn self_attention(&self, weights: &Block, x: &[f32], seq: usize) -> Vec<f32> {
        let dim = self.d;
        let n_heads = self.nhead;
        let head_dim = dim / n_heads;

        // q, k, v fused: in_proj_weight is [3d, d], rows 0..d = q, d..2d = k, 2d..3d = v.
        let qkv: Vec<f32> = x
            .chunks_exact(dim)
            .flat_map(|row| {
                linear(
                    &weights.in_proj_weight,
                    &weights.in_proj_bias,
                    row,
                    3 * dim,
                    dim,
                )
            })
            .collect();

        // No causal mask: an encoder attends bidirectionally over the whole sequence.
        // A head size, bounded by the model dimension; the conversion is exact.
        #[allow(clippy::cast_precision_loss)]
        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut attended = vec![0.0_f32; seq * dim];
        for head in 0..n_heads {
            for query in 0..seq {
                let mut scores = vec![0.0_f32; seq];
                for (key, score) in scores.iter_mut().enumerate() {
                    let mut acc = 0.0_f32;
                    // A token's projected row is `[q_0..q_{d-1}, k..., v...]`, so chunking it by
                    // `head_dim` yields 3*n_heads pieces: q heads first, then k, then v. Head h of
                    // k is therefore at `n_heads + h`. Reading `1 + head` there was an
                    // off-by-n_heads that the differential test caught on the first case.
                    let q_head = qkv
                        .chunks_exact(3 * dim)
                        .nth(query)
                        .and_then(|r| r.chunks_exact(head_dim).nth(head))
                        .unwrap_or(&[]);
                    let k_head = qkv
                        .chunks_exact(3 * dim)
                        .nth(key)
                        .and_then(|r| r.chunks_exact(head_dim).nth(n_heads + head))
                        .unwrap_or(&[]);
                    for (lhs, rhs) in q_head.iter().zip(k_head) {
                        acc += lhs * rhs;
                    }
                    *score = acc * scale;
                }
                let probs = softmax(&scores);
                for (key, &prob) in probs.iter().enumerate() {
                    if prob == 0.0 {
                        continue;
                    }
                    let v_head = qkv
                        .chunks_exact(3 * dim)
                        .nth(key)
                        .and_then(|r| r.chunks_exact(head_dim).nth(2 * n_heads + head))
                        .unwrap_or(&[]);
                    if let Some(dst) = attended
                        .chunks_exact_mut(dim)
                        .nth(query)
                        .and_then(|r| r.chunks_exact_mut(head_dim).nth(head))
                    {
                        for (slot, &v) in dst.iter_mut().zip(v_head) {
                            *slot += prob * v;
                        }
                    }
                }
            }
        }
        // out_proj over each row
        let mut out = Vec::with_capacity(seq * dim);
        for row in attended.chunks_exact(dim) {
            out.extend(linear(
                &weights.out_proj_weight,
                &weights.out_proj_bias,
                row,
                dim,
                dim,
            ));
        }
        out
    }
}

/// The **35** tensors the forward pass reads, with their expected element counts.
///
/// F1 requires all **36** tensors in the checkpoint to be loaded, and this map is the 35 of them
/// the pass actually consumes. The 36th is `temperature`, a buffer the reference registers but
/// the forward pass does not read (it is applied in the *client*, after the model returns). Both
/// counts matter and they are different numbers, so a test asserting "all 36" against this map
/// would be off by one in a way that looks like a missing tensor.
#[must_use]
pub fn expected_tensor_names(d: usize, ff: usize) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for i in 0..2 {
        let p = format!("head.layers.{i}");
        m.insert(format!("{p}.linear1.bias"), ff);
        m.insert(format!("{p}.linear1.weight"), ff * d);
        m.insert(format!("{p}.linear2.bias"), d);
        m.insert(format!("{p}.linear2.weight"), d * ff);
        m.insert(format!("{p}.norm1.bias"), d);
        m.insert(format!("{p}.norm1.weight"), d);
        m.insert(format!("{p}.norm2.bias"), d);
        m.insert(format!("{p}.norm2.weight"), d);
        m.insert(format!("{p}.self_attn.in_proj_bias"), 3 * d);
        m.insert(format!("{p}.self_attn.in_proj_weight"), 3 * d * d);
        m.insert(format!("{p}.self_attn.out_proj.bias"), d);
        m.insert(format!("{p}.self_attn.out_proj.weight"), d * d);
    }
    m.insert("act_head.0.bias".into(), 256);
    m.insert("act_head.0.weight".into(), 256 * (d + 4));
    m.insert("act_head.2.bias".into(), 2);
    m.insert("act_head.2.weight".into(), 2 * 256);
    m.insert("scorer.0.bias".into(), d);
    m.insert("scorer.0.weight".into(), d);
    m.insert("scorer.1.bias".into(), d);
    m.insert("scorer.1.weight".into(), d * d);
    m.insert("scorer.3.bias".into(), 1);
    m.insert("scorer.3.weight".into(), d);
    m.insert("type_emb.weight".into(), 3 * d);
    m
}
