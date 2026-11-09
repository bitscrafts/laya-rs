#!/usr/bin/env python3
"""REFACTOR: clear the clippy findings in the new module. Every one is a real fix.

  * `const fn` on `Scope::new`.
  * `map(..).unwrap_or(..)` -> `.ok().unwrap_or(0)` shape for the temp-dir clock.
  * `indexing_slicing` in the TESTS: `v["data"]["content"]` can panic, and the workspace
    denies it. Rewritten as `get` chains — which also makes a failure report WHICH key was
    missing instead of panicking inside the assert.
"""
import pathlib
import re

p = pathlib.Path("crates/laya-core/src/ask_file.rs")
t = p.read_text()

# ---- const fn ---------------------------------------------------------------
t = t.replace(
    "    pub fn new(roots: Vec<PathBuf>) -> Self {\n        Self { roots }\n    }",
    "    pub const fn new(roots: Vec<PathBuf>) -> Self {\n        Self { roots }\n    }",
    1,
)

# ---- the temp-dir clock -----------------------------------------------------
t = t.replace(
    """            let p = std::env::temp_dir().join(format!(
                "laya-askfile-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));""",
    """            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map_or(0, |d| d.as_nanos());
            let p = std::env::temp_dir().join(format!(
                "laya-askfile-{tag}-{}-{nanos}",
                std::process::id()
            ));""",
)

# ---- de-index the tests -----------------------------------------------------
t = t.replace(
    """        let v = st.to_state();
        assert!(
            v["data"]["content"].is_string(),
            "content lives under `data`"
        );
        let note = v["data"]["note"].as_str().unwrap_or_default();
        assert!(
            note.contains("UNTRUSTED"),
            "the content must be labelled untrusted: {note}"
        );
        assert!(v["question"].is_string(), "the question is separate");""",
    """        let v = st.to_state();
        let data = v.get("data");
        assert!(
            data.and_then(|d| d.get("content")).is_some_and(|c| c.is_string()),
            "content lives under `data`: {v}"
        );
        let note = data
            .and_then(|d| d.get("note"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            note.contains("UNTRUSTED"),
            "the content must be labelled untrusted: {note}"
        );
        assert!(
            v.get("question").is_some_and(|q| q.is_string()),
            "the question is separate: {v}"
        );""",
)

t = t.replace(
    """        let v = st.to_state();
        let q = v["question"].as_str().unwrap_or_default();
        assert!(
            !q.contains("developer mode"),
            "file content leaked into the question: {q}"
        );""",
    """        let v = st.to_state();
        let q = v
            .get("question")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            !q.contains("developer mode"),
            "file content leaked into the question: {q}"
        );""",
)

t = t.replace(
    """        let spec = FileState::question_spec();
        assert_eq!(spec["answer"]["type"], "noul");
        assert!(
            spec["answer"]["instructions"].is_string(),
            "the question carries its own instructions"
        );""",
    """        let spec = FileState::question_spec();
        let answer = spec.get("answer");
        assert_eq!(
            answer.and_then(|a| a.get("type")).and_then(serde_json::Value::as_str),
            Some("noul")
        );
        assert!(
            answer
                .and_then(|a| a.get("instructions"))
                .is_some_and(|i| i.is_string()),
            "the question carries its own instructions: {spec}"
        );""",
)

p.write_text(t)
print("  refactor applied")
