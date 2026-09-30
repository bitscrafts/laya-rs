//! A live demonstration of the route-shaped payload contract.
//!
//! This is a KEPT example, not a temporary probe: it is the executable form of the
//! contract that spec 02 describes, and it is how a reader can see — without
//! running the model, the backbone or the server — exactly what a graph's `route`
//! node will receive.
//!
//! ```text
//! cargo run -p laya-core --example payload_demo
//! ```
//!
//! The two fields to notice are `label` and `band`: they are the ones a DAG matches
//! on (`route on = "$.payload.label"`), and they are plain strings on purpose —
//! pii-graph's expression language supports only member access and exact equality,
//! so the payload must arrive ready to match. See `DESIGN-AMENDMENTS` blocker B5
//! for why the band cannot be computed in the graph.

use laya_abi::{Answer, QuestionType};
use laya_core::{DecisionPayload, Policy};

/// Build the triage answer our demo reuses.
///
/// A constructor rather than `..base.clone()`: the struct-update form clones the
/// whole struct and then overwrites two fields, which is both a real copy and a
/// clippy warning. Building each case explicitly keeps the demo honest about what
/// distinguishes the cases, which is the point of showing them side by side.
fn triage(value: &str, confidence: f64, probabilities: Vec<f64>) -> Answer {
    Answer {
        id: "triage".into(),
        kind: QuestionType::Choice,
        value: value.into(),
        p: None,
        score: None,
        probabilities,
        options: vec![
            "bug_in_code".into(),
            "wrong_test".into(),
            "environment".into(),
        ],
        confidence,
        latency_ms: Some(41.0),
    }
}

fn main() {
    let policy = Policy::default();

    // What the LLM lane produced: the test suite is red, and Laya is asked to
    // triage why. A `choice` over the caller's declared options — the model can
    // only select among these, never invent one.
    let payload = DecisionPayload::from_answer(
        &triage("bug_in_code", 0.93, vec![0.93, 0.05, 0.02]),
        &policy,
    );
    println!("=== a confident decision (routes on label) ===");
    println!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    // The case that matters: a real saturated choice, exactly as we measured it
    // from the deployed checkpoint. `band` stays "high" under the default policy
    // (the flag is recorded, not enforced), and `saturated: true` tells the ledger
    // — and any consumer paying attention — that this confidence is suspect.
    let sp =
        DecisionPayload::from_answer(&triage("bug_in_code", 0.99, vec![1.0, 0.0, 0.0]), &policy);
    println!("=== the measured saturated case (1.000/0.000/0.000) ===");
    println!(
        "saturated={}  band={}  <- flagged as suspect, not read as strong evidence",
        sp.saturated, sp.band
    );

    // With escalation on, the same answer can never claim High, so it routes to an
    // ask/confirm branch instead of auto-allowing. This is the property that makes
    // saturation safe rather than merely reported.
    let strict = Policy {
        escalate_on_saturation: true,
        ..Policy::default()
    };
    let dp =
        DecisionPayload::from_answer(&triage("bug_in_code", 0.99, vec![1.0, 0.0, 0.0]), &strict);
    println!("\n=== the same answer with escalate_on_saturation = true ===");
    println!(
        "saturated={}  band={}  <- demoted, so it cannot auto-allow",
        dp.saturated, dp.band
    );

    // The near-miss CONTROL: genuinely confident but not degenerate. This must keep
    // its High band, or the demotion above becomes a blanket tax on confidence.
    let hp = DecisionPayload::from_answer(
        &triage("bug_in_code", 0.98, vec![0.98, 0.011, 0.009]),
        &strict,
    );
    println!("\n=== near-miss control: confident but NOT saturated ===");
    println!(
        "saturated={}  band={}  <- stays High; the demotion is not a blanket tax",
        hp.saturated, hp.band
    );

    // Failure modes, shown as they actually present to a graph.
    let low =
        DecisionPayload::from_answer(&triage("wrong_test", 0.35, vec![0.35, 0.40, 0.25]), &policy);
    println!("\n=== a low-confidence pick (the video's 0.35 case) ===");
    println!(
        "label={}  band={}  <- routes to an ask/escalate port, NOT an action",
        low.label, low.band
    );
}
