//! `laya` — the terminal runner.
//!
//! Implemented: `laya doctor`, the readiness check (spec 05). Exits **0** when Laya
//! is usable and **1** when it is not, printing the actionable diagnostic on stderr.
//!
//! Implemented: `laya ledger compact|stats|tail|outcome` — the only writer of the store.
//!
//! Not implemented here: `decide`. The CLI prints that it is unimplemented and
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

/// `laya decide` — the whole path, text in and JSON out.
///
/// Prints the decision JSON on **stdout** and a one-line summary on stderr, so a harness can read
/// stdout while a human sees the summary. Exit codes: 1 usage, 2 an artifact or the backbone is not
/// usable, 3 the decision failed — documented on [`laya_cli::decide`].
fn decide(rest: &[String], endpoint: &str) -> ExitCode {
    let args = match laya_cli::DecideArgs::parse(rest, endpoint) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    match laya_cli::decide(&args) {
        Ok(json) => {
            println!("{json}");
            // The summary is rebuilt from the same JSON, so it cannot drift from what was printed.
            if let Ok(out) = serde_json::from_str::<laya_cli::DecideOutput>(&json) {
                eprintln!("{}", laya_cli::decide::summarise(&out));
            }
            ExitCode::SUCCESS
        }
        Err((msg, code)) => {
            eprintln!("{msg}");
            ExitCode::from(code)
        }
    }
}

/// Parse the arguments into `(command, endpoint)`.
/// Parse the command word and the endpoint, and pass everything else through untouched.
///
/// `rest` carries the arguments of a **subcommand** (`laya ledger compact --dir X`). They are
/// collected verbatim rather than validated here: this parser knows about the top-level
/// shape only, and each subcommand decides what it accepts. Validating a subcommand's
/// arguments in the top-level parser is how a flag gets rejected by code that has no idea
/// what it means — which is exactly what happened to `laya ledger compact`.
fn parse_args(args: &[String]) -> Result<(Option<String>, String, Vec<String>), String> {
    let mut command: Option<String> = None;
    let mut endpoint = DEFAULT_ENDPOINT.to_string();
    let mut rest: Vec<String> = Vec::new();
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
            // Everything after the command word belongs to the command.
            Some(other) => {
                rest.push(other.to_string());
                i += 1;
            }
            None => break,
        }
    }
    Ok((command, endpoint, rest))
}

// ---------------------------------------------------------------------------
// `laya ledger` — the single writer
// ---------------------------------------------------------------------------

/// Where the segments and the store live.
///
/// One env var, one default, and the SAME resolution used by the server, so the writer and the
/// readers cannot disagree about which directory they are talking about. A mismatch there would
/// look exactly like "the ledger is empty", which is the failure this avoids.
#[must_use]
pub fn ledger_dir(explicit: Option<&str>) -> std::path::PathBuf {
    explicit.map_or_else(
        || {
            std::env::var(LEDGER_DIR_ENV).map_or_else(
                |_| std::path::PathBuf::from(DEFAULT_LEDGER_DIR),
                std::path::PathBuf::from,
            )
        },
        std::path::PathBuf::from,
    )
}

/// The env var naming the ledger directory.
pub const LEDGER_DIR_ENV: &str = "LAYA_LEDGER_DIR";

/// The default ledger directory.
pub const DEFAULT_LEDGER_DIR: &str = ".laya";

/// `laya ledger <sub>` — a dispatcher, deliberately thin.
///
/// Each subcommand is its own function: the dispatcher stays readable, and a reader looking for
/// what `compact` does does not have to hold four unrelated behaviours in their head at once.
fn ledger(args: &[String]) -> ExitCode {
    let no_compact = args.iter().any(|a| a == "--no-compact");
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!(
            "usage: laya ledger <compact|stats|tail|outcome> [args]\n\
             \x20 compact              merge segment files into the store (the only writer)\n\
             \x20 stats                counts, latency percentiles, saturation rate\n\
             \x20 tail [N]             the most recent N records (default 10)\n\
             \x20 outcome <id> --correct|--wrong --source S [--note T]"
        );
        return ExitCode::FAILURE;
    };

    let dir = ledger_dir(option_value(args, "--dir").as_deref());
    let store = dir.join(laya_store::STORE_FILE);

    match sub {
        "compact" => ledger_compact(&dir, &store),
        // Ingest first by default: a reader that ignores un-ingested segments reports an empty
        // store while decisions sit on disk beside it. `--no-compact` is the pure read, for
        // anyone who wants the store exactly as it stands.
        "stats" => ledger_stats(&dir, &store, no_compact),
        "tail" => ledger_tail(&dir, &store, args, no_compact),
        "outcome" => ledger_outcome(&store, args),
        other => {
            eprintln!("laya: unknown ledger subcommand `{other}`");
            ExitCode::FAILURE
        }
    }
}

/// `laya ledger compact` — merge segments into the store. THE ONLY WRITER.
fn ledger_compact(dir: &std::path::Path, store: &std::path::Path) -> ExitCode {
    match laya_store::compact(dir, store) {
        Ok(report) => {
            println!(
                "segments read:    {}\nrecords seen:     {}\ninserted:         {}\n\
                 already present:  {}\ningested into:    {}",
                report.segments_read,
                report.records_seen,
                report.records_inserted,
                report.records_duplicate,
                report.ingested_into.display()
            );
            println!(
                "\n(The segments were MOVED to {}, not deleted: the merge is re-runnable and \
                 the raw evidence stays.)",
                laya_store::INGESTED_DIR
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `laya ledger stats` — and it says plainly when there is nothing to calibrate.
fn ledger_stats(dir: &std::path::Path, store: &std::path::Path, no_compact: bool) -> ExitCode {
    ingest_before_reading(dir, store, no_compact);
    // READ-ONLY. This used to open_writer, so asking for numbers while a `compact` ran got a
    // lock error instead of an answer -- a read demanding exclusivity it does not need.
    match laya_store::Ledger::read_only(store).and_then(|l| l.stats()) {
        Ok(s) => {
            println!("decisions:      {}", s.decisions);
            println!("outcomes:       {}", s.outcomes);
            println!("saturated:      {}", s.saturated);
            println!("near ties:      {}", s.near_ties);
            for (band, n) in &s.by_band {
                println!("band {band:<10} {n}");
            }
            println!("latency p50:    {} ms", show_ms(s.latency_p50_ms));
            println!("latency p95:    {} ms", show_ms(s.latency_p95_ms));
            println!("latency max:    {} ms", show_ms(s.latency_max_ms));
            // Per tool, because a single blended percentile answers nothing: `decide` and
            // `gate` are different costs, and the average moves only when the mix changes.
            if !s.by_tool.is_empty() {
                println!("\nper tool:");
                for (tool, ts) in &s.by_tool {
                    println!(
                        "  {tool:<16} {:<5} p50 {} ms  p95 {} ms  max {} ms",
                        ts.decisions,
                        show_ms(ts.latency_p50_ms),
                        show_ms(ts.latency_p95_ms),
                        show_ms(ts.latency_max_ms)
                    );
                    for (verdict, n) in &ts.verdicts {
                        println!("    {verdict:<12} {n}");
                    }
                    if !ts.verdicts.is_empty() {
                        println!("    {:<12} {}", "executed", ts.executed);
                    }
                }
            }
            if s.outcomes == 0 {
                // Stated plainly, because the alternative is a reader assuming a calibration
                // exists. ECE needs (confidence, correct) pairs and there are none.
                println!(
                    "\nNO OUTCOMES RECORDED — nothing here can be calibrated yet. Record one \
                     with:\n  laya ledger outcome <id> --correct --source test"
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Ingest any un-compacted segments before a read, so a reader sees what is on disk.
///
/// **Why this is here at all:** the server appends to `ledger-<pid>.jsonl` and `stats` reads
/// `ledger.redb`, which only `compact` fills. Measured on the live deployment: 7 real decisions
/// on disk, `stats` reporting `decisions: 0 — latency unknown`, and nothing scheduled to ever
/// compact. The two states were indistinguishable from outside: a ledger switched OFF and a
/// ledger nobody had ingested both read as zero.
///
/// **It never fails the read.** Compaction is opportunistic: if another writer holds the store
/// (its normal state while a server is up), the read proceeds against what is already ingested.
/// A read that refused because it could not also write would be worse than the staleness.
fn ingest_before_reading(dir: &std::path::Path, store: &std::path::Path, no_compact: bool) {
    if no_compact {
        return;
    }
    if laya_store::segments(dir).map_or(true, |s| s.is_empty()) {
        return;
    }
    // Held by a live writer, or nothing to insert: read what is already there and say nothing.
    // Silence here is deliberate -- the read still has an answer, and a warning on every call
    // while a server is up would be noise. A read is never failed by a failed ingestion: it
    // would be trading its answer for the chance to write.
    if let Ok(r) = laya_store::compact(dir, store)
        && r.records_inserted > 0
    {
        eprintln!(
            "laya: ingested {} row(s) from {} segment(s) before reading",
            r.records_inserted, r.segments_read
        );
    }
}

/// `laya ledger tail [N]` — the most recent records, newest first.
fn ledger_tail(
    dir: &std::path::Path,
    store: &std::path::Path,
    args: &[String],
    no_compact: bool,
) -> ExitCode {
    ingest_before_reading(dir, store, no_compact);
    let n = args
        .get(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10);
    match laya_store::Ledger::read_only(store).and_then(|l| l.tail(n)) {
        Ok(records) => {
            for r in records {
                println!("{}", serde_json::to_string(&r).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `laya ledger outcome <id> --correct|--wrong --source S [--note T]`.
///
/// The record that does not exist anywhere else in the system, and the reason this command is
/// worth having at all: `calibrate` needs `(confidence, correct)` pairs, and the ledger
/// otherwise knows only what was decided, never whether it was right.
fn ledger_outcome(store: &std::path::Path, args: &[String]) -> ExitCode {
    let Some(decision) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("laya: `ledger outcome` needs the decision id as its first argument");
        return ExitCode::FAILURE;
    };
    let correct = args.iter().any(|a| a == "--correct");
    let wrong = args.iter().any(|a| a == "--wrong");
    if correct == wrong {
        // Exactly one: an outcome with no verdict, or with two, is not a judgement.
        eprintln!("laya: pass exactly one of --correct or --wrong");
        return ExitCode::FAILURE;
    }
    let Some(source) = option_value(args, "--source") else {
        // Required, not defaulted. An outcome whose author is unknown cannot be weighed
        // against another, and "unknown" would quietly become a third class.
        eprintln!("laya: --source is required (test | human | later-run | …)");
        return ExitCode::FAILURE;
    };

    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let outcome = laya_store::OutcomeRow {
        ts: format!("{}", elapsed.as_secs()),
        // `as_nanos` is u128; the ledger key is u64. Saturate rather than truncate: a wrapped
        // timestamp would sort an outcome BEFORE the decision it judges.
        // This row is already epoch-based; it is the SERVER's rows that were process-relative,
        // which is why the two could never be ordered together.
        ts_nanos: u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
        wall_nanos: u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
        decision: decision.clone(),
        correct,
        source,
        note: option_value(args, "--note"),
    };
    match laya_store::Ledger::open_writer(store).and_then(|l| l.record_outcome(&outcome)) {
        Ok(()) => {
            let verdict = if correct { "correct" } else { "wrong" };
            println!("recorded: {decision} is {verdict}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("laya: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The value following an option, if present.
#[must_use]
pub fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Render a latency, or say it is unknown rather than printing a zero we never measured.
fn show_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |v| v.to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, endpoint, rest) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("laya: {e}\nusage: laya <doctor|ledger> [--url URL]");
            return ExitCode::FAILURE;
        }
    };

    match command.as_deref() {
        Some("doctor") => doctor(&endpoint),
        Some("ledger") => ledger(&rest),
        Some("decide") => decide(&rest, &endpoint),
        Some("graph") => laya_cli::graph(&rest),
        Some(other) => {
            eprintln!(
                "laya: unknown command `{other}`\nusage: laya <doctor|decide|ledger|graph> [--url URL]"
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: laya <doctor|decide|ledger|graph> [--url URL]");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::indexing_slicing
    )]

    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_bare_command_defaults_to_no_command_and_the_default_endpoint() {
        let (c, e, r) = parse_args(&args(&[])).unwrap_or((None, String::new(), Vec::new()));
        assert!(c.is_none());
        assert!(r.is_empty());
        assert_eq!(e, DEFAULT_ENDPOINT);
    }

    #[test]
    fn the_url_flag_overrides_the_endpoint() {
        let (c, e, _r) = parse_args(&args(&["doctor", "--url", "http://x:1"])).unwrap_or((
            None,
            String::new(),
            Vec::new(),
        ));
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
    fn arguments_after_the_command_belong_to_the_command() {
        // This test used to assert that `doctor extra` is an ERROR, which conflated two things:
        // an argument the top-level parser does not understand, and a SUBcommand's own
        // arguments. `laya ledger compact --dir X` is exactly the second kind, and that
        // conflation is what made every `laya ledger` invocation fail with
        // "unexpected argument `compact`".
        //
        // The top-level parser now collects them verbatim; each subcommand validates its own.
        let (_c, _e, rest) = parse_args(&args(&["ledger", "compact", "--dir", "/tmp/x"]))
            .unwrap_or_else(|e| panic!("must parse: {e}"));
        assert_eq!(rest, vec!["compact", "--dir", "/tmp/x"]);

        // And an unknown COMMAND is still the caller's problem — checked at dispatch, not here.
        let (c, _e, r) = parse_args(&args(&["nosuchcommand"])).unwrap_or_default();
        assert_eq!(c.as_deref(), Some("nosuchcommand"));
        assert!(r.is_empty());
    }

    #[test]
    fn the_command_and_flag_can_appear_in_either_order() {
        let a = parse_args(&args(&["--url", "http://y:2", "doctor"])).unwrap_or_default();
        assert_eq!(a.0.as_deref(), Some("doctor"));
        assert_eq!(a.1, "http://y:2");
    }

    /// R19: `graph` follows the SAME verbatim pass-through rule `ledger`
    /// already does — the top-level parser must not reject a `run`-specific
    /// flag it has no idea about. This is the top-level-parser half of the
    /// guarantee; `graph.rs`'s own tests cover what the subcommand does once
    /// it receives those arguments.
    #[test]
    fn the_graph_subcommand_arguments_are_passed_through_verbatim() {
        let (c, _e, rest) = parse_args(&args(&["graph", "run", "flow.json", "--flag", "x"]))
            .unwrap_or_else(|e| panic!("must parse: {e}"));
        assert_eq!(c.as_deref(), Some("graph"));
        assert_eq!(rest, vec!["run", "flow.json", "--flag", "x"]);
    }
    // ---- the ingest step, which nothing used to perform -------------------------

    /// A decision row, for driving the real ingest path.
    fn a_decision(label: &str, wall_nanos: u64) -> laya_store::Record {
        laya_store::Record::Decision(laya_store::DecisionRow {
            ts: "epoch+1".to_string(),
            ts_nanos: 1,
            wall_nanos,
            tool: "decide".to_string(),
            run_id: Some("run-1".to_string()),
            node_id: None,
            question_id: "kind".to_string(),
            question_type: "choice".to_string(),
            label: label.to_string(),
            confidence: Some(0.9),
            band: "high".to_string(),
            margin: None,
            near_tie: false,
            probabilities: None,
            saturated: false,
            laya_version: "0.3.5".to_string(),
            state_sha256: "x".to_string(),
            head_sha256: None,
            backbone_sha256: None,
            bucket_clamped: None,
            latency_ms: 7,
            executed: false,
        })
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!("laya-cli-{tag}-{nanos}"));
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    #[test]
    fn reading_ingests_pending_segments_first() {
        // The defect measured in the live deployment: 7 real decisions sat in segments while
        // `stats` reported `decisions: 0 -- latency unknown`, and nothing was ever scheduled to
        // compact. A reader that ignores pending segments says "no data" about data on disk.
        let dir = scratch("ingest-first");
        let store = dir.join(laya_store::STORE_FILE);
        let _ = laya_store::append_segment(&dir, &a_decision("billing", 1)).expect("a");
        let _ = laya_store::append_segment(&dir, &a_decision("auth", 2)).expect("b");

        // Before: the store is empty and holds nothing.
        assert_eq!(
            laya_store::Ledger::read_only(&store)
                .expect("open")
                .len()
                .expect("len"),
            0
        );

        ingest_before_reading(&dir, &store, false);

        assert_eq!(
            laya_store::Ledger::read_only(&store)
                .expect("open")
                .len()
                .expect("len"),
            2,
            "both rows must be readable after an ordinary read"
        );
    }

    #[test]
    fn the_control_no_compact_reads_the_store_exactly_as_it_stands() {
        // The near-miss: `--no-compact` must NOT ingest, or the flag is a lie and there is no
        // way to inspect the store without mutating it.
        let dir = scratch("no-compact");
        let store = dir.join(laya_store::STORE_FILE);
        let _ = laya_store::append_segment(&dir, &a_decision("billing", 1)).expect("a");

        ingest_before_reading(&dir, &store, true);

        assert_eq!(
            laya_store::Ledger::read_only(&store)
                .expect("open")
                .len()
                .expect("len"),
            0,
            "--no-compact must leave the store alone"
        );
        assert_eq!(
            laya_store::segments(&dir).expect("segments").len(),
            1,
            "and the segment must still be there to ingest later"
        );
    }

    #[test]
    fn a_read_never_fails_because_ingestion_could_not_write() {
        // A reader must not trade its answer for the chance to write. With no segments at all
        // the helper must do nothing and stay silent, which is its state whenever a server is
        // between writes.
        let dir = scratch("ingest-noop");
        let store = dir.join(laya_store::STORE_FILE);
        ingest_before_reading(&dir, &store, false);
        assert_eq!(
            laya_store::Ledger::read_only(&store)
                .expect("open")
                .len()
                .expect("len"),
            0
        );
    }
}
