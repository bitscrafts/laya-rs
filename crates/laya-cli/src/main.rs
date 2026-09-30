//! `laya` — the terminal runner.
//!
//! Implemented: `laya doctor`, the readiness check (spec 05). Exits **0** when Laya
//! is usable and **1** when it is not, printing the actionable diagnostic on stderr.
//!
//! Not implemented: `decide`, `ledger`. Those print that they are unimplemented and
//! exit non-zero — a command that appears to work while doing nothing is the class
//! of defect this project's rules exist to prevent, and `doctor` is the one command
//! whose entire job is to report state accurately.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use laya_cli::{DEFAULT_ENDPOINT, PROBE_TIMEOUT_MS};
use laya_core::readiness::{ProbeOutcome, Readiness};

/// Build a bounded HTTP agent.
///
/// The timeout is the point: a hung server must become a REPORTED timeout rather
/// than a command that never returns.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(PROBE_TIMEOUT_MS)))
        .build()
        .new_agent()
}

/// Probe `/health` and return what it concluded.
fn probe_health(endpoint: &str) -> ProbeOutcome {
    let url = format!("{}/health", endpoint.trim_end_matches('/'));
    match agent().get(&url).call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            laya_backbone::from_health_response(status, &body)
        }
        Err(e) => laya_backbone::from_transport_error(&e.to_string()),
    }
}

/// Probe `/v1/models` and check the expected model is served.
fn probe_models(endpoint: &str) -> ProbeOutcome {
    let url = format!("{}/v1/models", endpoint.trim_end_matches('/'));
    match agent().get(&url).call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            if status != 200 {
                return ProbeOutcome::BadStatus { status };
            }
            laya_backbone::parse_models(&body)
        }
        Err(e) => laya_backbone::from_transport_error(&e.to_string()),
    }
}

/// The sidecar socket the head probe uses.
///
/// Same env var the server honours, so `doctor` and the server can never disagree
/// about where the head is: a doctor that checks a different socket than the server
/// uses would be a check on the wrong artefact.
fn head_socket() -> std::path::PathBuf {
    std::env::var("LAYA_SIDECAR_SOCKET").map_or_else(
        |_| std::path::PathBuf::from(laya_backbone::DEFAULT_SOCKET),
        std::path::PathBuf::from,
    )
}

/// `laya doctor [--url URL]`
fn doctor(endpoint: &str) -> ExitCode {
    // Three independent things must hold, and the order is the order they are worth
    // checking: two endpoints (a server answering /health but serving no model is not
    // usable) and then the head.
    //
    // The head check is the one that was MISSING, and its absence was the worst kind
    // of defect: `doctor` printed READY, exit 0, while `decide` could not answer a
    // single question because the sidecar was dead. A check that passes while the live
    // path is broken certifies the bug rather than catching it.
    let health = probe_health(endpoint);
    let outcome = if matches!(health, ProbeOutcome::Healthy) {
        probe_models(endpoint)
    } else {
        health
    };
    let backbone = if matches!(outcome, ProbeOutcome::Healthy) {
        laya_backbone::probe_head(&head_socket())
    } else {
        outcome
    };

    match laya_core::readiness::judge(endpoint, &backbone) {
        Readiness::Ready => {
            let socket = head_socket();
            println!(
                "laya: READY\n  \
                 endpoint  : {endpoint}\n  \
                 /health   : ok\n  \
                 /v1/models: ok (serving the expected model)\n  \
                 head      : ok (sidecar at {})",
                socket.display()
            );
            ExitCode::SUCCESS
        }
        Readiness::NotReady(f) => {
            // stderr, so a caller capturing stdout for the verdict does not swallow
            // the reason. The message is built by the readiness judgement — the same
            // text the server prints at startup when it refuses to run.
            eprintln!("{f}");
            eprintln!("\nlaya: NOT READY (exit 1)");
            ExitCode::FAILURE
        }
    }
}

/// Parse the arguments into `(command, endpoint)`.
fn parse_args(args: &[String]) -> Result<(Option<String>, String), String> {
    let mut command: Option<String> = None;
    let mut endpoint = DEFAULT_ENDPOINT.to_string();
    let mut i = 0;
    while i < args.len() {
        match args.get(i).map(String::as_str) {
            Some("--url") => {
                let Some(u) = args.get(i + 1) else {
                    return Err("--url needs a value".into());
                };
                endpoint.clone_from(u);
                i += 2;
            }
            Some(other) if command.is_none() => {
                command = Some(other.to_string());
                i += 1;
            }
            Some(other) => return Err(format!("unexpected argument `{other}`")),
            None => break,
        }
    }
    Ok((command, endpoint))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, endpoint) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("laya: {e}\nusage: laya doctor [--url URL]");
            return ExitCode::FAILURE;
        }
    };

    match command.as_deref() {
        Some("doctor") => doctor(&endpoint),
        Some(other @ ("decide" | "ledger")) => {
            eprintln!(
                "laya: `{other}` is not implemented yet (spec 04).\n\
                 \x20 implemented today: `laya doctor`"
            );
            ExitCode::FAILURE
        }
        Some(other) => {
            eprintln!("laya: unknown command `{other}`\nusage: laya doctor [--url URL]");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: laya doctor [--url URL]");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_bare_command_defaults_to_no_command_and_the_default_endpoint() {
        let (c, e) = parse_args(&args(&[])).unwrap_or((None, String::new()));
        assert!(c.is_none());
        assert_eq!(e, DEFAULT_ENDPOINT);
    }

    #[test]
    fn the_url_flag_overrides_the_endpoint() {
        let (c, e) =
            parse_args(&args(&["doctor", "--url", "http://x:1"])).unwrap_or((None, String::new()));
        assert_eq!(c.as_deref(), Some("doctor"));
        assert_eq!(e, "http://x:1");
    }

    #[test]
    fn the_url_flag_without_a_value_is_an_error() {
        // A missing value must be reported, not silently defaulted to the endpoint —
        // a doctor that checks the WRONG server and reports READY is worse than one
        // that fails.
        assert!(parse_args(&args(&["doctor", "--url"])).is_err());
    }

    #[test]
    fn an_unknown_command_is_an_error() {
        assert!(parse_args(&args(&["doctor", "extra"])).is_err());
    }

    #[test]
    fn the_command_and_flag_can_appear_in_either_order() {
        let a = parse_args(&args(&["--url", "http://y:2", "doctor"])).unwrap_or_default();
        assert_eq!(a.0.as_deref(), Some("doctor"));
        assert_eq!(a.1, "http://y:2");
    }
}
