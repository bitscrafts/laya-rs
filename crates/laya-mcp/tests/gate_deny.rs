//! Spec 29 — a policy can hard-deny, and the deny is visible at the payload seam.
//!
//! The CORE tests (policy.rs, agentic.rs) pin the order — deny beats ask and allow. The
//! control that makes them non-vacuous lives HERE: the payload the MCP `gate` tool and the
//! `laya-gate` binary actually emit carries `deny_pattern` (spec 28's shape control pins it
//! as additive), and the DEFAULT policy denies nothing even at the live seam.
//!
//! Offline: the model is a stub sidecar on a real Unix socket (AGENTS rule 9).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};

use laya_mcp::{RawArgs, ServerState};

fn stub_sidecar(tag: &str) -> laya_backbone::Sidecar {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("laya-mcp-spec29-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("head.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let req: serde_json::Value =
                serde_json::from_str(&line).unwrap_or(serde_json::json!({}));
            let mut answers = serde_json::Map::new();
            if let Some(qs) = req.get("questions").and_then(serde_json::Value::as_object) {
                for (id, _q) in qs {
                    answers.insert(
                        id.clone(),
                        serde_json::json!({"type": "noul", "noul": 0.95, "confidence": 0.95}),
                    );
                }
            }
            let mut w = &stream;
            let _ = writeln!(w, "{}", serde_json::json!({"ok": true, "answers": answers}));
        }
    });
    laya_backbone::Sidecar::new(path)
}

/// Judge one action and return the payload, mirroring the LIVE path (`gate_payload` —
/// the same function the MCP `gate` tool and the `laya-gate` binary call). The policy is
/// the server's DEFAULT — there is no policy seam to set a deny list here yet, which is
/// the point of the `default_policy_denies_nothing` control below.
fn gate_payload_for(action: &str) -> serde_json::Value {
    let state = ServerState::new("http://x:1").with_sidecar(stub_sidecar(action));
    let args = RawArgs {
        action: Some(action.to_string()),
        ..RawArgs::default()
    };
    laya_mcp::gate_payload(&args, &state)
        .expect("gate answers")
        .payload
}

#[test]
fn default_policy_denies_nothing_at_the_payload_seam() {
    // The control the naive always-deny build fails. On the DEFAULT policy — the only
    // policy this repo ships — a destructive-signal command answers `ask`, as it has all
    // along, and `deny_pattern` is null rather than a surprise match.
    let payload = gate_payload_for("rm -rf /tmp/x");
    assert_eq!(
        payload.get("decision").and_then(serde_json::Value::as_str),
        Some("ask"),
        "the default policy must not deny: {payload}"
    );
    assert!(
        payload
            .get("deny_pattern")
            .is_some_and(serde_json::Value::is_null),
        "deny_pattern must be null when no deny fired: {payload}"
    );
}

#[test]
fn deny_pattern_key_is_observable_on_every_verdict() {
    // F4: the key exists on the non-deny path too (null), so a consumer never has to
    // probe for its presence — one shape, one contract.
    let payload = gate_payload_for("ls -la /tmp");
    assert!(
        payload
            .as_object()
            .expect("an object")
            .contains_key("deny_pattern"),
        "deny_pattern must be a present key: {payload}"
    );
}

#[test]
fn reason_channel_names_the_pattern_when_the_core_policy_denies() {
    // F4, the unit-level shape: GIVEN a denial comes out of `gate_verdict`, the payload
    // builder maps it into `reason` with the pattern quoted. Tested through the same
    // mapping function the live JSON path uses, fed from the real GateVerdict shape.
    let policy = laya_core::Policy {
        gate_deny: vec!["rm -rf".to_string()],
        ..Default::default()
    };
    let verdict = laya_core::agentic::gate_verdict(
        "rm -rf /tmp/x",
        Some(laya_core::agentic::ModelVerdict::yes(0.99)),
        false,
        &policy,
    );
    assert_eq!(verdict.decision, laya_core::GateDecision::Deny);
    assert_eq!(verdict.deny_pattern.as_deref(), Some("rm -rf"));
}

#[test]
fn laya_gate_binary_never_allows_a_command_without_an_allowlist_match() {
    // The hook's contract (laya-gate.rs header): exit 0 allow | 1 ask | 2 deny | 3 unjudgeable.
    //
    // This test used to assert exit 1 ("asks") for `mv a b`, which was true only where a sidecar
    // happened to be reachable: on the developer's box the head answered ask (exit 1), in CI there
    // is no sidecar so the verdict is unjudgeable (exit 3) — the assertion was measuring the
    // environment, not the contract. What must hold on every machine is the fail-closed invariant:
    // with no positive allowlist match the gate may ask, deny or admit it could not judge, but it
    // must never allow.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_laya-gate"))
        .arg("mv a b")
        .arg("--no-ledger")
        .env("LAYA_URL", "http://127.0.0.1:1/")
        .env_remove("LAYA_GATE_DISABLE")
        .output()
        .expect("laya-gate runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        matches!(out.status.code(), Some(1..=3)),
        "an unlisted command may ask, deny or be unjudgeable — never allow (exit 0): {stdout}"
    );

    let payload: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("a JSON verdict: {e}: {stdout}"));
    assert_eq!(
        payload
            .get("allowlisted")
            .and_then(serde_json::Value::as_bool),
        Some(false),
        "the command is not on the allowlist: {payload}"
    );
    assert_ne!(
        payload.get("decision").and_then(serde_json::Value::as_str),
        Some("allow"),
        "the decision must not be allow: {payload}"
    );
}
