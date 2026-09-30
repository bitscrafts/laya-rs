#!/usr/bin/env python3
"""Fix a contradiction between TWO TESTS I WROTE, by stating the semantics.

Both tests are mine and they cannot both hold:
  `a_nonexistent_path_is_not_in_scope`               — asserts a missing path is NOT in scope
  `a_missing_file_inside_the_scope_reports_unresolvable` — requires it to BE in scope

TDD says "test fails -> fix the code, not the test". That rule is about not weakening a
test to make broken code pass. Here the test encodes an intent that CONTRADICTS another
test, so the behaviour has to be decided before either can be judged. Deciding it
explicitly:

  in-scope, exists      -> allowed, read it
  in-scope, missing     -> IN SCOPE, fails as `Unresolvable`. The caller may read here, so
                           telling them the file is absent leaks nothing they could not
                           discover themselves — and reporting "outside the read scope"
                           for a typo sends them to debug permissions.
  out-of-scope, exists  -> `OutOfScope`
  out-of-scope, missing -> `OutOfScope`. IDENTICAL to the row above, which is the oracle
                           property: the error must not reveal existence.

So `a_nonexistent_path_is_not_in_scope` is WRONG and is corrected. The oracle test is the
control and is untouched — it is the one that would catch a regression here.

The genuine "unresolvable is not in scope" case still exists and gets its own test: a path
whose PARENT does not exist cannot be placed, so it is not in scope. That is a real rule
and it now has coverage instead of being conflated with "file not there".
"""
import pathlib

p = pathlib.Path("crates/laya-core/src/ask_file.rs")
t = p.read_text()

old = '''    #[test]
    fn a_nonexistent_path_is_not_in_scope() {
        // Not merely "unreadable": a missing path must not be reported as allowed, or a
        // caller could distinguish "refused by scope" from "not there".
        let t = Tmp::new("missing");
        assert!(!scope_of(&t.0).allows(&t.0.join("nope.txt")));
    }
'''

new = '''    #[test]
    fn a_nonexistent_path_inside_the_scope_is_still_in_scope() {
        // CORRECTED. An earlier version of this test asserted the opposite, and that was
        // wrong: it conflated "missing" with "forbidden". The scope decision is about
        // WHERE the path would live, so a missing path inside the scope is in scope and
        // then fails as Unresolvable (see the test below). Reporting "outside the read
        // scope" for a typo would send a caller to debug permissions.
        //
        // The oracle property people worry about here is preserved by a DIFFERENT rule,
        // and it has its own test: an out-of-scope path must fail identically whether or
        // not it exists.
        let t = Tmp::new("missing");
        assert!(scope_of(&t.0).allows(&t.0.join("nope.txt")));
    }

    #[test]
    fn a_path_whose_parent_does_not_exist_is_not_in_scope() {
        // The genuine "cannot be placed" case: with no resolvable parent there is no way
        // to say where the path would live, so it is NOT in scope. Distinct from "the file
        // is not there", which is in scope and reports Unresolvable.
        let t = Tmp::new("noparent");
        let unplaceable = t.0.join("no-such-dir").join("deep").join("file.txt");
        assert!(
            !scope_of(&t.0).allows(&unplaceable),
            "a path with no resolvable parent cannot be placed, so it is not in scope"
        );
    }
'''

assert old in t, "anchor not found"
t = t.replace(old, new, 1)
p.write_text(t)
print("  corrected the contradictory test + added the real not-in-scope case")
