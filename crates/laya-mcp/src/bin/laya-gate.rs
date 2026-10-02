//! `laya-gate` — one command judged, one verdict printed, one ledger row written.
//!
//! # Why this binary exists
//!
//! The gate was reachable only over MCP, and it was used three times in the life of this
//! installation — measured 2026-10-02: **3 calls out of 63,886 tool calls**. A layer that
//! must be *used* needs a caller on the path where work actually happens, and a shell hook
//! cannot speak MCP: it needs one process that takes a command, answers, and exits. This
//! is that process.
//!
//! The judgement is NOT reimplemented here. It calls [`laya_mcp::judge_command`] — the
//! same function the MCP `gate` tool calls — so an MCP call and a hook call are one
//! decision. A second gate would be the defect class this project keeps finding: a rule
//! stated in two places, disagreeing.
//!
//! # Contract
//!
//! ```text
//! argv  : <command> [--irreversible] [--run-id ID] [--node-id ID] [--no-ledger]
//! stdout: the verdict payload as one JSON object
//! stderr: one summary line, in the same shape the seeing log uses
//! exit  : 0 allow | 1 ask | 2 deny | 3 unjudgeable
//! ```
//!
//! **Exit codes carry the verdict**, so a caller does not have to parse JSON to decide
//! whether to block. `ask` is deliberately not an error: a verdict of `ask` is the gate
//! doing its job, and a caller that treats it as a refusal stops using the gate.
//!
//! Exit 3 means no judgement was possible — the head was unreachable, or its answer could
//! not be read. Distinct from `ask` on purpose: `ask` is a judgement about the command,
//! exit 3 is the absence of a judgement, and only the caller knows what an unjudged
//! action should cost in its context.
//!
//! `--no-ledger` skips recording, for a caller that is already recording.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use laya_mcp::{RawArgs, ServerState};

/// Exit codes, named so `main` reads as the contract above.
const EXIT_ALLOW: u8 = 0;
const EXIT_ASK: u8 = 1;
const EXIT_DENY: u8 = 2;
const EXIT_UNJUDGEABLE: u8 = 3;

const USAGE: &str = "usage: laya-gate <command> [--irreversible] [--run-id ID] [--node-id ID] \
                     [--no-ledger]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut command: Option<String> = None;
    let mut irreversible = false;
    let mut run_id: Option<String> = None;
    let mut node_id: Option<String> = None;
    let mut ledger = true;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--irreversible" => irreversible = true,
            "--no-ledger" => ledger = false,
            "--run-id" => run_id = args.next(),
            "--node-id" => node_id = args.next(),
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                if command.is_some() {
                    eprintln!("laya-gate: unexpected extra argument {other:?}\n{USAGE}");
                    return ExitCode::from(EXIT_UNJUDGEABLE);
                }
                command = Some(other.to_string());
            }
        }
    }

    let Some(action) = command else {
        eprintln!("laya-gate: a command is required\n{USAGE}");
        return ExitCode::from(EXIT_UNJUDGEABLE);
    };

    // The SAME entry point the MCP `gate` tool uses. Nothing about the judgement is
    // duplicated here.
    let mut state = ServerState::new(
        std::env::var("LAYA_URL").unwrap_or_else(|_| laya_backbone::DEFAULT_ENDPOINT.to_string()),
    );
    if !ledger {
        // Drop the directory BEFORE judging: `gate_payload` records through whatever
        // directory the state holds, so this is what makes `--no-ledger` a pure judgement
        // rather than a judgement plus a row the caller did not ask for.
        state = state.with_ledger_dir(None);
    }
    // Only `action` is cloned: it is still needed for the summary line below. The ids are
    // moved, since nothing reads them after the judgement.
    let args_for_gate = RawArgs {
        action: Some(action.clone()),
        irreversible: Some(irreversible),
        run_id,
        node_id,
        ..RawArgs::default()
    };

    let started = std::time::Instant::now();
    // `gate_payload` is the shared judgement; it records internally, and tells us how long
    // it took so this binary can print the same number the ledger holds.
    let outcome = match laya_mcp::gate_payload(&args_for_gate, &state) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("laya-gate: could not judge the command: {e}");
            return ExitCode::from(EXIT_UNJUDGEABLE);
        }
    };
    let payload = outcome.payload;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let decision = payload
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ask")
        .to_string();
    let reason = payload
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let signals = payload
        .get("destructive_signals")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);

    match serde_json::to_string(&payload) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("laya-gate: could not serialise the verdict: {e}"),
    }

    // One summary line, in the shape the seeing log uses, so a log reader sees the same
    // fields whether the verdict came from here or from the MCP tool.
    let label = if reason.as_deref() == Some("sidecar-unavailable") {
        "UNJUDGEABLE"
    } else {
        decision.as_str()
    };
    eprintln!(
        "laya-gate: {label} [{decision}] signals={signals} latency={latency_ms}ms reason={} \
         cmd={}",
        reason.as_deref().unwrap_or("-"),
        truncate(&action, 80)
    );

    if reason.as_deref() == Some("sidecar-unavailable") {
        return ExitCode::from(EXIT_UNJUDGEABLE);
    }
    if let Some(err) = payload
        .get("laya_error")
        .and_then(serde_json::Value::as_str)
    {
        eprintln!("laya-gate: the head could not be read: {err}");
        return ExitCode::from(EXIT_UNJUDGEABLE);
    }
    match decision.as_str() {
        "allow" => ExitCode::from(EXIT_ALLOW),
        "deny" => ExitCode::from(EXIT_DENY),
        _ => ExitCode::from(EXIT_ASK),
    }
}

/// Truncate on a character boundary, so a multi-byte character is never split.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
