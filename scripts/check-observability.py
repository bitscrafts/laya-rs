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

**An OFF ledger exits non-zero.** "Off" and "broken" must not look the same, and the first version
of this script could never see "off": it forced `LAYA_LEDGER_DIR` into the server's environment
itself, so a deployment that had lost the variable still passed. It now

  * reads the variable from the host's MCP entry (`HERMES_CONFIG`, default
    `/root/.hermes/config.yaml`) and fails when the entry does not set it;
  * starts the server with ONLY that environment and requires its startup `ledger` event to say
    `enabled: true` -- the server's own view, which a shell variable cannot certify;
  * runs a control server with the variable removed and requires it to announce `enabled: false`
    as a warning, so the off state is proven to be LOUD rather than assumed to be;
  * accepts `LAYA_OBS_SIMULATE_OFF=1`, which starts the main server without the variable. The
    script must then exit 1. That is the control for the script itself.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import threading
from pathlib import Path

MCP_BIN = os.environ.get("LAYA_MCP_BIN", "/root/.local/bin/laya-mcp")
CLI_BIN = os.environ.get("LAYA_CLI_BIN", "/root/.local/bin/laya")
LEDGER_DIR = os.environ.get("LAYA_LEDGER_DIR", "/var/lib/laya-ledger")


HERMES_CONFIG = os.environ.get("HERMES_CONFIG", "/root/.hermes/config.yaml")


def configured_env() -> dict[str, str] | None:
    """The `env:` block of the host's `laya` MCP entry, or None when there is no such entry.

    Parsed by indentation rather than with a YAML library, which the script must not need: the
    entry is `  laya:` / `    command: ...` / `    env:` / `      KEY: value`.
    """
    try:
        lines = Path(HERMES_CONFIG).read_text().splitlines()
    except OSError:
        return None
    start = next((i for i, l in enumerate(lines) if re.fullmatch(r"\s{2}laya:\s*", l)), None)
    if start is None:
        return None
    env: dict[str, str] = {}
    in_env = False
    for line in lines[start + 1 :]:
        if line.strip() and not line.startswith("    "):
            break  # the next sibling entry
        if re.fullmatch(r"\s{4}env:\s*", line):
            in_env = True
            continue
        if in_env:
            m = re.fullmatch(r"\s{6}([A-Za-z_][A-Za-z0-9_]*):\s*(.*?)\s*", line)
            if not m:
                in_env = False
                continue
            env[m.group(1)] = m.group(2).strip("\"'")
    return env


class Server:
    """A spawned `laya-mcp`, with its stderr drained on a thread so the pipe can never fill."""

    def __init__(self, env: dict[str, str]):
        self.stderr_lines: list[str] = []
        self.proc = subprocess.Popen(
            [MCP_BIN],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            bufsize=1,
        )
        assert self.proc.stderr
        self._t = threading.Thread(target=self._drain, daemon=True)
        self._t.start()

    def _drain(self) -> None:
        assert self.proc.stderr
        for line in self.proc.stderr:
            self.stderr_lines.append(line.rstrip("\n"))

    def events(self, name: str) -> list[dict]:
        """Structured events of one kind, parsed from the JSON lines on stderr."""
        out = []
        for line in list(self.stderr_lines):
            if line.startswith("{"):
                try:
                    e = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if e.get("event") == name:
                    out.append(e)
        return out

    def wait_for_event(self, name: str, seconds: float = 20.0) -> list[dict]:
        import time

        end = time.time() + seconds
        while time.time() < end:
            found = self.events(name)
            if found:
                return found
            if self.proc.poll() is not None:
                break
            time.sleep(0.05)
        return self.events(name)

    def stop(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def handshake(proc: subprocess.Popen) -> None:
    """Drive the MCP handshake the way a real client does (see the note in `main`)."""
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
    assert proc.stdin
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    proc.stdin.flush()


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

    # --- 0. the host's own configuration must turn the ledger on -----------
    # The server's environment comes from the host's MCP entry, not from this shell, so this
    # script builds the server's environment FROM that entry. A script that exported the variable
    # itself would certify the shell and pass while the deployment was misconfigured -- which is
    # exactly how the ledger came to be off for the whole life of the first deployment.
    cfg = configured_env()
    check(
        "host_config_sets_ledger_dir",
        cfg is not None and bool(cfg.get("LAYA_LEDGER_DIR")),
        f"{HERMES_CONFIG}: laya.env = {cfg!r}"
        if cfg is not None
        else f"no `laya` MCP entry found in {HERMES_CONFIG}",
    )
    server_env = dict(os.environ)
    server_env.pop("LAYA_LEDGER_DIR", None)
    server_env.update(cfg or {})
    if os.environ.get("LAYA_OBS_SIMULATE_OFF") == "1":
        # The control for THIS script: start the main server as if the host had lost the variable.
        server_env.pop("LAYA_LEDGER_DIR", None)
        report["simulated_off"] = True
    ledger_dir = Path(server_env.get("LAYA_LEDGER_DIR", LEDGER_DIR))

    # --- 1. the server announces, and it announces ON ---------------------
    server = Server(server_env)
    proc = server.proc
    try:
        # A real MCP client announces its protocol version and capabilities in `_meta`; the
        # server rejects a bare `initialize` with -32602, which is how the first version of this
        # script failed. Drive the handshake the way an actual client does.
        handshake(proc)
        announced = server.wait_for_event("ledger")
        check(
            "server_announces_ledger_state",
            len(announced) == 1,
            f"{len(announced)} `ledger` startup event(s) on stderr",
        )
        ev = announced[0] if announced else {}
        check(
            "ledger_is_on",
            ev.get("enabled") is True and ev.get("dir") == str(ledger_dir),
            f"the server's own view: {json.dumps(ev)}",
        )

        listed = rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        tools = [t["name"] for t in listed.get("result", {}).get("tools", [])]
        check("tools_exposed", len(tools) == 7, f"{len(tools)} tools: {tools}")

        before_rows = rows_in_segments(ledger_dir)
        before_segments = segments(ledger_dir)

        # --- 2. a gate call must WRITE A ROW --------------------------------
        # The command carries a distinctive token, so a leak cannot hide in a common word.
        secret = "obs-secret-canary-7f3a"
        gate_text = call_tool(
            proc,
            "gate",
            {"action": f"echo {secret}", "run_id": "obs-check", "node_id": "obs-node"},
            3,
        )
        gate = json.loads(gate_text)
        check("gate_answers", "decision" in gate, f"decision={gate.get('decision')!r}")

        after_rows = rows_in_segments(ledger_dir)
        check(
            "gate_records_a_row",
            after_rows > before_rows,
            f"segments {before_segments}->{segments(ledger_dir)}, rows {before_rows}->{after_rows}",
        )

        # --- 3. the row must carry the caller's ids -------------------------
        # Find THIS call's row by its run id, across every segment -- not "the last line of the
        # newest file", which is whichever process happened to write last.
        mine: list[dict] = []
        for f in sorted(ledger_dir.glob("ledger-*.jsonl")):
            for line in f.read_text(errors="ignore").splitlines():
                try:
                    r = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if r.get("run_id") == "obs-check" and r.get("tool") == "gate":
                    mine.append(r)
        decision = max(mine, key=lambda r: r.get("wall_nanos", 0)) if mine else {}
        check(
            "row_carries_run_and_node_id",
            decision.get("run_id") == "obs-check" and decision.get("node_id") == "obs-node",
            f"run_id={decision.get('run_id')!r} node_id={decision.get('node_id')!r}",
        )
        check(
            "row_carries_wall_clock",
            isinstance(decision.get("wall_nanos"), int)
            and decision["wall_nanos"] > 1_000_000_000_000_000_000,
            f"wall_nanos={decision.get('wall_nanos')!r} (epoch-scale, not process-relative)",
        )
        check(
            "row_carries_the_verdict",
            decision.get("gate_decision") in ("allow", "ask", "deny")
            and decision.get("command_sha256"),
            f"gate_decision={decision.get('gate_decision')!r} "
            f"command_sha256={decision.get('command_sha256')!r}",
        )

        # --- 4. the command text must NOT appear ANYWHERE in the ledger -----
        # Every segment, every byte -- a leak into a question id or a note would not be in "the
        # row" this script located above.
        leaked = [
            f.name
            for f in ledger_dir.glob("ledger-*.jsonl")
            if secret in f.read_text(errors="ignore")
        ]
        check(
            "command_text_absent",
            not leaked,
            "the action itself is in no segment, only a hash of it"
            if not leaked
            else f"the command text leaked into {leaked}",
        )

        # --- 4b. the call emitted a structured event carrying the ids -------
        calls = [
            e
            for e in server.events("tool_call")
            if e.get("tool") == "gate" and e.get("run_id") == "obs-check"
        ]
        check(
            "tool_call_event_carries_ids",
            len(calls) == 1 and calls[0].get("node_id") == "obs-node"
            and calls[0].get("outcome") == "ok",
            f"{json.dumps(calls[0]) if calls else 'no tool_call event for run obs-check'}",
        )
    finally:
        server.stop()

    # --- 4c. the control: with the variable REMOVED the server must say so ---
    off_env = dict(os.environ)
    off_env.pop("LAYA_LEDGER_DIR", None)
    off = Server(off_env)
    try:
        off_events = off.wait_for_event("ledger")
    finally:
        off.stop()
    oe = off_events[0] if off_events else {}
    check(
        "off_is_announced_not_silent",
        len(off_events) == 1 and oe.get("enabled") is False and oe.get("level") == "warn",
        f"a server with no LAYA_LEDGER_DIR wrote: {json.dumps(oe) if oe else 'NOTHING'}",
    )

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
        "per tool:" in stats and "p99" in stats,
        "the per-tool breakdown is printed, with p99",
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
