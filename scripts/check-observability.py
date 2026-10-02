#!/usr/bin/env python3
"""Measure the DEPLOYED laya on the live Hermes — the observability end-to-end check.

Why this exists: `specs/11` can only assert that the instrumentation is correct in isolation.
The question the operator actually has is "is the ledger working on the box I am standing on",
and that question had no answer, because a ledger that is switched OFF and a ledger nobody has
INGESTED both printed `decisions: 0`. This script drives the real deployment and reports what
it finds, failing loudly when the ledger is off.

It speaks MCP over stdio to the deployed `laya-mcp` binary, exactly as an agent does, so the
check exercises the live path rather than a re-enactment of it.

Usage:
    uv run scripts/check-observability.py                 # measure, and fail if the ledger is off
    uv run scripts/check-observability.py --json          # machine-readable

Exit codes: 0 = the ledger is recording AND readable, 1 = not, 2 = usage/environment error.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

MCP_BIN = os.environ.get("LAYA_MCP_BIN", "/root/.local/bin/laya-mcp")
CLI_BIN = os.environ.get("LAYA_CLI_BIN", "/root/.local/bin/laya")
LEDGER_DIR = os.environ.get("LAYA_LEDGER_DIR", "/var/lib/laya-ledger")


def rpc(proc: subprocess.Popen, msg: dict) -> dict:
    """One JSON-RPC round trip over the server's stdio."""
    assert proc.stdin and proc.stdout
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("the server closed the stream")
        try:
            reply = json.loads(line)
        except json.JSONDecodeError:
            continue
        if reply.get("id") == msg["id"]:
            return reply


def call_tool(proc: subprocess.Popen, name: str, args: dict, rid: int) -> str:
    """Call one MCP tool and return the text of the result, or raise on a protocol error."""
    reply = rpc(
        proc,
        {
            "jsonrpc": "2.0",
            "id": rid,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        },
    )
    if "error" in reply:
        raise RuntimeError(f"{name} returned a protocol error: {reply['error']}")
    blocks = reply.get("result", {}).get("content", [])
    return "\n".join(b.get("text", "") for b in blocks)


def segments(directory: Path) -> int:
    return len(list(directory.glob("ledger-*.jsonl"))) if directory.is_dir() else 0


def rows_in_segments(directory: Path) -> int:
    total = 0
    for f in sorted(directory.glob("ledger-*.jsonl")) if directory.is_dir() else []:
        for line in f.read_text(errors="ignore").splitlines():
            if line.strip():
                total += 1
    return total


def cli_ledger(subcommand: str) -> str:
    """Run the deployed CLI. `stats`/`tail` ingest pending segments first, by design."""
    out = subprocess.run(
        [CLI_BIN, "ledger", subcommand, "--dir", LEDGER_DIR],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    return (out.stdout + out.stderr).strip()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    for p in (MCP_BIN, CLI_BIN):
        if not os.access(p, os.X_OK):
            print(f"check-observability: {p} is not executable", file=sys.stderr)
            return 2

    report: dict = {"ledger_dir": LEDGER_DIR, "checks": {}}
    failures: list[str] = []

    def check(name: str, ok: bool, detail: str) -> None:
        report["checks"][name] = {"ok": ok, "detail": detail}
        if not ok:
            failures.append(f"{name}: {detail}")

    # --- 1. the server announces whether it is recording ---------------------
    env = dict(os.environ)
    env["LAYA_LEDGER_DIR"] = LEDGER_DIR
    proc = subprocess.Popen(
        [MCP_BIN],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        bufsize=1,
    )
    try:
        # A real MCP client announces its protocol version and capabilities in `_meta`; the
        # server rejects a bare `initialize` with -32602, which is how the first version of this
        # script failed. Drive the handshake the way an actual client does.
        rpc(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "check-observability", "version": "1.0"},
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2025-06-18",
                        "io.modelcontextprotocol/clientCapabilities": {},
                    },
                },
            },
        )
        # The notification that closes the handshake. No reply is expected, so nothing is read.
        assert proc.stdin
        proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
        )
        proc.stdin.flush()
        listed = rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        tools = [t["name"] for t in listed.get("result", {}).get("tools", [])]
        check("tools_exposed", len(tools) == 7, f"{len(tools)} tools: {tools}")

        before_rows = rows_in_segments(Path(LEDGER_DIR))
        before_segments = segments(Path(LEDGER_DIR))

        # --- 2. a gate call must WRITE A ROW --------------------------------
        gate_text = call_tool(proc, "gate", {"action": "git status", "run_id": "obs-check"}, 3)
        gate = json.loads(gate_text)
        check(
            "gate_answers",
            "decision" in gate,
            f"decision={gate.get('decision')!r}",
        )

        after_rows = rows_in_segments(Path(LEDGER_DIR))
        check(
            "gate_records_a_row",
            after_rows > before_rows,
            f"segments {before_segments}->{segments(Path(LEDGER_DIR))}, rows {before_rows}->{after_rows}",
        )

        # --- 3. the row must carry the caller's ids -------------------------
        # Read the newest segment and find this call's row.
        newest = None
        for f in sorted(Path(LEDGER_DIR).glob("ledger-*.jsonl"), key=lambda p: p.stat().st_mtime):
            newest = f
        row = {}
        if newest is not None:
            for line in reversed(newest.read_text(errors="ignore").splitlines()):
                if line.strip():
                    try:
                        row = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    break
        decision = row.get("Decision", row)
        check(
            "row_carries_run_id",
            decision.get("run_id") == "obs-check",
            f"run_id={decision.get('run_id')!r}",
        )
        check(
            "row_carries_wall_clock",
            isinstance(decision.get("wall_nanos"), int) and decision["wall_nanos"] > 1_000_000_000_000_000_000,
            f"wall_nanos={decision.get('wall_nanos')!r} (epoch-scale, not process-relative)",
        )
        check(
            "row_carries_tool",
            decision.get("tool") == "gate",
            f"tool={decision.get('tool')!r}",
        )

        # --- 4. the command text must NOT appear in the row ------------------
        # The ledger records a hash of the state, never the state: a state can hold a file, and a
        # file can hold a secret.
        blob = json.dumps(decision)
        check(
            "command_text_absent",
            "git status" not in blob,
            "the action itself is not in the ledger, only a hash of it",
        )
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()

    # --- 5. the operator's view must agree with the disk --------------------
    stats = cli_ledger("stats")
    report["stats"] = stats
    decisions = 0
    for line in stats.splitlines():
        if line.startswith("decisions:"):
            decisions = int(line.split(":")[1].strip())
    check(
        "stats_reads_what_is_on_disk",
        decisions > 0,
        f"stats reports {decisions} decision(s); the live store must not read as empty",
    )
    # This check first asserted `"p50" in stats`, which the BLENDED latency lines already
    # contain -- so it passed while the per-tool block was entirely missing from the CLI. That is
    # the defect this project keeps paying for, caught here by running the check against the
    # deployed binary and reading the output instead of the exit code. Assert the block itself.
    check(
        "stats_has_per_tool_block",
        "per tool:" in stats,
        "the per-tool breakdown is printed",
    )
    # And assert the GATE row specifically: it is the tool that must now record, and its verdicts
    # are what make "what did the gate allow yesterday" answerable.
    check(
        "stats_shows_gate_verdicts",
        "gate" in stats and any(v in stats for v in ("allow", "ask", "deny")),
        "the gate's verdict counts appear per tool",
    )

    report["failures"] = failures
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print("=== deployed laya observability ===")
        for name, r in report["checks"].items():
            print(f"  {'PASS' if r['ok'] else 'FAIL'}  {name:<28} {r['detail']}")
        print("\n=== laya ledger stats (the operator's view) ===")
        for line in stats.splitlines():
            print(f"  {line}")
    if failures:
        print(f"\nVERDICT: {len(failures)} CHECK(S) FAILED", file=sys.stderr)
        return 1
    print("\nVERDICT: the ledger is recording AND readable")
    return 0


if __name__ == "__main__":
    sys.exit(main())
