//! Spec 28 — the decision identity is visible in the gate payload.
//!
//! The payload's `decision_id` must be the ledger row's own `Record::identity()` — the same
//! bytes `laya ledger outcome` joins on — not an invented id. The anti-vacuous control reads
//! the row back from the segment and compares identities byte-for-byte.
//!
//! Offline: the model is a stub sidecar on a real Unix socket (AGENTS rule 9 — no model calls
//! in unit tests).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};

use laya_mcp::{RawArgs, ServerState};

/// A stand-in decision head on a REAL Unix socket, driving the production `Sidecar` client.
fn stub_sidecar(tag: &str) -> laya_backbone::Sidecar {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("laya-mcp-spec28-{tag}-{nanos}"));
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
                        serde_json::json!({"type": "noul", "noul": 0.9, "confidence": 0.8}),
                    );
                }
            }
            let mut w = &stream;
            let _ = writeln!(w, "{}", serde_json::json!({"ok": true, "answers": answers}));
        }
    });
    laya_backbone::Sidecar::new(path)
}

fn tempdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let p = std::env::temp_dir().join(format!("laya-mcp-spec28-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).expect("mkdir");
    p
}

fn gate_once(tag: &str, ledger: Option<&std::path::Path>) -> serde_json::Value {
    let mut state = ServerState::new("http://x:1").with_sidecar(stub_sidecar(tag));
    if let Some(dir) = ledger {
        state = state.with_ledger_dir(Some(dir.to_path_buf()));
    }
    let args = RawArgs {
        action: Some("git status".to_string()),
        ..RawArgs::default()
    };
    laya_mcp::gate_payload(&args, &state)
        .expect("gate answers")
        .payload
}

fn segment_records(dir: &std::path::Path) -> Vec<laya_store::Record> {
    let mut out = Vec::new();
    for f in laya_store::segments(dir).expect("segments") {
        out.extend(laya_store::read_segment(&f).expect("read"));
    }
    out
}

#[test]
fn payload_carries_decision_identity() {
    let dir = tempdir("carries");
    let payload = gate_once("carries-sock", Some(&dir));
    let id = payload
        .get("decision_id")
        .and_then(serde_json::Value::as_str)
        .expect("decision_id present when the ledger row was written");
    assert!(!id.is_empty(), "the identity must be a non-empty string");
}

#[test]
fn decision_id_equals_record_identity() {
    // The anti-vacuous control: a fresh UUID would pass a presence check and join NOTHING.
    // The comparison is against the identity computed from the ledger row itself.
    let dir = tempdir("equals");
    let payload = gate_once("equals-sock", Some(&dir));
    let payload_id = payload
        .get("decision_id")
        .and_then(serde_json::Value::as_str)
        .expect("decision_id present");
    let records = segment_records(&dir);
    assert_eq!(records.len(), 1, "exactly one decision row was written");
    assert_eq!(
        payload_id,
        records[0].identity(),
        "the payload's decision_id must be byte-equal to the ledger row's identity"
    );
}

#[test]
fn no_ledger_no_identity() {
    let payload = gate_once("noledger-sock", None);
    let v = payload
        .get("decision_id")
        .expect("the key exists even without a ledger");
    assert!(v.is_null(), "no ledger row, no identity: got {v:?}");
}

#[test]
fn known_good_payload_unchanged() {
    // Shape control: the golden (frozen pre-change) plus `decision_id` — and NOTHING else. A
    // rename or re-shape here would break every existing consumer (spec 28, F5). The golden
    // pins the KEY SET captured from the live binary, not the values: those are live model
    // outputs that vary run to run.
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("golden-payload-28.json")).expect("golden parses");
    let expected: std::collections::BTreeSet<&str> = golden["keys_after_decision_id"]
        .as_array()
        .expect("golden keys")
        .iter()
        .map(|k| k.as_str().expect("a key is a string"))
        // `decision_id` is the golden's own name now; it was chained as the additive
        // field when spec 28 froze the shape, and spec 29 added `deny_pattern` to the
        // frozen list itself.
        .chain(std::iter::once("decision_id"))
        .collect();
    let dir = tempdir("golden");
    let payload = gate_once("golden-sock", Some(&dir));
    let obj = payload.as_object().expect("the payload is an object");
    let actual: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let missing: Vec<_> = expected.difference(&actual).collect();
    let extra: Vec<_> = actual.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the payload shape is golden + decision_id only: missing={missing:?} extra={extra:?}"
    );
}

#[test]
fn laya_gate_binary_passthrough() {
    // The hook's binary prints the WHOLE payload, so it inherits the field. This guards
    // against a wrapper stripping it (spec 28, F3).
    let dir = tempdir("bin");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_laya-gate"))
        .arg("git status")
        .env("LAYA_LEDGER_DIR", &dir)
        .env_remove("LAYA_URL")
        .output()
        .expect("laya-gate runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let payload: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is one JSON payload ({e}): {stdout}"));
    let id = payload
        .get("decision_id")
        .and_then(serde_json::Value::as_str)
        .expect("the binary's stdout carries decision_id");
    assert!(!id.is_empty());
}
