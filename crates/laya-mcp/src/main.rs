//! `laya-rs` — the MCP stdio server binary.
//!
//! Order matters and is deliberate:
//!
//! ```text
//! 1. parse config (endpoint, queue capacity)
//! 2. PREFLIGHT — probe Laya. Not ready => diagnostic on stderr, exit(non-zero).
//! 3. serve MCP over stdio, with a bounded lifetime.
//! ```
//!
//! **Step 2 comes before serving on purpose.** User directive: *"it will be mandatory
//! the app to check for laya be running! and it must inform/stop if not!"* A decision
//! server that cannot decide must not accept work, because every accepted call is a
//! call a client believes was answered.
//!
//! The bounded lifetime (step 3) is the spec-22 fix: `running.waiting()` resolves when
//! the stdio transport closes, so the server exits cleanly on EOF instead of parking
//! forever — the "MCP client hang = server parks forever" caveat.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::sync::Arc;

use laya_backbone::DEFAULT_ENDPOINT;
use laya_mcp::{LayaMcpServer, ServerState, clock, probe};
use rmcp::serve_server;
use rmcp::transport::stdio;

fn main() -> ExitCode {
    let endpoint = std::env::var("LAYA_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    // ---- STEP 2: the mandatory preflight, BEFORE serving --------------------
    let verdict = probe(&endpoint);
    if !verdict.is_ready() {
        // stderr, so a client that reads stdout as protocol data is not corrupted by
        // a diagnostic. The message is the readiness module's — the same text
        // `laya doctor` prints.
        eprintln!("{verdict}");
        eprintln!("\nlaya-mcp: refusing to serve (spec 05). A decision layer that cannot");
        eprintln!("decide must not run, and a gate that cannot ask must never answer `allow`.");
        return ExitCode::FAILURE;
    }

    let state = Arc::new(ServerState::new(endpoint.clone()));
    // The verdict is stamped with the SHARED clock, so it is genuinely fresh —
    // recording it at 0 (the original code) made it always stale, which made the
    // server behave as though Laya were down. Found by the end-to-end test.
    state.record_readiness(verdict, clock::now_ms());
    let server = LayaMcpServer::new(Arc::clone(&state));
    eprintln!("laya-mcp: Laya is ready at {endpoint}; serving MCP over stdio");

    // ---- STEP 3: serve, with a bounded lifetime ----------------------------
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("laya-mcp: could not start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Two distinct failure stages with two distinct meanings, so they are reported
    // separately rather than funnelled through one `?`:
    //   - the handshake never completed (the client is not speaking MCP, or the
    //     transport failed) — nothing was ever served;
    //   - the session ended with an error after serving — work may have been done.
    let session_result: Result<(), String> = rt.block_on(async move {
        let (r, w) = stdio();
        let running = match serve_server(server, (r, w)).await {
            Ok(running) => running,
            Err(e) => return Err(format!("could not complete the MCP handshake: {e}")),
        };
        // Resolves when the transport closes, so the process exits instead of parking.
        // `waiting()` yields a `QuitReason` (how the session ended), which is worth
        // logging: it distinguishes a clean EOF from a client-initiated cancellation.
        match running.waiting().await {
            Ok(reason) => {
                eprintln!("laya-mcp: session ended ({reason:?})");
                Ok(())
            }
            Err(e) => Err(format!("the session ended with an error: {e}")),
        }
    });

    match session_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("laya-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
