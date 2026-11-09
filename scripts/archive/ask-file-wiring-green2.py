#!/usr/bin/env python3
"""GREEN, corrected anchor: insert `ask_file_via_sidecar` before `decide_via_sidecar`
(line 550) and wire it into `handle`.
"""
import pathlib

p = pathlib.Path("crates/laya-rs/crates/laya-mcp/src/lib.rs".replace("crates/laya-rs/", "/projects/laya-rs/"))
t = p.read_text()


def sub(old, new, count=1):
    global t
    if old not in t:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:240]}")
    t = t.replace(old, new, count)


handler = '''/// The `ask_file` tool: read a file inside the scope and ask a typed question about it.
///
/// The file NEVER enters the caller's context — only the typed answer is returned. That is
/// the entire value of the tool (measured: 9,089 tokens judged for $0.00049, against
/// $0.091 at a frontier model, paid again every turn until compaction).
///
/// Order of operations, and every step is deliberate:
///   1. arguments validated by the caller (`path`, `question`)
///   2. the read scope is enforced **before** the file is opened (spec 04 F7) — a refusal
///      costs nothing and discloses nothing
///   3. size and encoding limits are enforced, refusing rather than truncating
///   4. the content is placed under `data` as UNTRUSTED (spec 04 F9)
///   5. only then is the head asked
fn ask_file_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let path = args.path.as_deref().unwrap_or_default();
    let question = args.question.as_deref().unwrap_or_default();

    let file = laya_core::ask_file::read_for_question(&state.read_scope, path, question)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    let answers = state
        .sidecar
        .predict(
            &file.to_state(),
            &laya_core::ask_file::FileState::question_spec(),
        )
        .map_err(|e| {
            McpError::internal_error(
                format!("laya-mcp: the decision head is unavailable: {e}"),
                None,
            )
        })?;

    // Reported with the path it came from so a caller cannot confuse which file a verdict
    // belongs to — and `content_in_context: false` states the property explicitly rather
    // than leaving the caller to infer it.
    let mut out = json!({ "path": file.path, "answers": answers });
    if let Some(obj) = out.as_object_mut() {
        obj.insert("content_in_context".into(), json!(false));
    }
    to_result(ToolOutcome::Ok(out.to_string()))
}

'''

sub("fn decide_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {",
    handler + "fn decide_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {")

sub('''        "ask_file" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            ask_file_via_sidecar(args, state)
        }
''', "")  # in case a previous partial run inserted it

sub('''            decide_via_sidecar(args, state)
        }''',
'''            decide_via_sidecar(args, state)
        }
        "ask_file" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            ask_file_via_sidecar(args, state)
        }''')

p.write_text(t)
print("  ask_file handler inserted + wired into handle")

# main.rs scope wiring (may already be done)
m = pathlib.Path("/projects/laya-rs/crates/laya-mcp/src/main.rs")
mt = m.read_text()
if "resolve_read_scope" not in mt:
    mt = mt.replace(
        "    let state = Arc::new(ServerState::new(endpoint.clone()));",
        '''    // Resolved ONCE at startup, and the boundary in force is PRINTED. A file-reading tool
    // whose boundary must be inferred is one nobody can reason about, and printing it makes
    // a misconfigured deployment visible in the log rather than only in a refusal.
    let (scope, scope_notes) =
        resolve_read_scope(std::env::var(READ_ROOTS_ENV).ok().as_deref());
    for n in &scope_notes {
        eprintln!("laya-mcp: {n}");
    }
    eprintln!(
        "laya-mcp: ask_file scope = {}",
        scope
            .roots()
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let state = Arc::new(
        ServerState::new(endpoint.clone()).with_read_scope(scope.roots().to_vec()),
    );''',
    )
    mt = mt.replace(
        "use laya_mcp::{LayaMcpServer, ServerState, clock, probe};",
        "use laya_mcp::{LayaMcpServer, READ_ROOTS_ENV, ServerState, clock, probe, resolve_read_scope};",
    )
    m.write_text(mt)
    print("  main.rs: scope resolved at startup + boundary logged")
else:
    print("  main.rs: already wired")
