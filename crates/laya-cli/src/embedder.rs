//! The live backbone: llama.cpp's `/embedding`, behind `laya-head`'s [`Embedder`] seam.
//!
//! ## Why this lives here and not in `laya-head`
//!
//! `laya-head` must stay `#![forbid(unsafe_code)]`, dependency-light, and testable with no network —
//! it is the crate whose whole job is to be checkable against `PyTorch`. So it defines the
//! [`Embedder`] trait and this crate, which already depends on `ureq` for the readiness probe,
//! provides the one implementation that talks to llama.cpp.
//!
//! ## The request shape, which is the reference's
//!
//! `{"content": [[ids...]]}` — a **list of token lists**, because llama.cpp's `/embedding` is
//! batch-shaped; the reference wraps its single sequence in an outer list for the same reason. The
//! response is `[{"embedding": [...]}]`, one entry per input.
//!
//! ## The two failure modes that matter
//!
//! **A truncated response.** llama.cpp truncates at its own `max_len`. If it does, the head would be
//! decided from a hidden state covering only part of the sequence while the marker positions still
//! refer to all of it — an answer that computes cleanly and is wrong. `laya-head` refuses the length
//! mismatch, and the error names both numbers, so the cause is visible rather than inferred.
//!
//! **An error body.** llama.cpp answers a bad request with a JSON object, not a list. Reading that
//! as "no embeddings" would look like an empty result; it is checked and reported as the server's
//! own message.

use laya_head::end_to_end::Embedder;

/// llama.cpp's `/embedding`, over HTTP.
#[derive(Debug, Clone)]
pub struct HttpEmbedder {
    endpoint: String,
    timeout: std::time::Duration,
}

/// What can go wrong reaching the backbone.
///
/// A `String` at the trait boundary, because `laya-head::RunError::Backbone` carries the message
/// straight to the caller — but the kinds are distinguished here so the message can say which one
/// it was.
#[derive(Debug)]
pub enum EmbedError {
    /// The request could not be sent or the connection failed.
    Transport(String),
    /// The server answered with a non-success status.
    Status {
        /// The status code.
        code: u16,
        /// The server's own message, when there was one.
        body: String,
    },
    /// The response was not the shape `/embedding` documents.
    Shape(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(why) => write!(f, "cannot reach the backbone: {why}"),
            Self::Status { code, body } => {
                write!(f, "the backbone answered HTTP {code}: {body}")
            }
            Self::Shape(why) => write!(f, "the backbone's response is not usable: {why}"),
        }
    }
}

impl HttpEmbedder {
    /// Point at a llama.cpp server.
    ///
    /// The timeout is generous on purpose: the first call after a restart pays for the model's
    /// prompt cache being cold, and the reference's own timeout is 60 s for one sequence.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: std::time::Duration::from_mins(2),
        }
    }

    /// The endpoint this embedder will call.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// One `/embedding` call. Returns the flat `[seq, d]` values for the single sequence sent.
    ///
    /// # Errors
    /// [`EmbedError`], whose `Display` says which kind of failure it was and names the endpoint.
    pub fn call(&self, tokens: &[u32]) -> Result<Vec<f32>, EmbedError> {
        // `content` is a list of token lists: the batch shape, with one entry, exactly as the
        // reference sends it.
        let body = serde_json::json!({ "content": [tokens] });
        let url = format!("{}/embedding", self.endpoint.trim_end_matches('/'));

        let payload = serde_json::to_string(&body).map_err(|e| EmbedError::Shape(e.to_string()))?;
        let mut resp = ureq::post(&url)
            .config()
            .timeout_global(Some(self.timeout))
            .build()
            .header("Content-Type", "application/json")
            .send(payload.as_bytes())
            .map_err(|e| EmbedError::Transport(format!("{url}: {e}")))?;

        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| EmbedError::Transport(format!("{url}: body: {e}")))?;

        if !(200..300).contains(&status) {
            return Err(EmbedError::Status {
                code: status,
                body: text,
            });
        }

        parse_embedding(&text).map_err(|why| EmbedError::Shape(format!("{url}: {why}")))
    }
}

/// Extract the embedding from llama.cpp's response body.
///
/// Split out and public so it can be tested without a server — the parsing is where a silent bug
/// would live, and the error cases (an error object, an empty list, a non-numeric element) are worth
/// pinning directly.
///
/// # Errors
/// A human-readable reason the body is not usable.
pub fn parse_embedding(text: &str) -> Result<Vec<f32>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;

    // An error body is an OBJECT (`{"error": {...}}`), which is how llama.cpp reports a bad request.
    // Reading it as a list would look like "no embeddings" rather than a failure.
    if value.is_object() {
        let message = value
            .get("error")
            .map_or_else(|| value.to_string(), ToString::to_string);
        return Err(format!("the server returned an error object: {message}"));
    }

    let entries = value
        .as_array()
        .ok_or_else(|| format!("expected a list of results, got {}", json_kind(&value)))?;
    let first = entries
        .first()
        .ok_or_else(|| "the result list is empty, so nothing was embedded".to_string())?;
    let embedding = first
        .get("embedding")
        .ok_or_else(|| "the first result has no `embedding` field".to_string())?;
    let rows = embedding
        .as_array()
        .ok_or_else(|| format!("`embedding` is a {}, not a list", json_kind(embedding)))?;

    // llama.cpp returns `embedding` as a list of ROWS: `[[d values], [d values], …]`, one row per
    // input token. (Measured against the live server: a 2-token input returns 2 rows of 1024.)
    //
    // The Python sidecar hides this — it writes `h[:n] = torch.tensor(r[0]["embedding"])`, and torch
    // accepts a nested list as a 2-D tensor — so a port that copied the `r[0]["embedding"]` call
    // without checking the nesting reads a list of lists where it expected numbers. That is what
    // happened here: the first live run reported "embedding[0] is a list, not a number" and the
    // error named the shape, which is why the message is worth writing carefully.
    //
    // FLATTENING IS CORRECT ONLY IF EVERY ROW IS THE SAME WIDTH. A ragged result cannot be a hidden
    // state, so it is refused rather than silently concatenated into something of the wrong stride —
    // which the head would then read as plausible rows of the wrong length.
    let mut out: Vec<f32> = Vec::new();
    let mut width: Option<usize> = None;
    for (row_index, row) in rows.iter().enumerate() {
        let cells = row.as_array().ok_or_else(|| {
            if row.is_number() {
                // A FLAT `embedding`. Not what this server does, but a shape worth naming rather
                // than failing on obscurely if a future server flattens it.
                format!(
                    "`embedding` is flat (element {row_index} is a number); expected a list of rows. \
                     Both shapes are handled, so this should not happen — the response is \
                     inconsistent."
                )
            } else {
                format!("embedding[{row_index}] is a {}, not a list", json_kind(row))
            }
        })?;
        if let Some(w) = width {
            if cells.len() != w {
                return Err(format!(
                    "embedding[{row_index}] has {} values but embedding[0] has {w}: the rows are \
                     ragged, so this is not a `[seq, d]` hidden state",
                    cells.len()
                ));
            }
        } else {
            width = Some(cells.len());
        }
        for (cell_index, v) in cells.iter().enumerate() {
            // A f32 is what the head consumes; the server sends f32 widened to JSON numbers.
            #[allow(clippy::cast_possible_truncation)]
            let number = v.as_f64().ok_or_else(|| {
                format!(
                    "embedding[{row_index}][{cell_index}] is a {}, not a number",
                    json_kind(v)
                )
            })?;
            // The cast is intentional and is the whole point of the conversion: the server sends
            // f32 widened to a JSON number, and the head consumes f32. Truncating back to f32 is
            // exact for any value that came from an f32 in the first place.
            #[allow(clippy::cast_possible_truncation)]
            out.push(number as f32);
        }
    }
    if out.is_empty() {
        return Err("the embedding is empty".to_string());
    }
    Ok(out)
}

/// The JSON kind of a value, for an error that says what was actually found.
const fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "object",
    }
}

impl Embedder for HttpEmbedder {
    fn embed(&self, tokens: &[u32]) -> Result<Vec<f32>, String> {
        self.call(tokens).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    // A test should fail loudly, indexes small fixed slices it builds itself, and casts a
    // three-digit index to f64 to construct its expectation.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_precision_loss
    )]

    use super::*;

    #[test]
    fn the_measured_shape_parses_to_a_flat_embedding() {
        // THE MEASURED SHAPE, from the live server: `embedding` is a list of ROWS, one per input
        // token. Measured with a 2-token input, which returned 2 rows of 1024.
        let body = r#"[{"embedding":[[0.5,-1.25,3.0],[1.0,2.0,4.0]]}]"#;
        let got = parse_embedding(body).expect("must parse");
        assert_eq!(got, vec![0.5_f32, -1.25, 3.0, 1.0, 2.0, 4.0]);
    }

    #[test]
    fn the_row_count_is_the_sequence_length() {
        // The property the head depends on: flattening a `[seq, d]` result gives `seq * d` values in
        // row-major order, which is what `Head::run` then checks against the token count.
        let d = 4;
        let seq = 3;
        let rows: Vec<Vec<f64>> = (0..seq)
            .map(|r| (0..d).map(|c| (r * d + c) as f64).collect())
            .collect();
        let body = serde_json::json!([{"embedding": rows}]).to_string();
        let got = parse_embedding(&body).expect("must parse");
        assert_eq!(got.len(), seq * d);
        // Row-major: row 2, column 1 is index 2*4+1 = 9 and holds 9.0.
        assert!(
            (got[9] - 9.0).abs() < f32::EPSILON,
            "row-major order must be preserved"
        );
    }

    #[test]
    fn a_ragged_result_is_refused_rather_than_concatenated() {
        // Ragged rows cannot be a hidden state. Concatenating them would give a length the head
        // might still accept by coincidence, with the wrong stride.
        let body = r#"[{"embedding":[[1.0,2.0],[3.0]]}]"#;
        let err = parse_embedding(body).expect_err("ragged rows must be refused");
        assert!(err.contains("ragged"), "got: {err}");
    }

    #[test]
    fn an_error_object_is_reported_rather_than_read_as_empty() {
        // llama.cpp reports a bad request with an object, not a list. Reading that as "no results"
        // would look like an empty embedding instead of a failure.
        let body = r#"{"error":{"code":400,"message":"the input is too long"}}"#;
        let err = parse_embedding(body).expect_err("an error object must not parse as embeddings");
        assert!(err.contains("error object"), "got: {err}");
        assert!(
            err.contains("too long"),
            "the server's message must survive: {err}"
        );
    }

    #[test]
    fn an_empty_result_list_is_an_error_not_an_empty_embedding() {
        let err = parse_embedding("[]").expect_err("[] must not yield an empty embedding");
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn a_missing_embedding_field_is_named() {
        let err = parse_embedding(r#"[{"nope":1}]"#).expect_err("must fail");
        assert!(err.contains("embedding"), "got: {err}");
    }

    #[test]
    fn a_non_numeric_element_names_both_indices() {
        let err = parse_embedding(r#"[{"embedding":[[1.0,"x",3.0]]}]"#).expect_err("must fail");
        assert!(
            err.contains("embedding[0][1]"),
            "both indices must be named: {err}"
        );
        assert!(
            err.contains("string"),
            "the kind found must be named: {err}"
        );
    }

    #[test]
    fn a_non_list_body_says_what_it_was() {
        let err = parse_embedding("not json at all").expect_err("must fail");
        assert!(err.contains("not JSON"), "got: {err}");
        let err = parse_embedding(r#""a string""#).expect_err("must fail");
        assert!(err.contains("expected a list"), "got: {err}");
    }

    #[test]
    fn the_endpoint_is_used_verbatim_apart_from_a_trailing_slash() {
        // A trailing slash would make `//embedding`, which some servers redirect and some 404 —
        // either way it is a difference worth removing rather than relying on.
        let e = HttpEmbedder::new("http://10.10.10.217:8083/");
        assert_eq!(e.endpoint(), "http://10.10.10.217:8083/");
    }
}
