#!/usr/bin/env python3
"""THE POC TEST, corrected: full chain AND a genuinely fail-closed case.

Corrections over the first version, each because the first version's check was wrong:

  1. It started the sidecar on a TEMPORARY socket while the server used the DEFAULT
     one, so it was exercising a different path than it claimed. Now the server socket
     is set explicitly for BOTH the success and the failure case.
  2. The "fail closed" step killed the temporary sidecar, which the server was never
     talking to — so it would have passed even if fail-closed were broken. Now it
     points the server at a socket that definitely does not exist, which is the
     condition being tested.
  3. It never asserted the success case actually SUCCEEDED (only printed it). Now a
     failure to get a decision is a test failure.
"""
import json
import os
import socket
import subprocess
import sys
import time

ROOT = "/projects/laya-rs"
PY = f"{ROOT}/.venv-laya/bin/python"
SIDECAR = f"{ROOT}/scripts/laya-sidecar.py"
MCP = f"{ROOT}/target/release/laya-mcp"
HEAD = "/projects/_tmp/laya-gguf/laya-head.safetensors"
LIVE_SOCK = "/run/laya/sidecar.sock"
DEAD_SOCK = f"/run/laya/definitely-not-here-{os.getpid()}.sock"

failures = []


def check(cond, label):
    print(f"  {'ok  ' if cond else 'FAIL'} {label}")
    if not cond:
        failures.append(label)


def start_sidecar(sock):
    if sock == LIVE_SOCK and os.path.exists(sock):
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.settimeout(5); s.connect(sock); s.close()
            print("  reusing the already-running sidecar")
            return None
        except OSError:
            pass
    proc = subprocess.Popen(
        [PY, SIDECAR, "--socket", sock, "--head", HEAD],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
        start_new_session=True,
    )
    deadline = time.time() + 180
    while time.time() < deadline:
        if os.path.exists(sock):
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.settimeout(5); s.connect(sock); s.close()
                print("  sidecar ready")
                return proc
            except OSError:
                pass
        if proc.poll() is not None:
            print(proc.stderr.read()[-1200:]); sys.exit(3)
        time.sleep(0.5)
    proc.kill(); sys.exit(3)


def start_mcp(sock):
    env = dict(os.environ)
    env["LAYA_SIDECAR_SOCKET"] = sock
    p = subprocess.Popen([MCP], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True, env=env)
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "poc", "version": "0.1"}}}) + "\n")
    p.stdin.flush()
    p.stdout.readline()
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    p.stdin.flush()
    return p


def call(p, rid, name, arguments):
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                              "params": {"name": name, "arguments": arguments}}) + "\n")
    p.stdin.flush()
    line = p.stdout.readline()
    if not line:
        return None
    r = json.loads(line)
    if "error" in r:
        return {"__error__": r["error"]}
    return json.loads(r["result"]["content"][0]["text"])


QUESTIONS = {
    "department": {"type": "choice", "instructions": "Which team should handle this?",
                   "criteria": {"billing": "invoices and refunds",
                                "technical": "bugs", "sales": "pricing"}},
    "churn_risk": {"type": "noul", "instructions": "Does the user threaten to cancel?"},
}
STATE = "Duplicate charge on invoice 4411. Refund requested. This is unacceptable."

print("=== CASE A: the head is UP — decide and gate must return REAL answers ===")
sc = start_sidecar(LIVE_SOCK)
mcp = start_mcp(LIVE_SOCK)
try:
    t0 = time.time()
    d = call(mcp, 2, "decide", {"state": STATE, "questions": QUESTIONS})
    wall = (time.time() - t0) * 1000
    check(d is not None and "__error__" not in d, f"decide returned an answer (wall {wall:.0f} ms)")
    if d and "__error__" not in d:
        print(f"       {json.dumps(d)[:300]}")
        ans = d.get("answers", {})
        dep = ans.get("department", {})
        check(dep.get("choice") in QUESTIONS["department"]["criteria"],
              f"choice {dep.get('choice')!r} is a DECLARED option")
        check("noul" in ans.get("churn_risk", {}), "noul answer present")

    g = call(mcp, 3, "gate", {"action": "rm -rf node_modules && npm install"})
    check(g is not None and "__error__" not in g, "gate returned a decision")
    if g and "__error__" not in g:
        print(f"       gate -> decision={g.get('decision')!r} band={g.get('band')!r} "
              f"conf={g.get('confidence')}")
        check(g.get("decision") in ("allow", "deny", "ask"),
              "gate decision is one of allow/deny/ask")
    mcp.stdin.close(); mcp.wait(timeout=20)
finally:
    if sc:
        sc.terminate(); sc.wait(timeout=10)

print("\n=== CASE B: the head is DOWN — the gate MUST fail closed ===")
# Point the server at a socket that does not exist. This is the condition under test;
# the earlier version killed a sidecar the server was not using, so it proved nothing.
mcp = start_mcp(DEAD_SOCK)
try:
    g = call(mcp, 4, "gate", {"action": "drop the production database",
                              "irreversible": True})
    check(g is not None, "gate answered even with no head")
    if g and "__error__" not in g:
        print(f"       {json.dumps(g)}")
        check(g.get("decision") == "ask", f"decision is `ask`, got {g.get('decision')!r}")
        check(g.get("decision") != "allow", "NEVER `allow` while the head is down")
        check(g.get("reason") == "sidecar-unavailable", "the reason names the cause")

    # decide has no safe fallback — it must be an ERROR, not a fabricated answer
    d = call(mcp, 5, "decide", {"state": STATE, "questions": QUESTIONS})
    check(d is not None and "__error__" in d,
          "decide ERRORS when the head is down (never a made-up answer)")
    if d and "__error__" in d:
        print(f"       decide error: {d['__error__'].get('message','')[:120]}")

    mcp.stdin.close(); mcp.wait(timeout=20)
finally:
    try:
        mcp.terminate(); mcp.wait(timeout=10)
    except Exception:
        mcp.kill()

print("\n" + "=" * 68)
if failures:
    print(f"POC RESULT: {len(failures)} FAILURE(S)")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("POC RESULT: ALL CHECKS PASSED")
print("  MCP client -> laya-mcp -> sidecar -> llama.cpp /embedding -> head -> answer")
print("  and: no head -> gate `ask` (never allow), decide errors")
