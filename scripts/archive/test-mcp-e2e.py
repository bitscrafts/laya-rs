#!/usr/bin/env python3
"""End-to-end test of the laya-rs MCP server over REAL stdio, both directions.

Not a unit test of a function — this drives the actual binary with the actual
MCP wire protocol, because that is the thing a client (Hermes, pi, Claude Code)
will do. A unit test of `handle()` would pass while the transport, the handshake
or the readiness preflight was broken.

Cases:
  1. Laya UP   -> initialize, tools/list, tools/call. Verify route is absent.
  2. Laya DOWN -> the process must REFUSE to serve: non-zero exit, diagnostic on
                  stderr, and no MCP handshake on stdout.
"""
import json
import os
import subprocess
import sys

BIN = "/projects/laya-rs/target/debug/laya-mcp"
LIVE = "http://10.10.10.217:8083"
DOWN = "http://127.0.0.1:9"


def rpc(proc, obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def read_lines(proc, n, timeout_hint=""):
    """Read up to n lines from the child's stdout, non-blocking-ish."""
    out = []
    for _ in range(n):
        line = proc.stdout.readline()
        if not line:
            break
        out.append(line.rstrip("\n"))
    return out


def run_case(endpoint, label):
    env = dict(os.environ)
    env["LAYA_URL"] = endpoint
    print(f"\n{'='*70}\n{label}  (LAYA_URL={endpoint})\n{'='*70}")
    proc = subprocess.Popen(
        [BIN],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    try:
        # The MCP handshake.
        rpc(proc, {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "laya-e2e", "version": "0.1"},
            },
        })
        lines = read_lines(proc, 1)
        if not lines:
            proc.stdin.close()
            rc = proc.wait(timeout=15)
            err = proc.stderr.read()
            print(f"  no handshake; process exited rc={rc}")
            print("  stderr:")
            for l in err.strip().splitlines():
                print(f"    {l}")
            return rc, err

        init = json.loads(lines[0])
        info = init.get("result", {}).get("serverInfo", {})
        print(f"  initialize OK -> serverInfo={info}")

        # The initialized notification is required before other requests.
        rpc(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})

        # tools/list
        rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        raw = read_lines(proc, 1)
        tools = json.loads(raw[0]).get("result", {}).get("tools", []) if raw else []
        names = sorted(t.get("name", "") for t in tools)
        print(f"  tools/list -> {len(names)} tools: {names}")
        if "route" in names:
            print("  *** FAIL: `route` must NOT be exposed ***")
        else:
            print("  route correctly ABSENT")

        # tools/call on gate — the fail-closed path (should be an ERROR while the
        # decision path is unimplemented, even when Laya is up).
        rpc(proc, {
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "gate", "arguments": {"action": "rm -rf /tmp/x"}},
        })
        raw = read_lines(proc, 1)
        if raw:
            resp = json.loads(raw[0])
            if "error" in resp:
                print(f"  tools/call gate -> protocol ERROR: {resp['error'].get('message','')[:90]}")
            else:
                content = resp.get("result", {}).get("content", [])
                text = content[0].get("text", "") if content else ""
                print(f"  tools/call gate -> result: {text[:90]}")

        # tools/call on an unknown tool — must be an error, not a success.
        rpc(proc, {
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "nonexistent", "arguments": {}},
        })
        raw = read_lines(proc, 1)
        if raw:
            resp = json.loads(raw[0])
            kind = "ERROR" if "error" in resp else "SUCCESS(!)"
            print(f"  tools/call nonexistent -> {kind}")

        # Close stdin: the bounded lifetime must let the process exit.
        proc.stdin.close()
        rc = proc.wait(timeout=20)
        print(f"  after EOF -> exit rc={rc}  (bounded lifetime; must not hang)")
        return rc, proc.stderr.read()
    except subprocess.TimeoutExpired:
        proc.kill()
        print("  *** FAIL: the process HUNG (bounded-lifetime fix not working) ***")
        return -1, ""


print("BINARY:", BIN)
if not os.path.exists(BIN):
    print("not built yet"); sys.exit(2)

# ---- case 1: Laya up --------------------------------------------------------
rc_up, err_up = run_case(LIVE, "CASE 1 — Laya is UP")
# ---- case 2: Laya down -----------------------------------------------------
rc_down, err_down = run_case(DOWN, "CASE 2 — Laya is DOWN (must refuse to serve)")

print(f"\n{'='*70}\nSUMMARY\n{'='*70}")
print(f"  Laya UP   : exit={rc_up}   (0 expected; it served and exited on EOF)")
print(f"  Laya DOWN : exit={rc_down}  (non-zero expected; it must refuse)")
print(f"  the DOWN diagnostic on stderr:")
for l in err_down.strip().splitlines()[:8]:
    print(f"    {l}")
