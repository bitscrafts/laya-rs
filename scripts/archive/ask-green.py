#!/usr/bin/env python3
"""GREEN for `ask` (L10): insert the composition + validation, above the test module."""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-core/src/agentic.rs")
t = p.read_text()

IMPL = r'''
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

    if let Some(s) = summary {
        if !s.trim().is_empty() {
            parts.push(format!("SUMMARY\n{s}\n"));
        }
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
pub fn validate_questions(
    questions: &serde_json::Value,
) -> Result<&serde_json::Value, AskFailure> {
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
    }
    Ok(questions)
}
'''

anchor = "// ---------------------------------------------------------------------------\n// The contract."
assert anchor in t, "test-module anchor not found"
t = t.replace(anchor, IMPL + "\n" + anchor, 1)
p.write_text(t)
print("  inserted compose_state + validate_questions + clip_stream")
