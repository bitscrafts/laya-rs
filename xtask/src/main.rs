//! `cargo xtask <task>` — recurring workspace tasks.
//!
//! See the library crate for what the gate checks and why.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("gate") | None => xtask::gate(),
        Some(other) => {
            eprintln!("unknown task: {other}");
            eprintln!("usage: cargo xtask gate");
            ExitCode::FAILURE
        }
    }
}
