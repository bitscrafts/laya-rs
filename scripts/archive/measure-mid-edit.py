#!/usr/bin/env python3
"""Is the `mid_edit` question over-triggering? Measure before changing the design.

THE FINDING THAT PROMPTED THIS: an e2e case with the summary
"Turn 1: finished the parser. Turn 2: wrote its tests and they pass. Now I want to start a
completely different task" was judged `mid_edit: True`. Nothing was in progress, so a
correct judgement is `False`. Because `mid_edit` is a VETO, a false positive suppresses every
compaction request — which would make the tool useless in practice.

One sample is not a rate. This measures several summaries with a KNOWN intended answer and
reports the agreement, so any design change is made on a measurement rather than a hunch.
"""
import json
import socket

SOCK = "/run/laya/sidecar.sock"

# (label, summary, the correct mid_edit answer)
CASES = [
    ("clearly NOT mid-edit: work finished, nothing in progress",
     "Turn 1: added a parse function. Turn 2: ran the tests and they all pass. "
     "The task is complete.", False),
    ("clearly NOT mid-edit: gear switch, clean finish",
     "Turn 1: finished the parser and verified it. Turn 2: nothing is in progress. "
     "Next I want to start a completely different task: the CLI help text.", False),
    ("clearly NOT mid-edit: a question was answered",
     "Turn 1: answered the user's question about the config format. Nothing pending.", False),
    ("clearly MID-edit: half a refactor",
     "I changed the interface but the three call sites are NOT yet updated. "
     "Stopping here would leave the tree broken.", True),
    ("clearly MID-edit: editing one file across steps",
     "I am part-way through rewriting the parser: the new function is added but the old "
     "call sites still reference the removed one.", True),
    ("ambiguous: a long edit that just finished",
     "I was editing three files. I have finished all three and the build is green.", False),
]

QUESTIONS = {
    "mid_edit": {
        "type": "noul",
        "instructions": "Is an edit in progress that spans more than one step and would be "
                        "broken by losing the earlier conversation?",
    }
}


def ask(summary):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(120)
    s.connect(SOCK)
    req = {"op": "predict",
           "state": {"turn_summary": summary},
           "questions": QUESTIONS}
    s.sendall((json.dumps(req) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        c = s.recv(65536)
        if not c:
            break
        buf += c
    s.close()
    r = json.loads(buf.decode())
    if not r.get("ok"):
        return None, None
    a = r["answers"]["mid_edit"]
    return a.get("noul"), a.get("confidence")


print(f"  {'intended':>9}  {'p':>7}  {'judged':>7}  agrees?  case")
agree = 0
false_pos = 0
false_neg = 0
for label, summary, intended in CASES:
    p, conf = ask(summary)
    judged = None if p is None else (p >= 0.5)
    ok = judged == intended
    if ok:
        agree += 1
    elif judged is True and intended is False:
        false_pos += 1
    elif judged is False and intended is True:
        false_neg += 1
    print(f"  {str(intended):>9}  {p if p is None else round(p,3):>7}  "
          f"{str(judged):>7}  {'YES' if ok else 'NO ':>6}  {label}")

print()
print(f"  agreement            : {agree}/{len(CASES)}")
print(f"  false POSITIVE (says mid-edit when not) : {false_pos}")
print(f"  false NEGATIVE (says not when mid-edit) : {false_neg}")
print()
print("  A false POSITIVE is the dangerous-in-the-other-direction one for usefulness: since")
print("  `mid_edit` is a veto, it suppresses every compaction request.")
print("  A false NEGATIVE would let a mid-edit turn compact — which is why the veto exists.")
