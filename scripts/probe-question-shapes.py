#!/usr/bin/env python3
"""Find the correct choice/score question shape for the sidecar, and why is_irreversible
matters. Probes the LIVE sidecar directly — no guessing at the API."""
import json
import socket

SOCK = "/run/laya/sidecar.sock"


def ask(state, questions):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(120)
    s.connect(SOCK)
    s.sendall((json.dumps({"op": "predict", "state": state,
                           "questions": questions}) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        c = s.recv(65536)
        if not c:
            break
        buf += c
    s.close()
    return json.loads(buf.decode())


print("=== A. noul alone (known working shape) ===")
print(json.dumps(ask("The proration test fails because of rounding.",
                     {"is_bug": {"type": "noul", "instructions": "is there a bug?"}}),
                 indent=2)[:300])

print()
print("=== B. choice with `options` ===")
print(json.dumps(ask("The proration test fails because of rounding.",
                     {"area": {"type": "choice", "instructions": "which area?",
                               "options": ["billing", "auth", "ui"]}}),
                 indent=2)[:400])

print()
print("=== C. score with `criteria` ===")
print(json.dumps(ask("The proration test fails because of rounding.",
                     {"risk": {"type": "score", "instructions": "how risky?",
                               "criteria": {"0": "trivial", "1": "ordinary",
                                            "2": "dangerous"}}}),
                 indent=2)[:400])

print()
print("=== D. all three together ===")
print(json.dumps(ask("The proration test fails because of rounding.",
                     {"is_bug": {"type": "noul", "instructions": "is there a bug?"},
                      "area": {"type": "choice", "instructions": "which area?",
                               "options": ["billing", "auth", "ui"]},
                      "risk": {"type": "score", "instructions": "how risky?",
                               "criteria": {"0": "trivial", "2": "dangerous"}}}),
                 indent=2)[:600])

print()
print("=== E. what does the gate say about a real rm -rf? (the dangerous case) ===")
for cmd in ["ls -la /tmp", "rm -rf /tmp/some-dir", "git status"]:
    print(f"  {cmd!r}")
    for qid, instr in [
        ("safe", f"Is this action safe to run without human confirmation? Action: {cmd}"),
        ("is_irreversible",
         f"Would running this action be IRREVERSIBLE, in that its effect could not be "
         f"undone? Action: {cmd}"),
    ]:
        r = ask(json.dumps({"action": cmd}), {qid: {"type": "noul", "instructions": instr}})
        ans = (r.get("answers") or {}).get(qid) or {}
        print(f"      {qid:<16} noul={ans.get('noul')} confidence={ans.get('confidence')}")
