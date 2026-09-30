//! `laya decide` — the whole path in one command, from text to a decision.
//!
//! ## Why this is a CLI command and not just a library function
//!
//! Spec 03's last exit criterion needs a **live corpus** run: the same states and questions through
//! the Rust path and the Python reference, comparing labels. A harness can only do that if the Rust
//! side can be driven **from text** — its own tokenizer, its own sequence builder, its own backbone
//! call — because a comparison that starts from a pre-computed hidden state would skip exactly the
//! parts that are most likely to differ.
//!
//! It also makes the port usable the way the rest of the workspace already works: `laya doctor`
//! answers "is it up?" with an exit code, and `laya decide` answers a question with JSON on stdout.
//!
//! ## The interface
//!
//! ```text
//! laya decide --state '<json or text>' --question '<json>'   [--url URL] [--head PATH] [--tokenizer PATH]
//! laya decide -                                              read {"state":…,"question":…} from stdin
//! ```
//!
//! Both forms exist because they answer different callers: a shell one-liner wants flags, and a
//! harness with a corpus wants to stream one JSON object per case without quoting anything into a
//! shell.
//!
//! ## Exit codes
//!
//! `0` on a decision, `1` on a usage error, `2` on a readiness failure (the backbone or the head is
//! not usable), `3` on a decision that could not be reached. Distinct codes because a caller that
//! only reads the exit code should be able to tell "you asked me wrong" from "the world is down" from
//! "I got far enough to have an opinion and it failed".

use std::path::PathBuf;

use laya_abi::{Answer, Question};
use laya_head::end_to_end::Run;
use laya_head::forward::Head;
use laya_head::safetensors::{HeadConfig, SafetensorsFile};
use laya_head::tokenizer::Tokenizer;

use crate::embedder::HttpEmbedder;

/// Where the head checkpoint lives by default, matching the sidecar's own default.
pub const DEFAULT_HEAD: &str = "/projects/_tmp/laya-gguf/laya-head.safetensors";

/// The tokenizer, from the pinned HF snapshot the reference uses.
///
/// Hardcoded as a default and overridable because the tokenizer **must** be the one the checkpoint
/// was trained with: a different `tokenizer.json` produces different ids and every downstream number
/// is then wrong while looking perfectly reasonable.
pub const DEFAULT_TOKENIZER: &str = "/root/.cache/huggingface/hub/models--convaiinnovations--laya/snapshots/\
1c5edc17a7acd8701df6fc341c0d179f1c62c982/tokenizer/tokenizer.json";

/// The knobs `decide` accepts, parsed from the subcommand's own arguments.
#[derive(Debug, Clone)]
pub struct DecideArgs {
    /// The state: a JSON object/array, or a bare string.
    pub state: String,
    /// The question, as the workspace's own JSON shape.
    pub question: String,
    /// Read `{"state":…,"question":…}` from stdin instead of from flags.
    pub stdin: bool,
    /// The backbone endpoint.
    pub url: String,
    /// The checkpoint path.
    pub head: PathBuf,
    /// The tokenizer path.
    pub tokenizer: PathBuf,
}

impl DecideArgs {
    /// Parse the subcommand's arguments.
    ///
    /// # Errors
    /// A message naming what is missing or unrecognised — never a silent default, because a caller
    /// who mistypes `--state` and gets a decision for an empty state would have no way to tell.
    pub fn parse(rest: &[String], endpoint: &str) -> Result<Self, String> {
        let mut args = Self {
            state: String::new(),
            question: String::new(),
            stdin: false,
            url: endpoint.to_string(),
            head: PathBuf::from(DEFAULT_HEAD),
            tokenizer: PathBuf::from(DEFAULT_TOKENIZER),
        };
        let mut i = 0;
        while i < rest.len() {
            match rest.get(i).map(String::as_str) {
                Some("--state") => {
                    args.state = take(rest, &mut i, "--state")?;
                }
                Some("--question") => {
                    args.question = take(rest, &mut i, "--question")?;
                }
                Some("--url") => {
                    args.url = take(rest, &mut i, "--url")?;
                }
                Some("--head") => {
                    args.head = PathBuf::from(take(rest, &mut i, "--head")?);
                }
                Some("--tokenizer") => {
                    args.tokenizer = PathBuf::from(take(rest, &mut i, "--tokenizer")?);
                }
                Some("-" | "--stdin") => {
                    args.stdin = true;
                    i += 1;
                }
                Some(other) => {
                    return Err(format!(
                        "laya decide: unrecognised argument `{other}`\n\
                         usage: laya decide --state STATE --question JSON [--url URL] [--head PATH]\n\
                         \x20      laya decide -    (reads {{\"state\":…,\"question\":…}} from stdin)"
                    ));
                }
                None => break,
            }
        }
        if !args.stdin && args.question.is_empty() {
            return Err(
                "laya decide: --question is required (or use `-` to read a JSON object from stdin)\n\
                 usage: laya decide --state STATE --question JSON [--url URL] [--head PATH]"
                    .to_string(),
            );
        }
        Ok(args)
    }
}

/// Take the value after a flag, advancing the cursor.
fn take(rest: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    let value = rest
        .get(*i + 1)
        .ok_or_else(|| format!("laya decide: {flag} needs a value"))?
        .clone();
    *i += 2;
    Ok(value)
}

/// How the harness reads the decision back.
///
/// Serialised as JSON on stdout with the run's diagnostics beside it, because a parity comparison
/// needs more than the label: a disagreement caused by the two sides TRUNCATING differently looks
/// identical to one caused by arithmetic unless the token count is in the output.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DecideOutput {
    /// The answer, in the shared ABI shape.
    pub answer: Answer,
    /// The temperature actually used for this bucket.
    pub temperature: f64,
    /// Whether the configured temperature was clamped to get there, i.e. whether this answer's
    /// confidence is uncalibrated.
    pub bucket_clamped: bool,
    /// The temperature bucket key.
    pub bucket: String,
    /// The gap between the top two raw logits over the options.
    pub logit_gap: f64,
    /// Tokens in the built sequence — what the backbone was asked to embed.
    pub tokens: usize,
    /// Rows the backbone returned. Should equal `tokens`; a smaller number is a truncation.
    pub rows: usize,
    /// The marker row indices, echoed so a mismatch in sequence building is visible.
    pub marker_pos: Vec<usize>,
    /// The act head's softmaxed first logit (inert in this checkpoint; carried for parity).
    pub act_probability: f64,
}

impl From<Run> for DecideOutput {
    fn from(run: Run) -> Self {
        Self {
            answer: run.decision.answer,
            temperature: run.decision.temperature,
            bucket_clamped: run.decision.bucket_clamped,
            bucket: run.decision.bucket,
            logit_gap: run.decision.logit_gap,
            tokens: run.tokens,
            rows: run.rows,
            marker_pos: run.marker_pos,
            act_probability: run.decision.act_probability,
        }
    }
}

/// Run one decision end to end and return the JSON to print.
///
/// # Errors
/// A `(message, exit_code)` pair: the code is what a caller reads, and the message is what a human
/// reads. The codes are documented on the module.
pub fn decide(args: &DecideArgs) -> Result<String, (String, u8)> {
    // 1. Resolve the state and the question. `-` reads one JSON object from stdin, so a harness can
    //    stream cases without shell quoting.
    let (state_text, question_text) = if args.stdin {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .map_err(|e| (format!("laya decide: cannot read stdin: {e}"), 1))?;
        let v: serde_json::Value = serde_json::from_str(&buf).map_err(|e| {
            (
                format!(
                    "laya decide: stdin must be a JSON object like \
                     {{\"state\":\"…\",\"question\":{{…}}}}: {e}"
                ),
                1,
            )
        })?;
        let state = v.get("state").cloned().unwrap_or(serde_json::Value::Null);
        let question = v.get("question").cloned().ok_or_else(|| {
            (
                "laya decide: stdin JSON has no `question` field".to_string(),
                1,
            )
        })?;
        (state, question)
    } else {
        // A bare state is a string unless it parses as JSON, which matches how a caller thinks:
        // `--state 'the build broke'` is text, `--state '{"a":1}'` is a structure.
        let state = match serde_json::from_str::<serde_json::Value>(&args.state) {
            Ok(v @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => v,
            _ => serde_json::Value::String(args.state.clone()),
        };
        let q: serde_json::Value = serde_json::from_str(&args.question).map_err(|e| {
            (
                format!(
                    "laya decide: --question is not JSON: {e}\n\
                     expected e.g. '{{\"id\":\"q\",\"type\":\"choice\",\"text\":\"…\",\"options\":[\"a\",\"b\"]}}'"
                ),
                1,
            )
        })?;
        (state, q)
    };

    let question: Question = serde_json::from_value(question_text).map_err(|e| {
        (
            format!(
                "laya decide: --question is not a valid question: {e}\n\
                 `type` must be choice|score|noul; `choice` and `score` need `options`"
            ),
            1,
        )
    })?;

    // 2. The artifacts. Both are required and both must be the pinned ones, so an absent file is
    //    reported by name rather than surfacing as a confusing downstream failure.
    let checkpoint = SafetensorsFile::open(&args.head).map_err(|e| {
        (
            format!(
                "laya decide: cannot read the head at {}: {e}",
                args.head.display()
            ),
            2,
        )
    })?;
    let config = HeadConfig::from_metadata(checkpoint.metadata()).map_err(|e| {
        (
            format!(
                "laya decide: the head at {} has no usable `laya.config`: {e}",
                args.head.display()
            ),
            2,
        )
    })?;
    let head = Head::load(&checkpoint).map_err(|e| {
        (
            format!(
                "laya decide: the head at {} is not loadable: {e}",
                args.head.display()
            ),
            2,
        )
    })?;
    let tokenizer = Tokenizer::load(&args.tokenizer).map_err(|e| {
        (
            format!(
                "laya decide: cannot read the tokenizer at {}: {e}",
                args.tokenizer.display()
            ),
            2,
        )
    })?;

    // 3. The path itself.
    let embedder = HttpEmbedder::new(&args.url);
    let run = head
        .run(&config, &tokenizer, &embedder, &state_text, &question)
        .map_err(|e| {
            // The stage is in the error's own text, so the code can stay "the decision failed"
            // while the message says where.
            (format!("laya decide: {e}"), 3)
        })?;

    serde_json::to_string(&DecideOutput::from(run)).map_err(|e| {
        (
            format!("laya decide: cannot serialise the decision: {e}"),
            3,
        )
    })
}

/// A short human line for a terminal, beside the JSON on stderr.
#[must_use]
pub fn summarise(out: &DecideOutput) -> String {
    format!(
        "{} = {:?}  confidence {:.4}  margin_gap {:.4}{}  T {:.4} ({})  {} tokens",
        out.answer.id,
        out.answer.value,
        out.answer.confidence,
        out.logit_gap,
        if out.answer.probabilities.iter().any(|p| *p > 0.999) {
            "  SATURATED"
        } else {
            ""
        },
        out.temperature,
        out.bucket,
        out.tokens,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn flags_parse_and_defaults_come_from_the_documented_paths() {
        let a = DecideArgs::parse(
            &args(&["--state", "hello", "--question", r#"{"type":"noul"}"#]),
            "http://x",
        )
        .expect("must parse");
        assert_eq!(a.state, "hello");
        assert_eq!(a.url, "http://x");
        assert_eq!(a.head, PathBuf::from(DEFAULT_HEAD));
        assert_eq!(a.tokenizer, PathBuf::from(DEFAULT_TOKENIZER));
        assert!(!a.stdin);
    }

    #[test]
    fn a_missing_question_is_refused_by_name() {
        let err = DecideArgs::parse(&args(&["--state", "x"]), "http://x").expect_err("must fail");
        assert!(err.contains("--question is required"), "got: {err}");
    }

    #[test]
    fn an_unknown_flag_is_refused_rather_than_ignored() {
        // A silently ignored flag is how a caller gets a decision for something they did not ask.
        let err = DecideArgs::parse(&args(&["--nope", "1"]), "http://x").expect_err("must fail");
        assert!(err.contains("unrecognised argument `--nope`"), "got: {err}");
    }

    #[test]
    fn a_flag_without_a_value_is_refused() {
        let err = DecideArgs::parse(&args(&["--state"]), "http://x").expect_err("must fail");
        assert!(err.contains("--state needs a value"), "got: {err}");
    }

    #[test]
    fn stdin_mode_needs_no_question_flag() {
        let a = DecideArgs::parse(&args(&["-"]), "http://x").expect("must parse");
        assert!(a.stdin);
    }

    #[test]
    fn overrides_are_taken_verbatim() {
        let a = DecideArgs::parse(
            &args(&[
                "-",
                "--url",
                "http://y",
                "--head",
                "/tmp/h",
                "--tokenizer",
                "/tmp/t",
            ]),
            "http://x",
        )
        .expect("must parse");
        assert_eq!(a.url, "http://y");
        assert_eq!(a.head, PathBuf::from("/tmp/h"));
        assert_eq!(a.tokenizer, PathBuf::from("/tmp/t"));
    }

    #[test]
    fn an_unknown_question_type_is_refused_by_serde() {
        // The question schema is the workspace's, so a typo in `type` must not silently become one
        // of the three — that would decide a different question than the caller asked.
        let q: Result<Question, _> =
            serde_json::from_str(r#"{"id":"q","type":"choise","text":"t","options":["a"]}"#);
        assert!(q.is_err(), "a misspelled type must not parse");
    }
}
