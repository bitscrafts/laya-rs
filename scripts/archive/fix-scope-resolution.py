#!/usr/bin/env python3
"""GREEN, part 2: resolve a path for the scope check WITHOUT requiring the file to exist.

THE CONFLICT THE TEST FOUND (a real one, not a test bug):

  * an out-of-scope path must give the SAME error whether the file exists or not, or the
    error becomes an existence oracle (that is `an_out_of_scope_refusal_does_not_reveal_whether_the_file_exists`);
  * an in-scope MISSING file must report `Unresolvable`, because inside the scope the
    caller is entitled to know it is not there.

The naive implementation — canonicalise the candidate — breaks the second, because a path
that does not exist cannot be canonicalised, so it is judged "not in scope". Observed:

  `.../nope.txt` is outside the configured read scope

which is the WRONG reason and would send a caller hunting for a permissions problem.

THE FIX: when the full path cannot be canonicalised, canonicalise its PARENT and
containment-test the parent + file name. That answers "would this path be inside the scope
if it existed?" — which is exactly the question, and it keeps the oracle closed because a
missing out-of-scope path fails at the same step as an existing one.
"""
import pathlib

p = pathlib.Path("crates/laya-core/src/ask_file.rs")
t = p.read_text()


def sub(old, new):
    global t
    if old not in t:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:200]}")
    t = t.replace(old, new, 1)


# ---- the resolution helper, shared by `allows` and the reader ---------------
sub('''impl Scope {
    /// A scope restricted to `roots`.''',
'''/// Resolve a path for a scope decision, tolerating a path that does not exist yet.
///
/// Returns `None` when the path cannot be placed at all.
///
/// Two steps, and the second is the point: canonicalise the whole path when it exists,
/// otherwise canonicalise the **parent** and re-attach the file name. Canonicalising only
/// the candidate would answer "is this INSIDE the scope" — but the question is "WOULD it
/// be", because a missing file inside the scope must be reported as missing, not as
/// forbidden. Getting that wrong makes `ask_file` say "outside the read scope" for a
/// typo, which sends the caller to debug permissions instead of their path.
fn resolve_for_scope(candidate: &Path) -> Option<PathBuf> {
    if let Ok(full) = candidate.canonicalize() {
        return Some(full);
    }
    // Not present (or unresolvable): place it by its parent, so the scope decision is
    // still made on where the path WOULD live.
    let parent = candidate.parent()?;
    let name = candidate.file_name()?;
    parent.canonicalize().ok().map(|p| p.join(name))
}

impl Scope {
    /// A scope restricted to `roots`.''')

# ---- allows() now uses the resolver ----------------------------------------
sub('''    /// Whether `candidate` is inside the scope, by canonicalised containment.
    ///
    /// Canonicalises **both** sides, which is what makes this containment rather than a
    /// string check: a prefix test would accept `/work/../etc/passwd` for a `/work` root.
    /// A path that cannot be canonicalised (it does not exist) is NOT in scope — a
    /// non-existent path must not be reported as in-scope, or the refusal for a real
    /// file elsewhere could be distinguished from one for a missing file.
    #[must_use]
    pub fn allows(&self, candidate: &Path) -> bool {
        let Ok(target) = candidate.canonicalize() else {
            return false;
        };
        self.roots
            .iter()
            .filter_map(|r| r.canonicalize().ok())
            .any(|root| target.starts_with(&root))
    }''',
'''    /// Whether `candidate` is inside the scope, by canonicalised containment.
    ///
    /// Canonicalises **both** sides, which is what makes this containment rather than a
    /// string check: a prefix test would accept `/work/../etc/passwd` for a `/work` root.
    ///
    /// A path that does not exist is placed by its parent (see [`resolve_for_scope`]), so
    /// a missing file inside the scope is in scope — and then fails as *missing* rather
    /// than as *forbidden*. An unresolvable path is NOT in scope.
    #[must_use]
    pub fn allows(&self, candidate: &Path) -> bool {
        let Some(target) = resolve_for_scope(candidate) else {
            return false;
        };
        self.roots
            .iter()
            .filter_map(|r| r.canonicalize().ok())
            .any(|root| target.starts_with(&root))
    }''')

# ---- the reader reuses the same resolution (one implementation, no drift) --
sub('''    let resolved = candidate
        .canonicalize()
        .map_err(|e| ReadFailure::Unresolvable {
            path: path.to_string(),
            why: e.to_string(),
        })?;''',
'''    // Reuse the SAME resolution the scope check used, so the path that was judged is the
    // path that is read. Two different resolutions is how a check and its subject drift.
    let resolved = resolve_for_scope(candidate).ok_or_else(|| ReadFailure::Unresolvable {
        path: path.to_string(),
        why: "the path could not be resolved".to_string(),
    })?;''')

p.write_text(t)
print("  parent-based resolution added; allows() and the reader share it")
