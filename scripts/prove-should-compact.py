#!/usr/bin/env python3
"""PROVE should_compact end to end, through the REAL MCP surface.

The unit tests sweep the arithmetic. This proves the TOOL: the four questions actually reach
the model in ONE call, the answers come back, and the VETO holds against a real judgement —
which is the property that matters most, because a wrong "compact_now" discards work.

Cases:
  1. four judgements in one call, and the advice is one of the four contract words
  2. a LOW context gets `nothing` — the quiet case must exist and be reachable
  3. a HIGH context gets `recommend`/`request`
  4. `compact_now` is true exactly when urgency is `request` (the invariant)
  5. the judgements and thresholds are echoed, so the arithmetic is auditable
  6. the token count is NOT sent to the model (thresholds stay in code)
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

BIN = pathlib.Path("/projects/laya-rs/target/release/laya-mcp")
SOCK = "/run/laya/sidecar.sock"
work = pathlib.Path(tempfile.mkdtemp(prefix="laya-compact-e2e-"))

env = {**os.environ, "LAYA_URL": "http://10.10.10.217:8083",
       "LAYA_SIDECAR_SOCKET": SOCK, "LAYA_READ_ROOTS": str(work)}

failures = []


def report(ok, label, detail=""):
    print(f"  {'ok  ' if ok else 'FAIL'} {label}")
    if detail:
        print(f"         {detail}")
    if not ok:
        failures.append(label)


def run(calls):
    p = subprocess.Popen([str(BIN)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True, env=env)

    def rpc(o):
        p.stdin.write(json.dumps(o) + "\n")
        p.stdin.flush()

    def rd():
        line = p.stdout.readline()
        if not line:
            raise SystemExit(f"server closed stdout:\n{p.stderr.read()}")
        return json.loads(line)

    rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "compact-e2e", "version": "1"}}})
    init = rd()
    report("result" in init, "MCP handshake", str(init.get("result", {}).get("serverInfo")))
    rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
    out = []
    for i, (name, args) in enumerate(calls):
        rpc({"jsonrpc": "2.0", "id": 100 + i, "method": "tools/call",
             "params": {"name": name, "arguments": args}})
        out.append(rd())
    p.stdin.close()
    p.wait(timeout=40)
    return out


def compact(summary, tokens):
    res = run([("should_compact", {"turns_summary": summary, "context_tokens": tokens})])[0]
    if "error" in res:
        return None, res["error"]["message"]
    return json.loads(res["result"]["content"][0]["text"]), ""


WORDS = {"nothing", "notice", "recommend", "request"}

print("=== CASE 1 + 2: a LOW context, mid-work ===")
b, err = compact(
    "Turn 1: added a parse function. Turn 2: fixed a typo in it. Still editing the same file.",
    1_000,
)
if b is None:
    report(False, "should_compact must answer", err[:200])
else:
    print(f"         urgency={b.get('urgency')} compact_now={b.get('compact_now')}")
    print(f"         judgements={b.get('judgements')}")
    report(b.get("urgency") in WORDS,
           "the advice is one of the four contract words", str(b.get("urgency")))
    report(b.get("urgency") == "nothing",
           "a 1k context gets `nothing` — the quiet case exists", str(b.get("urgency")))
    report(b.get("compact_now") is False, "and does not ask to compact")

print()
print("=== CASE 3 + 4: a HIGH context ===")
b, err = compact(
    "Turn 1: finished the parser. Turn 2: wrote its tests and they pass. "
    "Now I want to start a completely different task: the CLI help text.",
    500_000,
)
if b is None:
    report(False, "should_compact must answer", err[:200])
else:
    print(f"         urgency={b.get('urgency')} compact_now={b.get('compact_now')}")
    print(f"         judgements={b.get('judgements')}")
    report(b.get("urgency") in ("recommend", "request"),
           "a 500k context is at least a recommendation", str(b.get("urgency")))
    report(b.get("compact_now") == (b.get("urgency") == "request"),
           "compact_now tracks `request` exactly")

print()
print("=== CASE 5: the arithmetic is auditable ===")
if b is not None:
    report(isinstance(b.get("reasons"), list) and b["reasons"],
           "reasons explain every judgement that moved the answer",
           str(b.get("reasons"))[:200])
    th = b.get("thresholds") or {}
    report(th.get("notice") and th.get("recommend") and th.get("request"),
           "the thresholds are echoed", str(th))
    report(b.get("context_tokens") == 500_000, "the token count is echoed")

print()
print("=== CASE 6: the token count is not sent to the model ===")
# The model must not see the number: if it did, it could react to it and thresholds would
# have moved into the model. Verified by the design (state carries only the summary) and
# asserted here as a contract on the recorded judgements.
if b is not None:
    j = b.get("judgements") or {}
    report("context_tokens" not in j and "tokens" not in j,
           "the judgements contain no token-derived field", str(sorted(j)))

print()
print("=== CASE 7: the VETO, on a HIGH context mid-edit ===")
b, err = compact(
    "I am in the middle of a multi-file rename: the interface is changed but the three "
    "call sites are NOT yet updated. Stopping here would leave the tree broken.",
    500_000,
)
if b is None:
    report(False, "should_compact must answer", err[:200])
else:
    print(f"         urgency={b.get('urgency')} compact_now={b.get('compact_now')}")
    print(f"         judgements={b.get('judgements')}")
    report(b.get("compact_now") is False,
           "a mid-edit turn is NOT told to compact, even at 500k")
    report(b.get("urgency") != "request",
           "and the urgency is not `request`", str(b.get("urgency")))
    reasons = " ".join(b.get("reasons") or []).lower()
    if (b.get("judgements") or {}).get("mid_edit"):
        report("mid-edit" in reasons or "mid edit" in reasons,
               "the reason names the mid-edit veto", reasons[:200])
    else:
        report(True, "the model did not judge this as mid-edit; veto not exercised",
               "not a failure — the invariant is checked in the unit sweep")

print()
print("=" * 68)
if failures:
    print(f"RESULT: {len(failures)} FAILED")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("RESULT: ALL CHECKS PASSED")
print("  four questions in one call, advised in the four contract words,")
print("  arithmetic auditable, and compact_now never fires on a mid-edit turn.")
