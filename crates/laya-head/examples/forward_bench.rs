//! Forward-pass benchmark that can fail: exits 1 when the median exceeds `--max-ms`.
//!
//! `cargo run --release -p laya-head --example forward_bench -- --seq 460 --max-ms 1000`
//!
//! Exit codes: 0 within budget, 1 over budget, 2 checkpoint absent or unusable, 64 bad usage.
//! It never skips: a missing checkpoint is a labelled failure, not a pass.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use laya_head::forward::Head;
use laya_head::safetensors::SafetensorsFile;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

const REAL_HEAD: &str = "/projects/_tmp/laya-gguf/laya-head.safetensors";
const RUNS: usize = 3;

fn arg(args: &[String], name: &str) -> Option<usize> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1)?.parse().ok()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(seq), Some(max_ms)) = (arg(&args, "--seq"), arg(&args, "--max-ms")) else {
        eprintln!("usage: forward_bench --seq <N> --max-ms <N>");
        return ExitCode::from(64);
    };
    if seq == 0 {
        eprintln!("forward_bench: --seq must be at least 1");
        return ExitCode::from(64);
    }
    let path = Path::new(REAL_HEAD);
    let head = match SafetensorsFile::open(path)
        .map_err(|e| e.to_string())
        .and_then(|f| Head::load(&f).map_err(|e| e.to_string()))
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("forward_bench: CHECKPOINT UNAVAILABLE at {REAL_HEAD}: {e}");
            return ExitCode::from(2);
        }
    };
    let d = head.d();
    // Deterministic pseudo-random hidden state; values are irrelevant to timing.
    let mut state = 0x9E37_79B9_u32;
    let hidden: Vec<f32> = (0..seq * d)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (f32::from((state >> 16) as u16) / 32768.0) - 1.0
        })
        .collect();
    let marker_pos = [seq / 2, seq - 1];
    let marker_mask = [true, true];

    let mut times = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t0 = Instant::now();
        if let Err(e) = head.forward(&hidden, 0, &marker_pos, &marker_mask) {
            eprintln!("forward_bench: forward failed: {e}");
            return ExitCode::from(2);
        }
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let min = times.first().copied().unwrap_or(f64::NAN);
    let median = times.get(times.len() / 2).copied().unwrap_or(f64::NAN);
    println!("forward seq={seq}: min {min:.1} ms, median {median:.1} ms (budget {max_ms} ms)");
    // `is_nan` is checked explicitly so a NaN median fails rather than passes.
    #[allow(clippy::cast_precision_loss)]
    if median.is_nan() || median > max_ms as f64 {
        eprintln!("forward_bench: OVER BUDGET: median {median:.1} ms > {max_ms} ms");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
