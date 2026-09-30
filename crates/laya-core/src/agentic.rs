//! L10, the agentic level: the CALLER writes the questions.
//!
//! ## What the level actually asks for
//!
//! Levels 1–9 have us deciding what to ask. L10 inverts that: **the agent writes the
//! questions itself**, over a state it composes from a summary plus files plus a command it
//! wants run. The command goes through the guardrail gate FIRST, and the agent receives the
//! **answers** — never the files and never the command output.
//!
//! > "This is where you stop deciding what Jev should do and the agent decides."
//!
//! ## Why the composing rules live here, in `laya-core`, and not in the server
//!
//! They are pure: given files and outputs, produce the exact bytes the model will judge. That
//! is testable offline with no model and no IO, which is the only way to pin the two things
//! that matter and are invisible otherwise — **the order** of the composed state, and the
//! **cap**.
//!
//! Order matters because of a measured property of the head: it reads a PREFIX of the state.
//! A signal at the START of a 199,951-byte state scored exactly the same 0.788 as the same
//! signal at 3,975 bytes, while the same signal at the END survived at 984 bytes and was
//! **completely gone** at 3,976 (0.087, identical to the no-signal control 0.0873). So
//! whatever is composed first is what gets read, and the freshest evidence — the command the
//! agent just ran — goes first.
//!
//! The cap is enforced as a **refusal, not a truncation**. Silently dropping a file would
//! mean answering questions about a set of inputs the caller never described, and the answer
//! would not say so.

use core::fmt;

/// The most state this module will compose, across every part (256 KiB).
///
/// The individual file reader has its own limit of the same size; this is the limit for the
/// **assembled** state, which is the thing the model is actually handed.
pub const STATE_CAP_BYTES: usize = 256 * 1024;

/// The most of any one stream (stdout or stderr) that is captured, before truncation.
pub const MAX_STREAM_BYTES: usize = 16 * 1024;

// ---------------------------------------------------------------------------
// RED. The tests below pin the contract; the implementation follows in the
// next commit. Compiled against these names so the failure is a missing item
// rather than an invented one.
// ---------------------------------------------------------------------------

/// One file's contribution to the composed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFile {
    /// The path as the caller gave it, so a judgement can be attributed to it.
    pub path: String,
    /// The file's text.
    pub content: String,
}

/// Why a state could not be composed or a question block could not be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskFailure {
    /// No `questions` at all. L10 is the agent writing them; an absent block is not "ask
    /// nothing", it is a caller that has not done the level's work.
    QuestionsMissing,
    /// The block is not a JSON object. It must be keyed by the caller's own ids.
    QuestionsNotAnObject {
        /// What was found instead (the JSON kind), so the message can name it.
        found: &'static str,
    },
    /// The block is empty: no question would be asked.
    QuestionsEmpty,
    /// A question has no `type`.
    QuestionWithoutType {
        /// The caller's own question id.
        id: String,
    },
    /// A question names a type Laya does not have.
    UnknownQuestionType {
        /// The caller's own question id.
        id: String,
        /// The type that was written.
        kind: String,
    },
    /// There is nothing to judge: no files, no command output, no summary.
    NoState,
    /// A `choice` or `score` question has no `criteria`, so there is nothing to choose
    /// between. This was a REAL defect: `validate_questions` accepted
    /// `{"type": "choice", "options": [...]}`, the call reached laya, and laya raised
    /// `AttributeError: 'NoneType' object has no attribute 'items'` — the validator passed a
    /// block that could not possibly work. laya's own docstring is the authority:
    /// `{"type": "choice", "instructions": "...", "criteria": {"optA": "...", ...}}`.
    QuestionWithoutCriteria {
        /// The caller's own question id.
        id: String,
        /// The type, which determines what `criteria` must look like.
        kind: String,
    },
    /// The composed state is over [`STATE_CAP_BYTES`].
    StateTooLarge {
        /// The composed size.
        bytes: usize,
        /// The limit that was exceeded.
        cap: usize,
    },
}

impl fmt::Display for AskFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuestionsMissing => {
                write!(
                    f,
                    "`ask` needs `questions`: the caller writes them at this level"
                )
            }
            Self::QuestionsNotAnObject { found } => write!(
                f,
                "`questions` must be an object keyed by your own ids, not a {found}"
            ),
            Self::QuestionsEmpty => write!(f, "`questions` is empty: no question would be asked"),
            Self::QuestionWithoutType { id } => write!(
                f,
                "question `{id}` has no `type`; it must be noul, choice or score"
            ),
            Self::UnknownQuestionType { id, kind } => write!(
                f,
                "question `{id}` has type `{kind}`; Laya has noul, choice and score"
            ),
            Self::NoState => write!(
                f,
                "`ask` has nothing to judge: give a `summary`, `paths`, or a `command`"
            ),
            Self::QuestionWithoutCriteria { id, kind } => write!(
                f,
                "question `{id}` is a `{kind}` with no `criteria`: a {kind} must say what to \
                 choose between, as {{\"criteria\": {{\"option\": \"description\"}}}}"
            ),
            Self::StateTooLarge { bytes, cap } => write!(
                f,
                "the composed state is {bytes} bytes, over the {cap}-byte limit. Refusing \
                 rather than truncating: a truncated state would answer questions about \
                 inputs you did not describe"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Composing the state
// ---------------------------------------------------------------------------

/// Truncate one stream, appending a marker if anything was dropped.
///
/// A stream is truncated rather than refused: the caller asked for the command to run, so
/// refusing to report its output would be worse than reporting most of it. The marker is not
/// optional — a silent truncation would let a verdict be read as covering output that was
/// never sent.
fn clip_stream(text: &str) -> String {
    if text.len() <= MAX_STREAM_BYTES {
        return text.to_string();
    }
    // Clip on a char boundary so the composed state is always valid UTF-8.
    let mut end = MAX_STREAM_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = text.len() - end;
    format!(
        "{}\n[... TRUNCATED: {dropped} more bytes of this stream were not sent ...]",
        &text[..end]
    )
}

/// Compose the exact bytes the model will judge.
///
/// **Order is load-bearing, not cosmetic.** The head reads a prefix of the state: a signal at
/// the start of a 199,951-byte state scored the same 0.788 as at 3,975 bytes, while the same
/// signal at the END survived at 984 bytes and was gone at 3,976 (0.087, the no-signal
/// control). So the freshest evidence goes first and the oldest last:
///
/// 1. the command output (what just happened)
/// 2. the files named
/// 3. the summary (background)
///
/// Parts that are absent or blank leave no marker at all: an empty section would be read
/// first and spend the prefix on nothing.
///
/// # Errors
/// [`AskFailure::NoState`] when there is nothing to judge; [`AskFailure::StateTooLarge`] when
/// the result would exceed [`STATE_CAP_BYTES`]. The cap is a refusal, never a truncation.
pub fn compose_state(
    summary: Option<&str>,
    files: &[StateFile],
    command_stdout: Option<&str>,
    command_stderr: Option<&str>,
) -> Result<String, AskFailure> {
    let mut parts: Vec<String> = Vec::new();

    let out = command_stdout.map(clip_stream).unwrap_or_default();
    let err = command_stderr.map(clip_stream).unwrap_or_default();
    if !out.trim().is_empty() || !err.trim().is_empty() {
        let mut section = String::from("COMMAND OUTPUT\n");
        if !out.trim().is_empty() {
            section.push_str(&out);
            section.push('\n');
        }
        if !err.trim().is_empty() {
            section.push_str("-- stderr --\n");
            section.push_str(&err);
            section.push('\n');
        }
        parts.push(section);
    }

    for f in files {
        if f.content.trim().is_empty() {
            continue;
        }
        parts.push(format!("FILE {}\n{}\n", f.path, f.content));
    }

    if let Some(s) = summary.filter(|s| !s.trim().is_empty()) {
        parts.push(format!("SUMMARY\n{s}\n"));
    }

    if parts.is_empty() {
        return Err(AskFailure::NoState);
    }

    let composed = parts.concat();
    if composed.len() > STATE_CAP_BYTES {
        return Err(AskFailure::StateTooLarge {
            bytes: composed.len(),
            cap: STATE_CAP_BYTES,
        });
    }
    Ok(composed)
}

// ---------------------------------------------------------------------------
// The question block
// ---------------------------------------------------------------------------

/// The three question types Laya has.
const QUESTION_TYPES: [&str; 3] = ["noul", "choice", "score"];

/// Check the caller's question block and return it for forwarding.
///
/// The block is forwarded **unchanged**: at this level the agent writes the questions, so
/// filtering or reinterpreting them would defeat the level. This only rejects a block that
/// could not produce an answer, and says which question is at fault.
///
/// # Errors
/// One [`AskFailure`] per way a block can be unusable. A single bad question refuses the whole
/// block rather than dropping that question, because silently asking fewer questions than the
/// caller wrote would return an answer set they did not describe.
pub fn validate_questions(questions: &serde_json::Value) -> Result<&serde_json::Value, AskFailure> {
    if questions.is_null() {
        return Err(AskFailure::QuestionsMissing);
    }
    let Some(obj) = questions.as_object() else {
        let found = match questions {
            serde_json::Value::Array(_) => "array",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::Bool(_) => "boolean",
            _ => "value",
        };
        return Err(AskFailure::QuestionsNotAnObject { found });
    };
    if obj.is_empty() {
        return Err(AskFailure::QuestionsEmpty);
    }
    for (id, q) in obj {
        let Some(kind) = q.get("type").and_then(serde_json::Value::as_str) else {
            return Err(AskFailure::QuestionWithoutType { id: id.clone() });
        };
        if !QUESTION_TYPES.contains(&kind) {
            return Err(AskFailure::UnknownQuestionType {
                id: id.clone(),
                kind: kind.to_string(),
            });
        }
        // A `choice` or `score` cannot be asked without `criteria`: there would be nothing to
        // choose between, and laya raises on it. Accepting such a block is how a validator
        // passes a request that can never work — the defect class this project keeps
        // removing, caught here in this validator's own first day.
        if kind != "noul" && q.get("criteria").is_none() {
            return Err(AskFailure::QuestionWithoutCriteria {
                id: id.clone(),
                kind: kind.to_string(),
            });
        }
    }
    Ok(questions)
}

// ---------------------------------------------------------------------------
// The second signal: a conservative, exact-where-it-matches check
// ---------------------------------------------------------------------------

/// A destructive primitive found in a command, with the text that matched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DestructiveSignal {
    /// What matched, quoted so a refusal can name it.
    pub matched: String,
    /// Why that is destructive.
    pub why: &'static str,
}

/// Find destructive primitives in a command.
///
/// **This is one signal, and it is deliberately NOT the only one.** It is narrow and exact
/// where it matches, and blind where it does not — a command can destroy data in ways this
/// list does not name, and an agent can obfuscate an idiom. Its job is to catch the cases the
/// model was *measured* to miss (`: > file`, `dd if=... of=...`), while the model catches the
/// shapes nobody enumerated. Neither is sufficient; together they overlap usefully.
///
/// The patterns describe EFFECTS rather than command names, so `truncate -s 0` and `: > f`
/// are caught even though neither contains the word `rm`.
#[must_use]
pub fn destructive_signals(command: &str) -> Vec<DestructiveSignal> {
    // (needle, why). `needle` is matched case-sensitively against the raw command: these are
    // shell metacharacters and flags, where case is significant.
    const PATTERNS: [(&str, &str); 12] = [
        ("rm -rf", "recursively deletes files without prompting"),
        ("rm -r", "recursively deletes files"),
        ("rm -f", "deletes files without prompting"),
        ("find ", "may delete with -delete / -exec"),
        ("-delete", "deletes matched files"),
        ("dd ", "writes raw bytes over a device or file"),
        ("of=/dev/", "writes raw bytes to a device"),
        ("truncate ", "erases a file's contents"),
        ("mv ", "moves data, and to /dev/null discards it"),
        (
            "/dev/null",
            "discards data irrecoverably when used as a destination",
        ),
        (": >", "truncates a file to zero bytes"),
        ("git reset --hard", "discards uncommitted work"),
    ];
    let mut found: Vec<DestructiveSignal> = Vec::new();
    for (needle, why) in PATTERNS {
        if command.contains(needle) {
            found.push(DestructiveSignal {
                matched: needle.to_string(),
                why,
            });
        }
    }
    // A force-push destroys remote history, which no local check can see.
    if command.contains("push") && (command.contains("--force") || command.contains(" -f")) {
        found.push(DestructiveSignal {
            matched: "push --force".to_string(),
            why: "rewrites remote history, which cannot be recovered locally",
        });
    }
    found
}

// ---------------------------------------------------------------------------
// The contract. No model, no IO: composing bytes is pure, which is the only way
// to pin the ORDER and the CAP, the two properties that are invisible otherwise.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> StateFile {
        StateFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    // ---- composing: ORDER is the load-bearing property ---------------------

    #[test]
    fn the_command_output_is_composed_first_because_the_head_reads_a_prefix() {
        // This is the whole reason ordering is specified rather than incidental. The
        // freshest evidence is the command the agent just ran, and a state large enough to
        // lose its tail must lose the OLDEST material, not the newest.
        let files = [file("src/old.rs", "fn old() {}")];
        let composed = compose_state(
            Some("we were refactoring"),
            &files,
            Some("test FAILED: expected 3, got 2"),
            Some(""),
        )
        .expect("composes");

        let out_pos = composed.find("test FAILED").expect("output present");
        let file_pos = composed.find("fn old()").expect("file present");
        let sum_pos = composed.find("refactoring").expect("summary present");
        assert!(
            out_pos < file_pos && file_pos < sum_pos,
            "order must be output, then files, then summary; got:\n{composed}"
        );
    }

    #[test]
    fn the_state_names_each_part_so_a_judgement_can_be_attributed() {
        let files = [file("a.rs", "AAA"), file("b.rs", "BBB")];
        let composed =
            compose_state(Some("SUM"), &files, Some("OUT"), Some("ERR")).expect("composes");

        for marker in ["a.rs", "b.rs", "SUM", "OUT", "ERR"] {
            assert!(
                composed.contains(marker),
                "`{marker}` must appear: {composed}"
            );
        }
        // And the paths appear as labels BEFORE their content, not merely somewhere.
        assert!(
            composed.find("a.rs") < composed.find("AAA"),
            "a file's path must precede its content"
        );
    }

    #[test]
    fn stderr_is_carried_because_a_failing_command_reports_there() {
        // A test runner prints its failure to stderr. Dropping stderr would drop exactly the
        // evidence the common "the tests are red" case depends on.
        let composed =
            compose_state(None, &[], Some("partial"), Some("PANIC: index 3")).expect("composes");
        assert!(composed.contains("PANIC: index 3"), "{composed}");
    }

    #[test]
    fn empty_parts_are_omitted_rather_than_labelled_empty() {
        // An empty section is noise the head reads first. Absent parts must leave no marker.
        let composed = compose_state(None, &[], Some("ONLY"), None).expect("composes");
        assert!(composed.contains("ONLY"), "{composed}");
        // The section LABEL is required (a judgement must be attributable), so the invariant
        // is not "no text but the output" — it is that the MISSING parts leave no marker of
        // their own. An empty `-- stderr --` or `SUMMARY` heading would be read first and
        // mean nothing.
        for absent in ["-- stderr --", "SUMMARY", "FILE "] {
            assert!(
                !composed.contains(absent),
                "an absent part must leave no `{absent}` marker: {composed:?}"
            );
        }
    }

    #[test]
    fn a_state_with_nothing_at_all_is_refused() {
        let err = compose_state(None, &[], None, None).expect_err("must refuse");
        assert_eq!(err, AskFailure::NoState);
        assert!(err.to_string().contains("nothing to judge"), "{err}");
    }

    #[test]
    fn a_whitespace_only_summary_is_not_a_state() {
        let err = compose_state(Some("   \n  "), &[], None, None).expect_err("must refuse");
        assert_eq!(err, AskFailure::NoState);
    }

    // ---- the CAP is a refusal, never a truncation -------------------------

    #[test]
    fn a_state_over_the_cap_is_refused_not_truncated() {
        let big = "x".repeat(STATE_CAP_BYTES + 1);
        let err =
            compose_state(None, &[file("huge.txt", &big)], None, None).expect_err("must refuse");
        match err {
            AskFailure::StateTooLarge { bytes, cap } => {
                assert!(bytes > cap, "bytes {bytes} must exceed cap {cap}");
                assert_eq!(cap, STATE_CAP_BYTES);
            }
            other => panic!("expected StateTooLarge, got {other:?}"),
        }
        let msg = AskFailure::StateTooLarge {
            bytes: 999,
            cap: STATE_CAP_BYTES,
        }
        .to_string();
        assert!(msg.contains("Refusing"), "the message must refuse: {msg}");
        assert!(
            msg.contains("did not describe"),
            "and must say why truncation is wrong: {msg}"
        );
    }

    #[test]
    fn exactly_at_the_cap_is_allowed_and_one_byte_over_is_not() {
        // The boundary, both sides. A cap that is off by one is a cap nobody can reason about.
        //
        // The cap applies to the COMPOSED state, which also carries a `FILE <path>` wrapper
        // and a trailing newline. So the CONTENT is sized so the total lands exactly on the
        // cap — rather than setting the content to the cap and forgetting the wrapper, which
        // is how this test was wrong the first time.
        const PATH: &str = "f.txt";
        let wrapper = format!("FILE {PATH}\n").len() + 1;
        let at_cap = "x".repeat(STATE_CAP_BYTES - wrapper);
        assert_eq!(
            compose_state(None, &[file(PATH, &at_cap)], None, None)
                .expect("exactly at the cap must compose")
                .len(),
            STATE_CAP_BYTES,
            "the composed state must be exactly the cap"
        );

        let one_over = "x".repeat(STATE_CAP_BYTES - wrapper + 1);
        match compose_state(None, &[file(PATH, &one_over)], None, None) {
            Err(AskFailure::StateTooLarge { bytes, cap }) => {
                assert_eq!(bytes, STATE_CAP_BYTES + 1, "one byte over, exactly");
                assert_eq!(cap, STATE_CAP_BYTES);
            }
            other => panic!("one byte over the cap must be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_long_command_output_is_truncated_and_says_so() {
        // Unlike the whole-state cap, an over-long stream is truncated rather than refused:
        // the caller asked for the command to run, so refusing to report it would be worse.
        // The marker is mandatory, because a silent truncation is a lie by omission.
        let long = "y".repeat(MAX_STREAM_BYTES * 2);
        let composed = compose_state(None, &[], Some(&long), None).expect("composes");
        assert!(
            composed.contains(long.as_str().chars().take(100).collect::<String>().as_str()),
            "the beginning of the stream must survive"
        );
        assert!(
            composed.contains("TRUNCATED"),
            "a truncated stream must say so: {}",
            &composed[composed.len().saturating_sub(200)..]
        );
    }

    #[test]
    fn a_stream_at_exactly_the_stream_cap_is_not_marked_truncated() {
        let exact = "z".repeat(MAX_STREAM_BYTES);
        let composed = compose_state(None, &[], Some(&exact), None).expect("composes");
        assert!(
            !composed.contains("TRUNCATED"),
            "at the cap nothing was dropped, so nothing may claim it was"
        );
    }

    // ---- the second signal ------------------------------------------------

    #[test]
    fn destructive_primitives_are_found_by_effect_not_by_command_name() {
        // The two the model was MEASURED to miss must be caught here, because that is the
        // reason this signal exists.
        for cmd in [": > important.db", "dd if=/dev/zero of=disk.img"] {
            assert!(
                !destructive_signals(cmd).is_empty(),
                "`{cmd}` must be caught by the pattern signal"
            );
        }
        // And idioms with no `rm` in them at all.
        for cmd in [
            "truncate -s 0 app.log",
            "git reset --hard HEAD~5",
            "find . -name '*.tmp' -delete",
            "mv app.db /dev/null",
            "rm -rf node_modules",
            "git push --force origin main",
        ] {
            assert!(
                !destructive_signals(cmd).is_empty(),
                "`{cmd}` must be caught"
            );
        }
    }

    #[test]
    fn a_read_only_command_raises_no_signal() {
        // The CONTROL. A signal that fires on everything is as useless as one that fires on
        // nothing: it would make every command need confirmation and the gate pointless.
        for cmd in [
            "ls -la src",
            "git status",
            "cat README.md",
            "grep -rn foo .",
            "cargo build",
            "npm test",
            "wc -l *.rs",
        ] {
            assert!(
                destructive_signals(cmd).is_empty(),
                "`{cmd}` must raise NO signal, got {:?}",
                destructive_signals(cmd)
            );
        }
    }

    #[test]
    fn a_signal_names_what_matched_and_why() {
        // A refusal must be able to say why. An opaque block teaches nothing and invites a
        // workaround.
        let found = destructive_signals("rm -rf /var/lib/postgresql/data");
        assert!(!found.is_empty());
        let first = &found[0];
        assert_eq!(first.matched, "rm -rf");
        assert!(
            first.why.contains("recursively"),
            "the reason must be explanatory: {}",
            first.why
        );
        // The message a caller sees must contain both.
        let msg = found
            .iter()
            .map(|s| format!("{} ({})", s.matched, s.why))
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            msg.contains("rm -rf") && msg.contains("recursively"),
            "{msg}"
        );
    }

    // ---- the question block -------------------------------------------------

    #[test]
    fn questions_must_be_present() {
        let err = validate_questions(&serde_json::Value::Null).expect_err("must refuse");
        assert_eq!(err, AskFailure::QuestionsMissing);
        assert!(err.to_string().contains("questions"), "{err}");
    }

    #[test]
    fn questions_must_be_an_object_keyed_by_the_callers_ids() {
        let arr = serde_json::json!([{"type": "noul"}]);
        let err = validate_questions(&arr).expect_err("must refuse");
        assert_eq!(err, AskFailure::QuestionsNotAnObject { found: "array" });

        let empty = serde_json::json!({});
        assert_eq!(
            validate_questions(&empty).expect_err("must refuse"),
            AskFailure::QuestionsEmpty
        );
    }

    #[test]
    fn every_question_needs_a_type_laya_actually_has() {
        let no_type = serde_json::json!({"q1": {"instructions": "hi"}});
        assert_eq!(
            validate_questions(&no_type).expect_err("must refuse"),
            AskFailure::QuestionWithoutType { id: "q1".into() }
        );

        let bad = serde_json::json!({"q1": {"type": "regression"}});
        assert_eq!(
            validate_questions(&bad).expect_err("must refuse"),
            AskFailure::UnknownQuestionType {
                id: "q1".into(),
                kind: "regression".into()
            }
        );
    }

    #[test]
    fn all_three_real_types_are_accepted_and_passed_through_untouched() {
        // The caller's block is forwarded verbatim — this level's promise is that the AGENT
        // writes the questions, so we must not filter or reinterpret them.
        //
        // NOTE THE `criteria` SHAPES, taken from laya's own docstring rather than guessed:
        // a choice is {"criteria": {"option": "description"}} and a score is
        // {"criteria": ["lvl0", "lvl1"]}. An earlier version of this test used an `options`
        // key, which laya does not read — the validator accepted it and the call then raised
        // AttributeError inside laya.
        let ok = serde_json::json!({
            "is_bug": {"type": "noul", "instructions": "is this a bug?"},
            "kind": {"type": "choice", "instructions": "which?",
                     "criteria": {"a": "the first", "b": "the second"}},
            "risk": {"type": "score", "instructions": "how risky?",
                     "criteria": {"0": "none", "2": "high"}}
        });
        let out = validate_questions(&ok).expect("accepts all three");
        assert_eq!(out, &ok, "the block must be forwarded unchanged");

        // Control: one bad question in an otherwise good block still refuses, and names it.
        let almost = serde_json::json!({
            "is_bug": {"type": "noul", "instructions": "is this a bug?"},
            "kind": {"type": "choice", "instructions": "which?",
                     "criteria": {"a": "the first", "b": "the second"}},
            "risk": {"type": "scoreish", "criteria": {}}
        });
        assert_eq!(
            validate_questions(&almost).expect_err("must refuse"),
            AskFailure::UnknownQuestionType {
                id: "risk".into(),
                kind: "scoreish".into()
            }
        );
    }

    #[test]
    fn a_choice_without_criteria_is_refused_because_it_could_not_work() {
        // The real defect this rule was added for. `options` was accepted, reached laya, and
        // raised `AttributeError: 'NoneType' object has no attribute 'items'` — a validator
        // passing a request that can never succeed.
        let with_options = serde_json::json!({
            "area": {"type": "choice", "instructions": "which area?",
                     "options": ["billing", "auth"]}
        });
        assert_eq!(
            validate_questions(&with_options).expect_err("must refuse"),
            AskFailure::QuestionWithoutCriteria {
                id: "area".into(),
                kind: "choice".into()
            }
        );
        let msg = validate_questions(&with_options)
            .expect_err("must refuse")
            .to_string();
        assert!(
            msg.contains("criteria"),
            "the message must name the fix: {msg}"
        );
        assert!(msg.contains("area"), "and name the question: {msg}");

        // A score with nothing to score on is the same hole.
        let score_no_crit =
            serde_json::json!({"risk": {"type": "score", "instructions": "how risky?"}});
        assert_eq!(
            validate_questions(&score_no_crit).expect_err("must refuse"),
            AskFailure::QuestionWithoutCriteria {
                id: "risk".into(),
                kind: "score".into()
            }
        );

        // CONTROL: a noul needs no criteria — it is yes/no, so it has nothing to choose
        // between. A rule that demanded criteria of every type would break every gate.
        let noul_only = serde_json::json!({"ok": {"type": "noul", "instructions": "is it ok?"}});
        assert!(
            validate_questions(&noul_only).is_ok(),
            "a noul needs no criteria"
        );
    }

    #[test]
    fn the_composed_state_carries_only_what_the_caller_gave() {
        // L10's promise is that the agent gets ANSWERS and never the files or the output.
        // That is the server's response shape; what this module must guarantee is narrower
        // and checkable: it composes exactly the inputs it was handed, and adds no state of
        // its own (no environment, no cwd, no file listing) that the caller did not ask for.
        let files = [file("one.txt", "CONTENT-ONE")];
        let composed = compose_state(Some("S"), &files, Some("O"), Some("E")).expect("composes");
        for leaked in ["/proc", "/etc", "std::env", "PATH=", "HOME=", "SHELL="] {
            assert!(
                !composed.contains(leaked),
                "composed state must not invent `{leaked}`: {composed}"
            );
        }
    }
}
