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

use crate::policy::{Band, GateDecision, Policy};

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
    let end = text.floor_char_boundary(MAX_STREAM_BYTES);
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
    // Checked against the command AS WRITTEN first, so an exact, already-covered spelling
    // keeps reporting the exact needle it always has (pinned by
    // `a_signal_names_what_matched_and_why`). Only when that finds nothing do we check the
    // NORMALISED form (R4): `rm -Rf`, `rm  -rf` and `rm --recursive --force` are the same
    // command as `rm -rf`, spelled differently, and a signal that only reads one spelling is
    // a gap an agent does not even need to look for — it falls out of normal flag order.
    let found = signals_in(command);
    if !found.is_empty() {
        return found;
    }
    signals_in(&normalise_command(command))
}

/// (needle, why). `needle` is matched case-sensitively: these are shell metacharacters and
/// flags, where case is significant.
const DESTRUCTIVE_PATTERNS: [(&str, &str); 12] = [
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

fn signals_in(command: &str) -> Vec<DestructiveSignal> {
    let mut found: Vec<DestructiveSignal> = Vec::new();
    for (needle, why) in DESTRUCTIVE_PATTERNS {
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

/// Normalise a command so equivalent spellings of the same flag read alike (R4).
///
/// Two transforms, both narrow on purpose:
/// 1. Runs of whitespace collapse to one space, so `rm  -rf x` (two spaces) reads the same
///    as `rm -rf x`.
/// 2. A clustered short flag is split one letter per token and lowercased — `-Rf` becomes
///    `-r -f`, and a lone `-R` becomes `-r` — and the two long-form aliases the destructive
///    patterns actually name, `--recursive` and `--force`, map to their short form first so
///    `rm --recursive --force x` reaches the same `-r -f` shape.
///
/// This is NOT a general command-line parser: it exists to feed [`signals_in`], and nothing
/// else reads its output.
#[must_use]
pub fn normalise_command(command: &str) -> String {
    let collapsed = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut tokens: Vec<String> = Vec::new();
    for token in collapsed.split(' ') {
        let token = match token {
            "--recursive" => "-r",
            "--force" => "-f",
            other => other,
        };
        let is_short_flag_cluster = token.len() > 1
            && token.starts_with('-')
            && !token.starts_with("--")
            && token[1..].chars().all(|c| c.is_ascii_alphabetic());
        if is_short_flag_cluster {
            for c in token[1..].chars() {
                tokens.push(format!("-{}", c.to_ascii_lowercase()));
            }
        } else {
            tokens.push(token.to_string());
        }
    }
    tokens.join(" ")
}

// ---------------------------------------------------------------------------
// R1/R2: the allowlist. `allow` requires a positive match; a metacharacter defeats one.
// ---------------------------------------------------------------------------

/// Characters that let a shell do something other than run the words as given — a pipe, a
/// redirect, a subshell, a substitution, a continuation. Any one of these defeats every
/// allowlist entry (R2): a prefix match on the first words is meaningless once the shell is
/// going to do something the match never saw.
const SHELL_METACHARACTERS: [char; 13] = [
    ';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}', '\n', '\\',
];

/// Whether `command` contains a shell metacharacter (R2).
#[must_use]
pub fn has_shell_metacharacters(command: &str) -> bool {
    command.chars().any(|c| SHELL_METACHARACTERS.contains(&c))
}

/// Match `command` against `allow`, a list of whitespace-tokenised command prefixes (R1).
///
/// `"git status"` matches `"git status --short"` (a longer command with the same first
/// words) but not `"git status; rm x"` — not because the words disagree, but because R2's
/// metacharacter check runs first and defeats the match outright.
///
/// Returns the entry that matched, so the caller can report which rule allowed the command
/// (N2's `allowlist_entry`).
#[must_use]
pub fn allowlist_match(command: &str, allow: &[String]) -> Option<String> {
    if has_shell_metacharacters(command) {
        return None;
    }
    let tokens: Vec<&str> = command.split_whitespace().collect();
    allow
        .iter()
        .find(|entry| {
            let entry_tokens: Vec<&str> = entry.split_whitespace().collect();
            !entry_tokens.is_empty()
                && tokens.get(..entry_tokens.len()) == Some(entry_tokens.as_slice())
        })
        .cloned()
}

// ---------------------------------------------------------------------------
// R3/R5/R6: the model verdict, and the one pure gate function.
// ---------------------------------------------------------------------------

/// Which way a `yes`/`no` noul came down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLabel {
    /// The model judged the action safe to run without confirmation.
    Yes,
    /// The model judged the action unsafe.
    No,
}

/// What the model reported, reduced to exactly what the gate is allowed to read.
///
/// The model may only ever **downgrade** a decision (spec 08 overview): it supplies a label,
/// a confidence, and whether the two leading options were a near tie, and nothing else. It
/// cannot grant `Allow` on its own — that also needs an allowlist match.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelVerdict {
    /// The noul's label.
    pub label: ModelLabel,
    /// The noul's confidence, `max(p, 1 − p)`.
    pub confidence: f64,
    /// Whether the decision was a near tie (policy's `near_tie_margin`).
    pub near_tie: bool,
}

impl ModelVerdict {
    /// A confident (or not) `yes`, not a near tie.
    #[must_use]
    pub const fn yes(confidence: f64) -> Self {
        Self {
            label: ModelLabel::Yes,
            confidence,
            near_tie: false,
        }
    }

    /// A confident (or not) `no`, not a near tie.
    #[must_use]
    pub const fn no(confidence: f64) -> Self {
        Self {
            label: ModelLabel::No,
            confidence,
            near_tie: false,
        }
    }

    /// Flag this verdict as a near tie.
    #[must_use]
    pub const fn near_tie(mut self) -> Self {
        self.near_tie = true;
        self
    }
}

/// What the gate concluded, with the evidence behind it (N2: the response stays auditable).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateVerdict {
    /// The final decision: what the caller must act on.
    pub decision: GateDecision,
    /// What the model's label and confidence alone would have decided, ignoring the
    /// allowlist and the destructive-pattern check. Carried so an auditor can see whether a
    /// `deny`/`ask` came from the model or from the command itself.
    pub model_decision: GateDecision,
    /// Whether `command` matched [`Policy::gate_allow`].
    pub allowlisted: bool,
    /// The allowlist entry that matched, if any.
    pub allowlist_entry: Option<String>,
    /// The destructive patterns found in `command` (R4).
    pub signals: Vec<DestructiveSignal>,
}

/// What the model alone would decide, ignoring the allowlist, the destructive-pattern check
/// and `irreversible` — `gate_verdict`'s other inputs apply on top of this.
fn model_only_decision(model: Option<ModelVerdict>, policy: &Policy) -> GateDecision {
    let Some(m) = model else {
        // R6: the head is unreachable. A gate that cannot ask must not permit.
        return GateDecision::Ask;
    };
    let band = policy.band_for(laya_abi::QuestionType::Noul, m.confidence);
    match (m.label, band) {
        (ModelLabel::Yes, Band::High) if !m.near_tie => GateDecision::Allow,
        (ModelLabel::No, Band::High) => GateDecision::Deny,
        _ => GateDecision::Ask,
    }
}

/// The one decision function (R6): I/O-free, so it is testable without a model or a
/// network, and the only thing `laya-mcp` calls to decide `allow`/`deny`/`ask`.
///
/// **`allow` requires a positive allowlist match (R1); the model may only downgrade.** The
/// order, from the architecture diagram:
///
/// 1. A destructive pattern (R4, on the normalised command) forces `ask`, whatever else is
///    true — it may be a legitimate cleanup, so a human confirms rather than a blanket deny.
/// 2. `irreversible` forces `ask` (R5), allowlisted or not.
/// 3. No allowlist match (R1/R2) forces `ask` — the model is never consulted for a command it
///    was never allowed to grant.
/// 4. Otherwise the model decides, and ONLY the model: a confident `yes` with no near tie
///    allows, a confident `no` denies, anything else (including an unreachable model, R6)
///    asks.
#[must_use]
pub fn gate_verdict(
    command: &str,
    model: Option<ModelVerdict>,
    irreversible: bool,
    policy: &Policy,
) -> GateVerdict {
    let signals = destructive_signals(command);
    let allowlist_entry = allowlist_match(command, &policy.gate_allow);
    let allowlisted = allowlist_entry.is_some();
    let model_decision = model_only_decision(model, policy);

    let decision = if !signals.is_empty() || irreversible || !allowlisted {
        GateDecision::Ask
    } else {
        model_decision
    };

    GateVerdict {
        decision,
        model_decision,
        allowlisted,
        allowlist_entry,
        signals,
    }
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

    // ---- spec 13: mutant-killing tests and metamorphic properties -----------

    #[test]
    fn push_without_force_and_dash_f_without_push_are_clean() {
        // Each half of `push && (--force || -f)` alone must NOT signal; kills `&&` -> `||`.
        assert!(destructive_signals("git push origin main").is_empty());
        assert!(destructive_signals("ls -f").is_empty());
        // CONTROL: the two halves together still signal.
        assert!(
            destructive_signals("git push --force origin main")
                .iter()
                .any(|s| s.matched == "push --force")
        );
        assert!(
            destructive_signals("git push -f origin main")
                .iter()
                .any(|s| s.matched == "push --force")
        );
    }

    #[test]
    fn clip_stream_keeps_exactly_the_cap_and_names_the_rest() {
        let at_cap = "a".repeat(MAX_STREAM_BYTES);
        assert_eq!(
            clip_stream(&at_cap),
            at_cap,
            "exactly the cap is not clipped"
        );

        let over = "a".repeat(MAX_STREAM_BYTES + 5);
        let out = clip_stream(&over);
        assert!(out.starts_with(&"a".repeat(MAX_STREAM_BYTES)));
        assert!(!out.starts_with(&"a".repeat(MAX_STREAM_BYTES + 1)));
        assert!(out.contains("5 more bytes"), "{out}");
        assert_eq!(
            out.find('\n'),
            Some(MAX_STREAM_BYTES),
            "the marker begins right after exactly the cap"
        );

        // A 3-byte char straddling the cap: it must be dropped whole, never split.
        let straddle = format!("{}\u{20ac}tail", "a".repeat(MAX_STREAM_BYTES - 1));
        let out = clip_stream(&straddle);
        assert!(out.starts_with(&"a".repeat(MAX_STREAM_BYTES - 1)));
        assert!(
            !out.contains('\u{20ac}'),
            "the straddling char is dropped whole"
        );
        assert!(
            out.contains(&format!(
                "{} more bytes",
                straddle.len() - (MAX_STREAM_BYTES - 1)
            )),
            "{out}"
        );
        assert_eq!(out.find('\n'), Some(MAX_STREAM_BYTES - 1));
    }

    #[test]
    fn the_stream_cap_is_sixteen_kib_and_a_lone_dash_survives_normalisation() {
        assert_eq!(MAX_STREAM_BYTES, 16_384, "the documented cap");
        // A lone `-` (stdin) is not a flag cluster and must not be dropped.
        assert_eq!(normalise_command("cat - x"), "cat - x");
        assert_eq!(normalise_command("rm  -Rf   x"), "rm -r -f x");
        assert_eq!(normalise_command("rm --recursive --force x"), "rm -r -f x");
    }

    #[test]
    fn a_non_object_questions_value_names_the_type_it_was() {
        for (v, found) in [
            (serde_json::json!([1]), "array"),
            (serde_json::json!("s"), "string"),
            (serde_json::json!(1), "number"),
            (serde_json::json!(true), "boolean"),
        ] {
            assert_eq!(
                validate_questions(&v).unwrap_err(),
                AskFailure::QuestionsNotAnObject { found },
                "{v}"
            );
        }
    }

    /// Seeded LCG (Knuth MMIX constants). A failure reproduces from the printed seed.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next()).unwrap_or(0) % n.max(1)
        }
    }

    #[test]
    fn destructive_signals_survive_respelling() {
        const SEED: u64 = 0x13AC_0001;
        // (command, a respelling it must still be caught under)
        let seeds = [
            "rm -rf build",
            "rm -r build",
            "rm -f build",
            "git reset --hard HEAD",
            "dd if=/dev/zero of=/dev/sda",
            "git push --force origin main",
        ];
        let mut rng = Lcg(SEED);
        for case in 0..512_u32 {
            let base = seeds[rng.below(seeds.len())];
            let mut cmd = base.to_string();
            // Insert extra spaces at word boundaries.
            if rng.below(2) == 0 {
                cmd = cmd.replace(' ', &" ".repeat(2 + rng.below(3)));
            }
            // Recombine / split short flags and flip -r <-> -R, for the rm family.
            if cmd.starts_with("rm ") && rng.below(2) == 0 {
                cmd = match rng.below(3) {
                    0 => cmd.replace("-rf", "-r -f"),
                    1 => cmd.replace("-rf", "-fr"),
                    _ => cmd.replace("-r", "-R"),
                };
            }
            if cmd.starts_with("rm ") && rng.below(4) == 0 {
                cmd = cmd.replace("-rf", "--recursive --force");
            }
            assert!(
                !destructive_signals(&cmd).is_empty(),
                "signal lost: seed {SEED:#x}, case {case}, from `{base}` to `{cmd}`"
            );
        }
    }

    #[test]
    fn an_allowlisted_command_with_a_metacharacter_is_never_allowed() {
        const SEED: u64 = 0x13AC_0002;
        let allow: Vec<String> = ["git status", "ls", "cargo test"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let policy = Policy {
            gate_allow: allow.clone(),
            ..Policy::default()
        };
        let confident_yes = ModelVerdict::yes(0.99);
        let suffixes = ["", " x", " --flag", " && echo hi", " foo bar baz", "é"];
        let mut rng = Lcg(SEED);
        for case in 0..512_u32 {
            let entry = &allow[rng.below(allow.len())];
            // CONTROL: the clean command, same entry, same model, IS allowed — so a `Deny`/`Ask`
            // below is the metacharacter's doing and not the fixture's.
            let clean = format!("{entry}{}", suffixes[rng.below(2)]);
            let v = gate_verdict(&clean, Some(confident_yes), false, &policy);
            assert_eq!(
                v.decision,
                GateDecision::Allow,
                "control failed on `{clean}`"
            );

            let meta = SHELL_METACHARACTERS[rng.below(SHELL_METACHARACTERS.len())];
            let suffix = suffixes[rng.below(suffixes.len())];
            let cmd = match rng.below(3) {
                0 => format!("{entry}{meta}{suffix}"),
                1 => format!("{entry} {meta}{suffix}"),
                _ => format!("{entry}{suffix}{meta}"),
            };
            let v = gate_verdict(&cmd, Some(confident_yes), false, &policy);
            assert_ne!(
                v.decision,
                GateDecision::Allow,
                "metacharacter allowed: seed {SEED:#x}, case {case}, `{cmd:?}`"
            );
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

    // ---- R4: normalisation sees through flag and whitespace variants --------

    #[test]
    fn destructive_signals_see_through_flag_and_whitespace_variants() {
        for cmd in [
            "rm -Rf x",
            "rm  -rf x",
            "rm --recursive --force x",
            "rm -r -f x",
        ] {
            assert!(
                !destructive_signals(cmd).is_empty(),
                "`{cmd}` must be caught after normalisation"
            );
        }
    }

    #[test]
    fn destructive_signals_the_control_benign_commands_stay_clean() {
        // The control for the row above: normalisation must not make a benign command look
        // destructive. `ls -la` splits into `-l -a`, neither of which is a pattern.
        for cmd in ["ls -la /tmp", "git status", "cargo test"] {
            assert!(
                destructive_signals(cmd).is_empty(),
                "`{cmd}` must still raise NO signal after normalisation, got {:?}",
                destructive_signals(cmd)
            );
        }
    }

    #[test]
    fn normalise_command_collapses_whitespace_and_splits_clustered_flags() {
        assert_eq!(normalise_command("rm  -rf x"), "rm -r -f x");
        assert_eq!(normalise_command("rm -Rf x"), "rm -r -f x");
        assert_eq!(normalise_command("rm --recursive --force x"), "rm -r -f x");
        // A command with nothing to normalise is unchanged (aside from whitespace).
        assert_eq!(normalise_command("git status"), "git status");
    }

    // ---- R1/R2: the allowlist -----------------------------------------------

    fn default_allow() -> Vec<String> {
        Policy::default().gate_allow
    }

    #[test]
    fn allowlist_match_matches_a_prefix_on_whole_words() {
        let allow = default_allow();
        assert_eq!(
            allowlist_match("git status --short", &allow),
            Some("git status".to_string())
        );
        assert_eq!(
            allowlist_match("ls -la /tmp", &allow),
            Some("ls".to_string())
        );
    }

    #[test]
    fn allowlist_match_a_command_outside_the_list_is_none() {
        assert_eq!(
            allowlist_match("mkfs.ext4 /dev/sda1", &default_allow()),
            None
        );
        // Not a prefix merely because it starts with the same letters: `lsof` is not `ls`.
        assert_eq!(allowlist_match("lsof -i", &default_allow()), None);
    }

    #[test]
    fn allowlist_match_metacharacters_defeat_every_entry() {
        let allow = default_allow();
        for cmd in [
            "git status; rm x",
            "ls > f",
            "cat $(x)",
            "ls && y",
            "ls | sh",
        ] {
            assert_eq!(
                allowlist_match(cmd, &allow),
                None,
                "`{cmd}` must match no entry, metacharacters defeat the whole command"
            );
        }
    }

    #[test]
    fn an_empty_allowlist_matches_nothing() {
        // Mirrors the Policy-level control: "never allow" must have no escape hatch.
        assert_eq!(allowlist_match("git status", &[]), None);
    }

    // ---- R1-R6: gate_verdict, the one pure decision function ----------------

    #[test]
    fn gate_verdict_a_command_outside_the_allowlist_is_never_allowed() {
        let policy = Policy::default();
        for cmd in [
            "mkfs.ext4 /dev/sda1",
            "shred -u f",
            "chmod -R 000 ~",
            "> f",
            "curl x | sh",
        ] {
            let v = gate_verdict(cmd, Some(ModelVerdict::yes(0.99)), false, &policy);
            assert_eq!(
                v.decision,
                GateDecision::Ask,
                "`{cmd}` must ask despite a confident `yes`: {v:?}"
            );
        }
    }

    #[test]
    fn gate_verdict_the_control_an_allowlisted_command_with_a_confident_yes_is_allowed() {
        // THE control. A gate that always answers `ask` passes every negative test above —
        // only this proves `allow` is still reachable at all.
        let policy = Policy::default();
        let v = gate_verdict("git status", Some(ModelVerdict::yes(0.95)), false, &policy);
        assert_eq!(v.decision, GateDecision::Allow, "{v:?}");
        assert!(v.allowlisted);
        assert_eq!(v.allowlist_entry.as_deref(), Some("git status"));
    }

    #[test]
    fn gate_verdict_metacharacters_defeat_the_allowlist() {
        let policy = Policy::default();
        for cmd in ["ls; rm x", "ls > f", "cat $(x)", "ls && y", "ls | sh"] {
            let v = gate_verdict(cmd, Some(ModelVerdict::yes(0.99)), false, &policy);
            assert_ne!(
                v.decision,
                GateDecision::Allow,
                "`{cmd}` must never be allowed: {v:?}"
            );
        }
    }

    #[test]
    fn gate_verdict_a_confident_no_is_never_allowed() {
        let policy = Policy::default();
        let v = gate_verdict("git status", Some(ModelVerdict::no(0.95)), false, &policy);
        assert_eq!(v.decision, GateDecision::Deny, "{v:?}");
    }

    #[test]
    fn gate_verdict_a_near_tie_asks() {
        let policy = Policy::default();
        let v = gate_verdict(
            "git status",
            Some(ModelVerdict::yes(0.95).near_tie()),
            false,
            &policy,
        );
        assert_eq!(v.decision, GateDecision::Ask, "{v:?}");
    }

    #[test]
    fn gate_verdict_irreversible_always_asks() {
        let policy = Policy::default();
        let v = gate_verdict("git status", Some(ModelVerdict::yes(0.99)), true, &policy);
        assert_eq!(v.decision, GateDecision::Ask, "{v:?}");
        // The control: the SAME input with `irreversible=false` is `Allow` — proves the
        // flag, not the allowlist or the model, is what changed the outcome.
        let control = gate_verdict("git status", Some(ModelVerdict::yes(0.99)), false, &policy);
        assert_eq!(control.decision, GateDecision::Allow, "{control:?}");
    }

    #[test]
    fn gate_verdict_an_unreachable_model_asks() {
        let policy = Policy::default();
        let v = gate_verdict("git status", None, false, &policy);
        assert_eq!(v.decision, GateDecision::Ask, "{v:?}");
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
