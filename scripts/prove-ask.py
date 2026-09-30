#!/usr/bin/env python3
"""PROVE ask (L10) end to end, through the REAL MCP surface.

The unit tests pin the composition order and the cap offline. This proves the TOOL: real
binary, real handshake, real files, real command, real model.

The two properties that matter most, and that no unit test can establish:

  1. THE AGENT WRITES THE QUESTIONS. Three shapes at once (noul + choice + score), the
     caller's own ids, forwarded unchanged.
  2. THE COMMAND IS GATED BEFORE IT RUNS, AND A REFUSAL IS FINAL. The control case is
     `rm -rf /var/lib/postgresql/data`: if the gate refuses it, the data must STILL BE THERE
     afterwards. That checks the effect, not the message — a message saying "refused" while
     the command ran anyway is exactly the failure the source research records for level 6,
     where an agent routed around a block and wrote the file anyway.

Also: the response must carry ANSWERS and NOT the files or the output.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

BIN = pathlib.Path("/projects/laya-rs/target/release/laya-mcp")
SOCK = "/run/laya/sidecar.sock"
work = pathlib.Path(tempfile.mkdtemp(prefix="laya-ask-e2e-"))

# A real thing to refuse. A directory that exists and holds a file we can check afterwards.
guard = work / "protected-data"
guard.mkdir()
(guard / "IMPORTANT.txt").write_text("this must survive a refused command\n")

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
        "clientInfo": {"name": "ask-e2e", "version": "1"}}})
    init = rd()
    report("result" in init, "MCP handshake", str(init.get("result", {}).get("serverInfo")))
    rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
    out = []
    for i, (name, args) in enumerate(calls):
        rpc({"jsonrpc": "2.0", "id": 100 + i, "method": "tools/call",
             "params": {"name": name, "arguments": args}})
        out.append(rd())
    p.stdin.close()
    p.wait(timeout=60)
    return out


def ask(args, rest=None):
    calls = [("ask", args)] + list(rest or [])
    res = run(calls)
    first = res[0]
    if "error" in first:
        return None, first["error"]["message"]
    return json.loads(first["result"]["content"][0]["text"]), ""


def verdict_of(res):
    if "error" in res:
        return None, res["error"]["message"]
    return json.loads(res["result"]["content"][0]["text"]), ""


# ---------------------------------------------------------------- CASE 1
print("=== CASE 1: the agent writes the questions — all three shapes at once ===")
(work / "billing.py").write_text(
    "def prorate(amount, days, total_days):\n"
    "    return round(amount * days / total_days, 2)\n"
)
questions = {
    "is_bug": {"type": "noul",
               "instructions": "Does this code contain a bug that would produce a wrong "
                               "number?"},
    "area": {"type": "choice", "instructions": "Which area is this?",
             "criteria": {"billing": "money and invoicing", "auth": "logins and sessions",
                          "ui": "rendering and layout"}},
    "risk": {"type": "score", "instructions": "How risky is changing it?",
             "criteria": {"0": "trivial", "1": "ordinary", "2": "dangerous"}},
}
body, err = ask({
    "questions": questions,
    "paths": [str(work / "billing.py")],
    "state": "The user says the proration test fails because of rounding.",
})
if body is None:
    report(False, "ask must answer", err[:300])
else:
    ans = body.get("answers") or {}
    print(f"         answers = {json.dumps(ans)[:240]}")
    print(f"         files_read={body.get('files_read')} state_bytes={body.get('state_bytes')} "
          f"questions_asked={body.get('questions_asked')}")
    report(set(ans.keys()) == set(questions.keys()),
           "all three of the caller's questions are answered, keyed by the caller's ids",
           str(sorted(ans.keys())))
    report(body.get("questions_asked") == 3, "three questions were asked, and it says so")
    report(isinstance(ans.get("risk"), dict) and "score" in ans["risk"],
           "the `score` shape came back as a score",
           json.dumps(ans.get("risk"))[:120])
    report(isinstance(ans.get("area"), dict) and "choice" in ans["area"],
           "the `choice` shape came back as a choice",
           json.dumps(ans.get("area"))[:120])

    # ------------------------------------------------------------ CASE 2
    print()
    print("=== CASE 2: the caller gets ANSWERS, never the files or the output ===")
    blob = json.dumps(body)
    report(body.get("content_in_context") is False,
           "content_in_context is explicitly false")
    report("round(amount * days" not in blob,
           "the file's TEXT is not in the response")
    # The path IS reported (a judgement must be attributable) and the file read is COUNTED,
    # but no file payload accompanies them — that is the difference between naming an input
    # and shipping it.
    report(body.get("files_read") == 1,
           "the file read is counted, so the caller knows what was judged")

    # ------------------------------------------------------------ CASE 3
    print()
    print("=== CASE 3: a command's output is judged, and the OUTPUT is not returned ===")
    body2, err2 = ask({
        "questions": {"tests_pass": {
            "type": "noul",
            "instructions": "Do the tests described in the output pass?"}},
        "command": "echo 'FAILED tests/test_billing.py::test_prorate - assert 2.34 == 2.33'",
    })
    if body2 is None:
        report(False, "ask with a command must answer", err2[:300])
    else:
        print(f"         answers = {json.dumps(body2.get('answers'))[:200]}")
        report(body2.get("command_ran") is True, "the tool reports that a command ran")
        report("assert 2.34" not in json.dumps(body2),
               "the command's OUTPUT is not in the response")
        p = (body2.get("answers") or {}).get("tests_pass") or {}
        report(isinstance(p.get("noul"), (int, float)),
               "and a judgement about it came back", json.dumps(p)[:160])

# ---------------------------------------------------------------- CASE 4
print()
print("=== CASE 4: A REFUSED COMMAND MUST NOT RUN — checked by EFFECT, not by message ===")
res = run([("ask", {
    "questions": {"harmful": {"type": "noul", "instructions": "is this destructive?"}},
    "command": f"rm -rf {guard}",
})])[0]
if "error" in res:
    print(f"         refused: {res['error']['message'][:200]}")
    report(True, "the ask was refused by the gate (reported as an error, not a success)")
else:
    print(f"         NOT refused: {json.dumps(res)[:220]}")
# THE EFFECT CHECK, which is the one that matters.
report((guard / "IMPORTANT.txt").is_file(),
       "the protected file STILL EXISTS after a refused command",
       "the gate refused and the command did not run" if (guard / "IMPORTANT.txt").is_file()
       else "THE COMMAND RAN DESPITE THE REFUSAL")

# ---------------------------------------------------------------- CASE 5
print()
print("=== CASE 5: a malformed block is refused, naming the fault ===")
for label, q, needle in [
    ("no questions at all", None, "questions"),
    ("an array instead of an object", [{"type": "noul"}], "object"),
    ("an empty object", {}, "empty"),
    ("a question with no type", {"q": {"instructions": "hi"}}, "type"),
    ("a type Laya does not have", {"q": {"type": "regression"}}, "regression"),
]:
    args = {"state": "x"}
    if q is not None:
        args["questions"] = q
    _, err = ask(args)
    ok = err is not None and needle in err
    report(ok, f"refused: {label}", (err or "NO ERROR")[:150] if not ok else "")

# ---------------------------------------------------------------- CASE 6
print()
print("=== CASE 6: nothing to judge is refused ===")
_, err = ask({"questions": {"q": {"type": "noul", "instructions": "hi"}}})
report(err is not None and "nothing to judge" in err,
       "a state with no summary, files or command is refused", (err or "NO ERROR")[:150])

print()
print("=" * 68)
if failures:
    print(f"RESULT: {len(failures)} FAILED")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("RESULT: ALL CHECKS PASSED")
print("  the agent wrote the questions, three shapes came back keyed by its own ids,")
print("  the response carried answers and not the content, and a refused command")
print("  was proven not to have run.")
