#!/usr/bin/env python3
"""RED for `ask` (L10): append the contract tests. No implementation yet, so this must fail
on missing ITEMS rather than on wrong behaviour — that is what makes it a real RED."""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-core/src/agentic.rs")

TESTS = r'''
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
    fn the_command_output_is_composed_FIRST_because_the_head_reads_a_prefix() {
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
            assert!(composed.contains(marker), "`{marker}` must appear: {composed}");
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
        let composed = compose_state(None, &[], Some("partial"), Some("PANIC: index 3"))
            .expect("composes");
        assert!(composed.contains("PANIC: index 3"), "{composed}");
    }

    #[test]
    fn empty_parts_are_omitted_rather_than_labelled_empty() {
        // An empty section is noise the head reads first. Absent parts must leave no marker.
        let composed = compose_state(None, &[], Some("ONLY"), None).expect("composes");
        assert_eq!(composed.trim(), "ONLY", "nothing but the output: {composed:?}");
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
    fn a_state_over_the_cap_is_REFUSED_not_truncated() {
        let big = "x".repeat(STATE_CAP_BYTES + 1);
        let err = compose_state(None, &[file("huge.txt", &big)], None, None)
            .expect_err("must refuse");
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
        let fits = "x".repeat(STATE_CAP_BYTES);
        assert!(
            compose_state(None, &[file("f.txt", &fits)], None, None).is_ok(),
            "a state of exactly the cap must compose"
        );
        let over = "x".repeat(STATE_CAP_BYTES + 1);
        assert!(
            compose_state(None, &[file("f.txt", &over)], None, None).is_err(),
            "one byte over must be refused"
        );
    }

    #[test]
    fn a_long_command_output_is_TRUNCATED_AND_SAYS_SO() {
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
        let ok = serde_json::json!({
            "is_bug": {"type": "noul", "instructions": "is this a bug?"},
            "kind": {"type": "choice", "options": ["a", "b"], "instructions": "which?"},
            "risk": {"type": "score", "criteria": {"0": "none", "2": "high"}}
        });
        let out = validate_questions(&ok).expect("accepts all three");
        assert_eq!(out, &ok, "the block must be forwarded unchanged");

        // Control: one bad question in an otherwise good block still refuses, and names it.
        let almost = serde_json::json!({
            "is_bug": {"type": "noul", "instructions": "is this a bug?"},
            "kind": {"type": "choice", "options": ["a", "b"], "instructions": "which?"},
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
    fn the_composed_state_carries_only_what_the_caller_gave() {
        // L10's promise is that the agent gets ANSWERS and never the files or the output.
        // That is the server's response shape; what this module must guarantee is narrower
        // and checkable: it composes exactly the inputs it was handed, and adds no state of
        // its own (no environment, no cwd, no file listing) that the caller did not ask for.
        let files = [file("one.txt", "CONTENT-ONE")];
        let composed =
            compose_state(Some("S"), &files, Some("O"), Some("E")).expect("composes");
        for leaked in ["/proc", "/etc", "std::env", "PATH=", "HOME=", "SHELL="] {
            assert!(
                !composed.contains(leaked),
                "composed state must not invent `{leaked}`: {composed}"
            );
        }
    }
}
'''

with p.open("a") as fh:
    fh.write(TESTS)
print("  appended the L10 contract tests (no implementation yet — must be RED)")
