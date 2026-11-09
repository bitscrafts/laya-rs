#!/usr/bin/env python3
"""RED for the ask_file HANDLER, and remove the duplicated security check.

TWO PROBLEMS FOUND BEFORE WRITING ANY IMPLEMENTATION:

1. **`ReadScope` in `laya-mcp` duplicates the rule now in `laya_core::ask_file::Scope`.**
   Two implementations of a containment check is precisely the drift risk this project's
   rules exist to prevent — and the laya-mcp copy has NO parent-resolution, so it would
   disagree with the core one about missing files. The server copy is DELETED and the
   server uses the core one. One rule, one place.

2. **`RawArgs` has no `question` field**, though the tool schema declares one and
   `required` lists it. So `ask_file` could never receive its question. Added.

Then the handler tests (RED — they will fail to compile, which is the failure).
"""
import pathlib

p = pathlib.Path("crates/laya-mcp/src/lib.rs")
t = p.read_text()


def sub(old, new):
    global t
    if old not in t:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:220]}")
    t = t.replace(old, new, 1)


# ---- 1. RawArgs gains `question` and `paths` -------------------------------
sub('''    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    op: Option<String>,''',
'''    #[serde(default)]
    path: Option<String>,
    /// The question `ask_file` asks about the file. Separate from `path` on purpose:
    /// keeping them in distinct fields is what stops content being concatenated into the
    /// question, which is how a file's text could become an instruction.
    #[serde(default)]
    question: Option<String>,
    /// The paths `ask_files` asks about.
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    op: Option<String>,''')

# ---- 2. delete the duplicate scope ----------------------------------------
start = t.find("/// The `ask_file` read scope: paths a file tool may read.")
assert start != -1, "duplicate scope doc not found"
end = t.find("#[cfg(test)]", start)
assert end != -1, "could not find the end of the duplicate scope block"
removed = t[start:end]
assert "pub struct ReadScope" in removed and "fn allows" in removed, "unexpected block"
sub(removed,
'''// NOTE: `ask_file`/`ask_files` do NOT define their own read scope here.
//
// The containment rule lives in `laya_core::ask_file::Scope` — ONE implementation, so it
// cannot drift, and so the rule that decides a refusal is testable without a server.
// A duplicate `ReadScope` used to sit at this spot. It differed from the core rule in a
// way that mattered: it had no parent-resolution, so it could not place a path that does
// not exist, and would have reported a missing in-scope file as "outside the scope".

/// The env var listing the roots `ask_file`/`ask_files` may read, colon-separated.
///
/// **There is deliberately no default of `/`.** With the variable unset, the scope is the
/// process's working directory — a real, bounded place — and the resolved roots are
/// logged at startup so the boundary in force is visible rather than inferred. A tool that
/// silently defaults to the whole filesystem is a tool nobody can reason about.
pub const READ_ROOTS_ENV: &str = "LAYA_READ_ROOTS";

/// Resolve the configured read scope.
///
/// Colon-separated (`:`), matching `PATH`. An entry that does not exist is dropped and
/// REPORTED, because a silently-dropped root looks like a scope that refuses everything
/// for no reason.
#[must_use]
pub fn resolve_read_scope(raw: Option<&str>) -> (laya_core::ask_file::Scope, Vec<String>) {
    let mut notes = Vec::new();
    let roots: Vec<std::path::PathBuf> = match raw {
        Some(v) if !v.trim().is_empty() => v
            .split(':')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let p = std::path::PathBuf::from(s);
                if p.is_dir() {
                    Some(p)
                } else {
                    notes.push(format!("read root `{s}` is not a directory; ignored"));
                    None
                }
            })
            .collect(),
        _ => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            notes.push(format!(
                "{READ_ROOTS_ENV} is unset, so the read scope is the working directory: {}",
                cwd.display()
            ));
            vec![cwd]
        }
    };
    (laya_core::ask_file::Scope::new(roots), notes)
}

''')

# ---- 3. ServerState carries the scope (built once, not per call) ------------
sub('''    /// How to reach the decision head. The head needs torch, so it is a separate
    /// pinned process — see `laya_backbone::sidecar`.
    pub sidecar: laya_backbone::Sidecar,
}''',
'''    /// How to reach the decision head. The head needs torch, so it is a separate
    /// pinned process — see `laya_backbone::sidecar`.
    pub sidecar: laya_backbone::Sidecar,
    /// The roots `ask_file`/`ask_files` may read. Resolved ONCE at construction, so the
    /// boundary cannot change between two calls that appear to use the same server.
    pub read_scope: laya_core::ask_file::Scope,
}''')

sub('''            readiness: std::sync::Mutex::new(ReadinessCache::new()),
            sidecar: socket,
        }
    }''',
'''            readiness: std::sync::Mutex::new(ReadinessCache::new()),
            sidecar: socket,
            read_scope: laya_core::ask_file::Scope::new(Vec::new()),
        }
    }

    /// Set the read scope (`ask_file`/`ask_files`).
    #[must_use]
    pub fn with_read_scope(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.read_scope = laya_core::ask_file::Scope::new(paths);
        self
    }''')

p.write_text(t)
print("  RawArgs: +question +paths | duplicate ReadScope removed | scope on ServerState")

# ---- 4. tests for the handler (RED) ---------------------------------------
tests = pathlib.Path("crates/laya-mcp/src/lib.rs")
tt = tests.read_text()
anchor = "    #[test]\n    fn the_server_does_not_use_a_work_queue() {"
assert anchor in tt, "test anchor missing"

new_tests = '''    // ---- ask_file: the handler --------------------------------------------

    fn args_ask_file(path: &str, question: &str) -> RawArgs {
        RawArgs {
            state: None,
            action: None,
            irreversible: None,
            path: Some(path.to_string()),
            question: Some(question.to_string()),
            paths: None,
            op: None,
            questions: None,
        }
    }

    #[test]
    fn ask_file_needs_a_path_and_a_question_and_says_which_is_missing() {
        for (a, want) in [
            (args_with(None, None), "path"),
            (args_ask_file("/tmp/x", ""), "question"),
        ] {
            let out = handle("ask_file", &ready(), &a, &test_state());
            let msg = format!("{:?}", out.as_ref().err().map_or_else(String::new, |e| format!("{e:?}")));
            assert!(out.is_err(), "ask_file must require its arguments");
            assert!(msg.contains(want), "must name `{want}`: {msg}");
        }
    }

    #[test]
    fn ask_file_refuses_an_out_of_scope_path_even_when_the_head_is_up() {
        // ORDER: the scope is enforced before the model is consulted. A refusal must not
        // depend on a network round trip, and must not be reachable by asking nicely.
        let t = ServerState::new("http://x:1").with_read_scope(vec![std::path::PathBuf::from("/tmp")]);
        let a = args_ask_file("/etc/hostname", "what is this?");
        let out = handle("ask_file", &ready(), &a, &t);
        let msg = format!("{:?}", out.as_ref().err().map_or_else(String::new, |e| format!("{e:?}")));
        assert!(out.is_err(), "an out-of-scope path must be refused");
        assert!(msg.contains("read scope"), "the reason must name the scope: {msg}");
    }

    #[test]
    fn ask_file_with_an_empty_scope_refuses_everything() {
        // FAIL CLOSED. `test_state()` has no roots configured, so nothing is readable.
        // A default-open scope is how "least privilege" becomes a slogan.
        let a = args_ask_file("/tmp/whatever.txt", "q");
        let out = handle("ask_file", &ready(), &a, &test_state());
        assert!(out.is_err(), "with no roots configured, ask_file must refuse");
    }

    #[test]
    fn resolve_read_scope_drops_a_bad_root_and_reports_it() {
        let (scope, notes) =
            resolve_read_scope(Some("/tmp:/definitely/not/a/dir/xyzzy:/usr"));
        assert_eq!(scope.roots().len(), 2, "the bogus root must be dropped");
        assert!(
            notes.iter().any(|n| n.contains("xyzzy")),
            "a dropped root must be REPORTED, not silent: {notes:?}"
        );
    }

    #[test]
    fn resolve_read_scope_without_the_env_var_defaults_to_the_cwd_and_says_so() {
        let (scope, notes) = resolve_read_scope(None);
        assert_eq!(scope.roots().len(), 1);
        assert!(
            notes.iter().any(|n| n.contains("unset")),
            "the default must be visible, not inferred: {notes:?}"
        );
        assert!(
            !scope.roots().iter().any(|r| r == std::path::Path::new("/")),
            "the default must NEVER be the filesystem root"
        );
    }

'''

tt = tt.replace(anchor, new_tests + anchor, 1)
tests.write_text(tt)
print("  ask_file handler tests added (RED)")
