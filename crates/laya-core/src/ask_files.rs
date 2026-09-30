//! `ask_files` — the batched form of [`crate::ask_file`].
//!
//! ## The promise, and what this module can actually keep
//!
//! The tool schema advertises "globs expanded, pruned, capped, **asked in parallel**". Two
//! of those are measurable facts and one is not yet true; this module is written so the
//! difference is visible rather than assumed.
//!
//! **Measured 2026-09-30, on the live sidecar:**
//!
//! * The sidecar is **strictly serial**. It accepts one connection and serves it in a
//!   blocking loop (`scripts/laya-sidecar.py`, `serve()`), so two clients sending at once
//!   are queued, not parallelised. The tool therefore asks **sequentially** and says so —
//!   advertising a parallelism that does not exist is the defect class this project keeps
//!   removing. (The BACKBONE is 5.8x parallel; the single-process head is not.)
//! * The head **judges a fixed PREFIX of the state**. A signal placed at the start scored
//!   exactly `0.788` at 3,975 / 15,943 / 63,951 and 199,951 bytes — invariant across a 50x
//!   size range, which is only possible if the tail is not being read. A signal at the END
//!   survived at 984 bytes and was completely gone at 3,976 (scoring `0.087`, identical to
//!   the no-signal control `0.0873`). The exact boundary could NOT be pinned: the filler's
//!   own semantics move the answer, so the measurement is bounded, not exact.
//!
//! ## What follows for this module
//!
//! * Every result carries `bytes` (how much content went in) and, above a conservative
//!   threshold, a `warning` naming the risk. A verdict that silently covers the first page
//!   of a long file is worse than a refusal, because nothing in the answer says so.
//! * The cap is enforced **per file** and the TOTAL is capped too, so a glob matching 10,000
//!   files cannot become 10,000 model calls.
//! * Refusals are per-file and reported, not fatal: one unreadable path in a batch of 20
//!   must not discard the other 19 verdicts.

use std::path::PathBuf;

use crate::ask_file::{FileState, ReadFailure, Scope, read_for_question};

/// Above this many bytes of content, the verdict may cover only the beginning.
///
/// **Conservatively below the smallest observed failure**, not at a measured boundary: the
/// last size where a trailing signal was still seen was 984 bytes, and it was gone by
/// 3,976. 1,024 is therefore inside the range where the content is certainly read, and the
/// warning fires for anything larger rather than the tool pretending certainty it does not
/// have. An exact token boundary was not derivable — the filler's own semantics move the
/// answer — so this is a documented bound, not a threshold.
pub const FULL_ATTENTION_BYTES: usize = 1024;

/// The most files one `ask_files` call will judge.
///
/// Bounded because the head is serial: this is a latency budget expressed as a count. At the
/// measured ~0.5 s per decision, 64 files is already ~30 s — past that the caller should
/// batch explicitly rather than discover the cost.
pub const MAX_FILES: usize = 64;

/// One file's outcome in a batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileVerdict {
    /// The canonical path that was judged.
    pub path: String,
    /// The model's answer object for this file, when it was judged.
    pub answers: Option<serde_json::Value>,
    /// Why it was not judged, when it was not.
    pub failure: Option<String>,
    /// How many bytes of content were sent.
    pub bytes: usize,
    /// Set when `bytes` exceeds [`FULL_ATTENTION_BYTES`]: the verdict may cover only the
    /// beginning of the file.
    pub warning: Option<String>,
}

impl FileVerdict {
    /// Whether this file produced a verdict.
    #[must_use]
    pub const fn judged(&self) -> bool {
        self.answers.is_some()
    }
}

/// Expand the requested paths into a concrete, ordered, deduplicated file list.
///
/// Globs are expanded with `glob`. A path that matches nothing is kept as a literal so the
/// caller gets a per-file "no such file" rather than silence — a glob that expands to
/// nothing and a glob the caller mistyped look identical otherwise.
///
/// Deduplicated because two patterns commonly overlap (`src/*.rs` and `src/main.rs`), and
/// judging the same file twice pays the model twice for one answer.
#[must_use]
pub fn expand(paths: &[String], limit: usize) -> (Vec<PathBuf>, Vec<String>) {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut notes = Vec::new();
    let mut is_glob = false;

    for p in paths {
        is_glob |= p.contains('*') || p.contains('?') || p.contains('[');
        match glob::glob(p) {
            Ok(entries) => {
                let mut matched = false;
                for e in entries.flatten() {
                    matched = true;
                    if !out.contains(&e) {
                        out.push(e);
                    }
                }
                if !matched && !p.contains('*') && !p.contains('?') && !p.contains('[') {
                    out.push(PathBuf::from(p));
                }
            }
            Err(e) => notes.push(format!("pattern `{p}` is not a valid glob: {e}")),
        }
    }

    out.sort();
    if out.len() > limit {
        notes.push(format!(
            "{} paths matched; only the first {limit} will be judged (the head is serial, \
             so this is a latency bound — pass a tighter pattern to choose)",
            out.len()
        ));
        out.truncate(limit);
    }
    if is_glob {
        notes.push(format!("glob expanded to {} file(s)", out.len()));
    }
    (out, notes)
}

/// Judge one question across many files, sequentially, collecting per-file outcomes.
///
/// `ask` is the model call, injected so this function is testable without a sidecar — and
/// so the batching logic (which is what this module owns) is separable from the transport
/// (which it does not).
pub fn judge_files<F>(
    scope: &Scope,
    files: &[PathBuf],
    question: &str,
    mut ask: F,
) -> Vec<FileVerdict>
where
    F: FnMut(&FileState) -> Result<serde_json::Value, String>,
{
    files
        .iter()
        .map(|f| {
            let path_str = f.to_string_lossy().to_string();
            match read_for_question(scope, &path_str, question) {
                Ok(state) => {
                    let bytes = state.content.len();
                    let warning = (bytes > FULL_ATTENTION_BYTES).then(|| {
                        format!(
                            "content is {bytes} bytes; the head reads a PREFIX of the state \
                             (measured: a trailing signal is seen at {FULL_ATTENTION_BYTES} \
                             bytes and not beyond ~4 KB), so this verdict may describe only \
                             the beginning of the file"
                        )
                    });
                    match ask(&state) {
                        Ok(answers) => FileVerdict {
                            path: state.path,
                            answers: Some(answers),
                            failure: None,
                            bytes,
                            warning,
                        },
                        Err(e) => FileVerdict {
                            path: state.path,
                            answers: None,
                            failure: Some(e),
                            bytes,
                            warning,
                        },
                    }
                }
                // A per-file failure is reported, never fatal: one bad path in a batch of 20
                // must not discard the other 19 verdicts.
                Err(f) => FileVerdict {
                    path: path_str.clone(),
                    answers: None,
                    failure: Some(describe(&f)),
                    bytes: 0,
                    warning: None,
                },
            }
        })
        .collect()
}

/// Render a read failure for a batch entry.
fn describe(f: &ReadFailure) -> String {
    f.to_string()
}

/// Which files the verdicts say answered the question, best first.
///
/// `noul` is the belief, so "the files whose content answers this" is exactly the set with
/// `p >= threshold` — and sorting by `p` turns a batch of yes/no into a ranking, which is
/// what a caller actually wants ("which file mentions the thing"). The threshold comes from
/// the caller's policy rather than a literal here, so a config change moves it.
#[must_use]
pub fn pick(verdicts: &[FileVerdict], threshold: f64) -> Vec<(String, f64)> {
    let mut hits: Vec<(String, f64)> = verdicts
        .iter()
        .filter_map(|v| {
            let a = v.answers.as_ref()?;
            // Accept both the raw sidecar shape and the payload shape, so this works whether
            // it is fed `answers` or a wrapped payload.
            let entry = a.get("answer").unwrap_or(a);
            let p = entry
                .get("noul")
                .and_then(serde_json::Value::as_f64)
                .or_else(|| entry.get("p").and_then(serde_json::Value::as_f64))?;
            (p >= threshold).then(|| (v.path.clone(), p))
        })
        .collect();
    hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing
    )]

    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "laya-askfiles-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&p).expect("tmp");
            Self(p)
        }
        fn write(&self, name: &str, body: &str) -> PathBuf {
            let f = self.0.join(name);
            std::fs::write(&f, body).expect("write");
            f
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // ---- glob expansion ----------------------------------------------------

    #[test]
    fn a_glob_expands_to_matching_files_in_sorted_order() {
        let t = Tmp::new("glob");
        t.write("b.txt", "x");
        t.write("a.txt", "x");
        t.write("c.md", "x");
        let (files, _) = expand(&[format!("{}/*.txt", t.0.display())], MAX_FILES);
        let names: Vec<String> = files
            .iter()
            .map(|f| {
                f.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt"], "sorted: {names:?}");
    }

    #[test]
    fn overlapping_patterns_do_not_double_count_a_file() {
        // `src/*.rs` and the explicit path both match main.rs. Judging it twice pays the
        // model twice for one answer.
        let t = Tmp::new("dedupe");
        t.write("a.txt", "x");
        let a = t.0.join("a.txt").display().to_string();
        let (files, _) = expand(&[format!("{}/*.txt", t.0.display()), a], MAX_FILES);
        assert_eq!(files.len(), 1, "duplicate: {files:?}");
    }

    #[test]
    fn a_pattern_matching_nothing_keeps_the_literal_so_the_caller_gets_an_error() {
        // Otherwise "glob matched nothing" and "you typed a wrong path" look identical.
        let (files, _) = expand(
            &["/tmp/laya-definitely-no-such-file-3f9a.txt".into()],
            MAX_FILES,
        );
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn a_wide_glob_is_capped_and_says_so() {
        // A silent cap would report "judged 64 files" with no hint that 10,000 matched.
        let t = Tmp::new("cap");
        for i in 0..5 {
            t.write(&format!("f{i}.txt"), "x");
        }
        let (files, notes) = expand(&[format!("{}/*.txt", t.0.display())], 3);
        assert_eq!(files.len(), 3, "must truncate to the limit");
        assert!(
            notes.iter().any(|n| n.contains("only the first 3")),
            "the truncation must be reported: {notes:?}"
        );
    }

    #[test]
    fn an_invalid_glob_is_reported_not_ignored() {
        let (_, notes) = expand(&["[unclosed".into()], MAX_FILES);
        assert!(
            notes.iter().any(|n| n.contains("not a valid glob")),
            "{notes:?}"
        );
    }

    // ---- per-file verdicts -------------------------------------------------

    #[test]
    fn every_file_gets_a_verdict_even_when_one_fails() {
        // One unreadable path in a batch must not discard the other verdicts.
        let t = Tmp::new("batch");
        t.write("a.txt", "hello");
        let missing = t.0.join("gone.txt");
        let scope = Scope::new(vec![t.0.clone()]);
        let files = vec![t.0.join("a.txt"), missing];

        let verdicts = judge_files(&scope, &files, "q?", |_| {
            Ok(serde_json::json!({"answer": {"noul": 0.9}}))
        });
        assert_eq!(verdicts.len(), 2);
        assert!(verdicts[0].judged(), "the good file must be judged");
        assert!(!verdicts[1].judged(), "the missing file must not be");
        assert!(
            verdicts[1].failure.is_some(),
            "and must carry the reason: {:?}",
            verdicts[1]
        );
    }

    #[test]
    fn a_large_file_is_judged_but_flagged_as_possibly_partial() {
        // THE honesty rule. The verdict is still produced — refusing would be its own
        // failure — but the caller is told the model may only have read the start.
        let t = Tmp::new("large");
        t.write("big.txt", &"word ".repeat(FULL_ATTENTION_BYTES));
        let scope = Scope::new(vec![t.0.clone()]);
        let verdicts = judge_files(&scope, &[t.0.join("big.txt")], "q?", |_| {
            Ok(serde_json::json!({"answer": {"noul": 0.8}}))
        });
        let v = &verdicts[0];
        assert!(v.judged(), "it must still be judged");
        assert!(
            v.bytes > FULL_ATTENTION_BYTES,
            "bytes recorded: {}",
            v.bytes
        );
        let w = v.warning.clone().unwrap_or_default();
        assert!(w.contains("PREFIX"), "the warning must name the risk: {w}");
    }

    #[test]
    fn a_small_file_carries_no_warning() {
        // The CONTROL: without this, a warning attached unconditionally would pass the test
        // above while telling every caller to distrust every answer.
        let t = Tmp::new("small");
        t.write("s.txt", "short content");
        let scope = Scope::new(vec![t.0.clone()]);
        let verdicts = judge_files(&scope, &[t.0.join("s.txt")], "q?", |_| {
            Ok(serde_json::json!({"answer": {"noul": 0.8}}))
        });
        assert!(verdicts[0].warning.is_none(), "{:?}", verdicts[0]);
    }

    #[test]
    fn an_out_of_scope_file_in_a_batch_is_refused_and_the_others_are_not() {
        let t = Tmp::new("scope");
        let inside = t.write("in.txt", "hi");
        let outside =
            t.0.parent()
                .unwrap_or(&t.0)
                .join("laya-askfiles-outside.txt");
        std::fs::write(&outside, "x").ok();

        let scope = Scope::new(vec![t.0.clone()]);
        let verdicts = judge_files(&scope, &[inside, outside.clone()], "q?", |_| {
            Ok(serde_json::json!({"answer": {"noul": 0.9}}))
        });
        assert!(verdicts[0].judged());
        let msg = verdicts[1].failure.clone().unwrap_or_default();
        assert!(msg.contains("scope"), "out-of-scope reason: {msg}");
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn a_model_error_on_one_file_does_not_lose_the_others() {
        let t = Tmp::new("modelerr");
        t.write("a.txt", "1");
        t.write("b.txt", "2");
        let scope = Scope::new(vec![t.0.clone()]);
        let mut n = 0;
        let verdicts = judge_files(
            &scope,
            &[t.0.join("a.txt"), t.0.join("b.txt")],
            "q?",
            |_| {
                n += 1;
                if n == 1 {
                    Err("sidecar exploded".into())
                } else {
                    Ok(serde_json::json!({"answer": {"noul": 0.7}}))
                }
            },
        );
        assert!(
            !verdicts[0].judged() && verdicts[1].judged(),
            "{verdicts:?}"
        );
    }

    // ---- pick: the ranking -------------------------------------------------

    #[test]
    fn pick_returns_only_files_at_or_above_the_threshold_best_first() {
        let mk = |p: &str, noul: f64| FileVerdict {
            path: p.into(),
            answers: Some(serde_json::json!({"answer": {"noul": noul}})),
            failure: None,
            bytes: 10,
            warning: None,
        };
        let v = vec![mk("a", 0.2), mk("b", 0.95), mk("c", 0.7)];
        let hits = pick(&v, 0.5);
        assert_eq!(
            hits,
            vec![("b".to_string(), 0.95), ("c".to_string(), 0.7)],
            "above threshold, descending"
        );
    }

    #[test]
    fn pick_excludes_files_that_were_not_judged() {
        let v = vec![FileVerdict {
            path: "a".into(),
            answers: None,
            failure: Some("nope".into()),
            bytes: 0,
            warning: None,
        }];
        assert!(pick(&v, 0.5).is_empty(), "an unjudged file is not a hit");
    }

    #[test]
    fn the_threshold_is_inclusive_at_the_boundary() {
        let v = vec![FileVerdict {
            path: "a".into(),
            answers: Some(serde_json::json!({"answer": {"noul": 0.5}})),
            failure: None,
            bytes: 1,
            warning: None,
        }];
        assert_eq!(pick(&v, 0.5).len(), 1, ">= threshold counts as a hit");
    }
}
