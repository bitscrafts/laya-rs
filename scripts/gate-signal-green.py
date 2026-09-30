#!/usr/bin/env python3
"""GREEN: the second, independent signal for command safety — plus honest validation.

WHY A SECOND SIGNAL, AND WHY NOT ONLY THIS ONE

Three measurement rounds agree that this head cannot classify command danger reliably:

    "is this safe?" as a noul            6/12   overlap
    six consequence phrasings                   all overlap, no threshold separates
    a `choice` over declared effects    13/18   overlap; missed `: > file` and `dd if=`

So the model ALONE must not be the control. But the source research is explicit that a static
list of forbidden strings must not be the only control either — `find . -delete` is "one most
engineers never think of", and an agent can route around a string match with a heredoc (that
is the recorded level-6 failure: blocked writing config/.env, it wrote the same file through
bash).

Therefore: TWO signals, and BOTH must pass.
  1. the model's verdict (generalises, but unreliable alone)
  2. a conservative pattern check (narrow, but exact where it matches)

This is what "treat a security gate as one signal, not the only control" means in practice.
Each covers the other's failure: the model catches command shapes nobody enumerated, the
pattern catches the idioms the model demonstrably misses.

The pattern list is deliberately about EFFECT, not command names: `: > f`, `dd of=`, `mv x
/dev/null`, `truncate -s 0`, `> /dev/sd`. It reports what it matched, so a refusal names its
reason instead of being an opaque block.
"""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-core/src/agentic.rs")
t = p.read_text()

IMPL = r'''
// ---------------------------------------------------------------------------
// The second signal: a conservative, exact-where-it-matches check
// ---------------------------------------------------------------------------

/// A destructive primitive found in a command, with the text that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        ("/dev/null", "discards data irrecoverably when used as a destination"),
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
'''

anchor = "// ---------------------------------------------------------------------------\n// The contract."
assert anchor in t
t = t.replace(anchor, IMPL + "\n" + anchor, 1)

# --- tests for the second signal ---
TESTS = r'''
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
        assert!(msg.contains("rm -rf") && msg.contains("recursively"), "{msg}");
    }
'''

anchor2 = "    // ---- the question block -------------------------------------------------"
assert anchor2 in t
t = t.replace(anchor2, TESTS + "\n" + anchor2, 1)
p.write_text(t)
print("  added destructive_signals + its tests")
