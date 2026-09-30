//! `xtask` — recurring workspace tasks.
//!
//! The point of having this crate (rather than remembering commands) is that the
//! gate is one command with one exit code, so a human and a CI job run the SAME
//! thing. It encodes the standard this workspace holds itself to:
//!
//! * `cargo test  --workspace` — unit contracts, offline, **zero model calls**
//! * `cargo clippy --workspace --all-targets` — **zero** warnings
//! * `cargo fmt    --workspace --check` — **zero** diffs
//!
//! "Zero, not below baseline" is the user's standing bar for Rust projects.
//! A gate that only reports is not a gate, so [`gate`] returns a nonzero exit
//! code when anything fails.

#![forbid(unsafe_code)]

use std::process::{Command, ExitCode};

/// Which steps the gate runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `cargo fmt --all --check`
    Fmt,
    /// `cargo clippy --workspace --all-targets -- -D warnings`
    Clippy,
    /// `cargo test --workspace`
    Test,
}

impl Step {
    /// Every step, in the order that fails fastest.
    pub const ALL: [Self; 3] = [Self::Fmt, Self::Clippy, Self::Test];

    /// A short human label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fmt => "fmt",
            Self::Clippy => "clippy",
            Self::Test => "test",
        }
    }

    /// The command and arguments this step runs.
    #[must_use]
    fn argv(self) -> (&'static str, Vec<&'static str>) {
        match self {
            Self::Fmt => ("cargo", vec!["fmt", "--all", "--check"]),
            Self::Clippy => (
                "cargo",
                vec![
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            Self::Test => ("cargo", vec!["test", "--workspace"]),
        }
    }
}

/// Run one step; return true when it passed.
///
/// `#[must_use]` because ignoring the result is exactly the bug this gate
/// exists to catch: a step that failed while the caller carried on.
#[must_use]
pub fn run(step: Step) -> bool {
    let (cmd, args) = step.argv();
    println!("=== {} : {} {} ===", step.label(), cmd, args.join(" "));
    // `Command::status` propagates the child's exit code, which is the whole
    // point: a failing test or a clippy warning must fail the gate.
    match Command::new(cmd).args(&args).status() {
        Ok(s) => {
            let ok = s.success();
            println!(
                "--- {} : {} ---",
                step.label(),
                if ok { "PASS" } else { "FAIL" }
            );
            ok
        }
        Err(e) => {
            // Distinguish "the step failed" from "we could not run the step" —
            // a missing cargo is an environment problem, not a code problem.
            eprintln!("--- {} : ERROR running {cmd}: {e} ---", step.label());
            false
        }
    }
}

/// The whole gate. Every step runs (no early exit) so one invocation reports
/// every problem, then the caller exits nonzero if any failed.
#[must_use]
pub fn gate() -> ExitCode {
    let failures: Vec<&str> = Step::ALL
        .into_iter()
        .filter(|s| !run(*s))
        .map(Step::label)
        .collect();
    if failures.is_empty() {
        println!("=== gate: PASS (all {} steps) ===", Step::ALL.len());
        ExitCode::SUCCESS
    } else {
        eprintln!("=== gate: FAIL ({}) ===", failures.join(", "));
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_steps_have_a_unique_label() {
        let mut labels: Vec<&str> = Step::ALL.iter().map(|s| s.label()).collect();
        let n = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "two steps share a label");
    }

    #[test]
    fn clippy_step_denies_warnings() {
        // The bar is zero warnings, so the gate must pass `-D warnings`; without
        // it clippy exits 0 on warnings and the gate silently passes.
        let (cmd, args) = Step::Clippy.argv();
        assert_eq!(cmd, "cargo");
        assert!(args.contains(&"-D"), "clippy must deny warnings: {args:?}");
        assert!(args.contains(&"warnings"));
    }

    #[test]
    fn fmt_step_checks_rather_than_rewrites() {
        // `--check` fails on a diff; plain `cargo fmt` would rewrite the tree and
        // always report success, which is not a gate.
        let (_, args) = Step::Fmt.argv();
        assert!(args.contains(&"--check"), "fmt must not rewrite: {args:?}");
    }

    #[test]
    fn every_step_runs_cargo() {
        for s in Step::ALL {
            assert_eq!(s.argv().0, "cargo");
        }
    }
}
