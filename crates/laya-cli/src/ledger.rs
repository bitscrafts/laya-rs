//! `laya ledger stats` — reading the store and rendering the numbers (spec 11, R7).
//!
//! In the library rather than in `main.rs` so the read path can be tested through the same
//! function the binary calls: a test that opened the store itself could not fail when this
//! function started taking the write lock again.

use std::path::Path;
use std::time::Duration;

use laya_store::{Ledger, Stats, StoreError};

/// How long a read waits for a writer to let go before reporting the store as busy.
///
/// A compaction holds the store for the length of one batch of inserts, so a few seconds rides
/// out a normal one. A writer that is still there after this is stuck, and saying so beats
/// hanging behind it.
pub const READ_WAIT: Duration = Duration::from_secs(5);

/// The store's stats, read through a **read-only** handle.
///
/// This used to be `Ledger::open_writer`, so asking for numbers took the exclusive lock and two
/// concurrent `stats` calls — or one during a `compact` — got a lock error in place of data. A
/// read-only handle never writes and coexists with other readers; against a WRITER it still
/// cannot coexist (redb, measured), so it waits up to `wait` for the writer to finish.
///
/// # Errors
/// [`StoreError::AlreadyOpen`] when a writer still holds the store after `wait`; otherwise any
/// failure to open or read the store.
pub fn read_stats(store: &Path, wait: Duration) -> Result<Stats, StoreError> {
    Ledger::read_only_waiting(store, wait)?.stats()
}

fn show_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |v| v.to_string())
}

/// The text `laya ledger stats` prints.
///
/// Per tool, because a single blended percentile answers nothing: `decide` and `gate` are
/// different costs, and the average moves only when the mix changes.
#[must_use]
pub fn render_stats(s: &Stats) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "decisions:      {}", s.decisions);
    let _ = writeln!(out, "outcomes:       {}", s.outcomes);
    let _ = writeln!(out, "saturated:      {}", s.saturated);
    let _ = writeln!(out, "near ties:      {}", s.near_ties);
    for (band, n) in &s.by_band {
        let _ = writeln!(out, "band {band:<10} {n}");
    }
    let _ = writeln!(out, "latency p50:    {} ms", show_ms(s.latency_p50_ms));
    let _ = writeln!(out, "latency p95:    {} ms", show_ms(s.latency_p95_ms));
    let _ = writeln!(out, "latency max:    {} ms", show_ms(s.latency_max_ms));
    if !s.by_tool.is_empty() {
        let _ = writeln!(out, "\nper tool:");
        for (tool, ts) in &s.by_tool {
            let _ = writeln!(
                out,
                "  {tool:<16} {:<5} p50 {} ms  p95 {} ms  p99 {} ms  max {} ms",
                ts.decisions,
                show_ms(ts.latency_p50_ms),
                show_ms(ts.latency_p95_ms),
                show_ms(ts.latency_p99_ms),
                show_ms(ts.latency_max_ms)
            );
            for (verdict, n) in &ts.verdicts {
                let _ = writeln!(out, "    {verdict:<12} {n}");
            }
            if !ts.verdicts.is_empty() {
                let _ = writeln!(out, "    {:<12} {}", "executed", ts.executed);
            }
        }
    }
    if s.outcomes == 0 {
        // Stated plainly, because the alternative is a reader assuming a calibration exists.
        // ECE needs (confidence, correct) pairs and there are none.
        let _ = writeln!(
            out,
            "\nNO OUTCOMES RECORDED — nothing here can be calibrated yet. Record one with:\n  \
             laya ledger outcome <id> --correct --source test"
        );
    }
    out
}

/// What `laya doctor` says about the ledger, and whether it is a warning.
///
/// `doctor` is its own process, so `raw` is **its** `LAYA_LEDGER_DIR`, which says nothing about
/// the environment of the running `laya-mcp` — that comes from the host's MCP entry. So this
/// reports what IS observable from outside the server: whether a directory is configured here,
/// and what state it is in (exists? how many pending segments?). The server's own view is the
/// `{"event":"ledger",...}` line it writes at startup; the two are complementary and neither
/// replaces the other. Never a failure: an off ledger is a configuration, announced.
#[must_use]
pub fn doctor_ledger_line(raw: Option<&str>) -> (String, bool) {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            "ledger: off (LAYA_LEDGER_DIR unset in this shell — the server's own setting is in its \
             startup `ledger` event)"
                .to_string(),
            true,
        );
    };
    let dir = Path::new(raw);
    if !dir.is_dir() {
        return (
            format!("ledger: {raw} does not exist yet (nothing has been recorded there)"),
            true,
        );
    }
    let pending = laya_store::segments(dir).map_or(0, |s| s.len());
    let store = if dir.join(laya_store::STORE_FILE).exists() {
        "store present"
    } else {
        "no store yet (run `laya ledger compact`)"
    };
    (
        format!("ledger: {raw} — {pending} pending segment(s), {store}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn store_with_one_gate_row(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("laya-cli-ledger-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let row = laya_store::Record::Decision(laya_store::DecisionRow {
            ts: "epoch+1".to_string(),
            ts_nanos: 1,
            wall_nanos: 1,
            tool: "gate".to_string(),
            run_id: None,
            node_id: None,
            question_id: "q".to_string(),
            question_type: "noul".to_string(),
            label: "allow".to_string(),
            confidence: None,
            band: "mid".to_string(),
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
            executed: Some(false),
            gate_decision: Some("allow".to_string()),
            model_decision: None,
            destructive_signals: None,
            command_sha256: None,
            p: None,
            score: None,
        });
        let _ = laya_store::append_segment(&dir, &row).expect("append");
        let store = dir.join(laya_store::STORE_FILE);
        let _ = laya_store::compact(&dir, &store).expect("compact");
        store
    }

    #[test]
    fn stats_does_not_take_the_write_lock() {
        // R7. A reader is held open, which excludes any WRITER (measured: a held reader makes
        // `open_writer` fail `AlreadyOpen`) but not another reader. If `read_stats` asked for the
        // write lock -- as `laya ledger stats` used to -- it would fail here with the lock error.
        let store = store_with_one_gate_row("nolock");
        let held = Ledger::read_only(&store).expect("a reader");
        assert!(
            matches!(
                Ledger::open_writer(&store),
                Err(StoreError::AlreadyOpen { .. })
            ),
            "the control: the held reader really does exclude a writer"
        );
        let stats = read_stats(&store, Duration::from_millis(0))
            .expect("stats must answer while another reader is held");
        assert_eq!(stats.decisions, 1);
        drop(held);
    }

    #[test]
    fn stats_waits_out_a_compaction_that_holds_the_writer() {
        // The operator's actual question: stats while a compact is running. The writer is held
        // for 300 ms, as a compaction would; the read must come back with numbers.
        let store = store_with_one_gate_row("wait");
        let writer = Ledger::open_writer(&store).expect("the compactor");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(writer);
        });
        let stats = read_stats(&store, Duration::from_secs(5)).expect("numbers, not a lock error");
        assert_eq!(stats.decisions, 1);
        release.join().expect("join");
    }

    #[test]
    fn the_control_a_stuck_writer_is_reported_as_busy() {
        let store = store_with_one_gate_row("stuck");
        let _writer = Ledger::open_writer(&store).expect("writer");
        let err = read_stats(&store, Duration::from_millis(100)).expect_err("busy");
        assert!(matches!(err, StoreError::AlreadyOpen { .. }), "{err:?}");
    }

    #[test]
    fn the_rendering_shows_per_tool_percentiles_and_the_gate_verdicts() {
        let store = store_with_one_gate_row("render");
        let text = render_stats(&read_stats(&store, READ_WAIT).expect("stats"));
        assert!(text.contains("per tool:"), "{text}");
        assert!(text.contains("gate"), "{text}");
        assert!(text.contains("p99 7 ms"), "{text}");
        assert!(text.contains("allow"), "{text}");
        assert!(text.contains("executed"), "{text}");
    }

    #[test]
    fn doctor_says_the_ledger_is_off_as_a_warning() {
        let (line, warning) = doctor_ledger_line(None);
        assert!(
            line.starts_with("ledger: off (LAYA_LEDGER_DIR unset"),
            "{line}"
        );
        assert!(warning, "off is announced, not silent");
        // An empty value is the same as unset: the server treats it so.
        assert!(doctor_ledger_line(Some("  ")).1);
    }

    #[test]
    fn the_control_a_configured_directory_is_not_a_warning() {
        let store = store_with_one_gate_row("doctor");
        let dir = store
            .parent()
            .expect("dir")
            .to_str()
            .expect("utf8")
            .to_string();
        let (line, warning) = doctor_ledger_line(Some(&dir));
        assert!(!warning, "{line}");
        assert!(line.contains("store present"), "{line}");
    }
}
