#!/usr/bin/env python3
"""GREEN: implement the ask_file handler, and repoint the OLD scope tests.

The old `ReadScope` tests must not be deleted — the behaviour they pin is real and now
belongs to `laya_core::ask_file::Scope`. Deleting them would drop coverage of the security
rule; leaving them calling a removed type would not compile. They are REPOINTED, and keep
their assertions verbatim so the pinning is provably unchanged.
"""
import pathlib
import re

p = pathlib.Path("crates/laya-mcp/src/lib.rs")
t = p.read_text()


def sub(old, new, count=1):
    global t
    if old not in t:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:240]}")
    t = t.replace(old, new, count)


# ---- repoint the old scope tests at the core type --------------------------
t = t.replace("ReadScope::new(", "laya_core::ask_file::Scope::new(")

# ---- the handler: ask_file -------------------------------------------------
# Inserted beside decide_via_sidecar / gate_via_sidecar.
anchor = "/// The `decide` tool: ask typed questions, return a route-shaped payload."
if anchor not in t:
    anchor = "fn decide_via_sidecar("
    anchor = t[max(0, t.find(anchor) - 400) : t.find(anchor)]
    # fall back to inserting before whichever function we found
    raise SystemExit("could not locate the decide handler anchor")

sub(anchor,
'''/// The `ask_file` tool: read a file inside the scope and ask a typed question about it.
///
/// The file NEVER enters the caller's context — only the typed answer is returned. That is
/// the entire value of the tool (measured: 9,089 tokens judged for $0.00049, against
/// $0.091 at a frontier model, paid again every turn).
///
/// Order of operations, and every step is deliberate:
///   1. arguments validated by the caller (`path`, `question`)
///   2. the read scope is enforced **before** the file is opened (spec 04 F7)
///   3. size and encoding limits are enforced, refusing rather than truncating
///   4. the content is placed under `data` as UNTRUSTED (spec 04 F9)
///   5. only then is the head asked
///
/// A refusal at step 2 costs nothing and tells the caller nothing they may not know.
fn ask_file_via_sidecar(args: &RawArgs, state: &ServerState) -> Result<CallToolResult, McpError> {
    let path = args.path.as_deref().unwrap_or_default();
    let question = args.question.as_deref().unwrap_or_default();

    let file = laya_core::ask_file::read_for_question(&state.read_scope, path, question)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    let answers = state
        .sidecar
        .predict(&file.to_state(), &laya_core::ask_file::FileState::question_spec())
        .map_err(|e| {
            McpError::internal_error(format!("laya-mcp: the decision head is unavailable: {e}"), None)
        })?;

    // The answer is reported with the path it came from, so a caller cannot confuse which
    // file a verdict belongs to.
    let mut out = json!({ "path": file.path, "answers": answers });
    if let Some(obj) = out.as_object_mut() {
        obj.insert("content_in_context".into(), json!(false));
    }
    Ok(to_result(ToolOutcome::Ok(out.to_string())))
}

''')

# ---- wire it into `handle`'s implemented branch ----------------------------
sub('''        "decide" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            decide_via_sidecar(args, state)
        }''',
'''        "decide" => {
            if let Some(missing) = missing_required(tool, args) {
                return Err(McpError::invalid_params(
                    format!("`{tool}` requires `{missing}`"),
                    None,
                ));
            }
            decide_via_sidecar(args, state)
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

# ---- main.rs: resolve the scope at startup and log the boundary ------------
p.write_text(t)
print("  ask_file handler implemented + wired; old scope tests repointed")

m = pathlib.Path("crates/laya-mcp/src/main.rs")
mt = m.read_text()
if "resolve_read_scope" not in mt:
    mt = mt.replace(
        "    let state = Arc::new(ServerState::new(endpoint.clone()));",
        '''    // The read scope is resolved ONCE, at startup, and the boundary in force is PRINTED.
    // A file-reading tool whose boundary has to be inferred is a tool nobody can reason
    // about; printing it also means a misconfigured deployment is visible in the log
    // rather than only in a refusal.
    let (scope, scope_notes) = laya_mcp::resolve_read_scope(
        std::env::var(laya_mcp::READ_ROOTS_ENV).ok().as_deref(),
    );
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
        "use laya_mcp::{LayaMcpServer, ServerState, clock, probe, resolve_read_scope};",
    )
    m.write_text(mt)
    print("  main.rs: scope resolved at startup + boundary logged")
