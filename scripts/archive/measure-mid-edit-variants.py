#!/usr/bin/env python3
"""Find a `mid_edit` phrasing that actually works — or conclude the veto must go.

MEASURED FIRST: the original phrasing agreed with a known-intended answer only **1/6** times
(3 false positives, 2 false negatives). Since `mid_edit` is a VETO, that is worse than not
having it: the false positives suppress every genuine compaction request, and the false
negatives permit exactly the compaction the veto exists to prevent.

So: four candidate phrasings, the same six cases with known intended answers, and pick by
agreement. If none reaches a usable rate, the veto is removed and the reason documented —
a safety mechanism that does not work must not be shipped as if it did.
"""
import json
import socket

SOCK = "/run/laya/sidecar.sock"

CASES = [
    ("NOT", "Turn 1: added a parse function. Turn 2: ran the tests and they all pass. "
            "The task is complete.", False),
    ("NOT", "Turn 1: finished the parser and verified it. Turn 2: nothing is in progress. "
            "Next I want to start a completely different task: the CLI help text.", False),
    ("NOT", "Turn 1: answered the user's question about the config format. Nothing pending.",
     False),
    ("MID", "I changed the interface but the three call sites are NOT yet updated. "
            "Stopping here would leave the tree broken.", True),
    ("MID", "I am part-way through rewriting the parser: the new function is added but the "
            "old call sites still reference the removed one.", True),
    ("NOT", "I was editing three files. I have finished all three and the build is green.",
     False),
]

VARIANTS = {
    "A_original_double": "Is an edit in progress that spans more than one step and would be "
                         "broken by losing the earlier conversation?",
    "B_simple_unfinished": "Is work currently IN PROGRESS and not yet finished?",
    "C_would_break": "Would losing this conversation BREAK an unfinished change?",
    # Inverted: a high p means the work IS finished, so mid_edit = NOT high. Measuring the
    # inverse is worth trying because the model may answer the positive, concrete form better.
    "D_inverted_finished": "Has the current unit of work been FINISHED, with nothing left "
                           "in progress?",
}


def ask(summary, instructions):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(120)
    s.connect(SOCK)
    req = {"op": "predict",
           "state": {"turn_summary": summary},
           "questions": {"q": {"type": "noul", "instructions": instructions}}}
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
        return None
    return r["answers"]["q"].get("noul")


for name, instructions in VARIANTS.items():
    inverted = "inverted" in name
    correct = 0
    fp = fn = 0
    rows = []
    for _tag, summary, intended in CASES:
        p = ask(summary, instructions)
        if p is None:
            rows.append((_tag, None, None))
            continue
        judged = p >= 0.5
        mid_edit = (not judged) if inverted else judged
        if mid_edit == intended:
            correct += 1
        elif mid_edit and not intended:
            fp += 1
        else:
            fn += 1
        rows.append((_tag, round(p, 3), mid_edit))

    print(f"\n  {name}")
    print(f"    instructions: {instructions[:78]}...")
    for tag, p, me in rows:
        print(f"      intended={tag:<3}  p={p}  mid_edit={me}")
    print(f"    AGREEMENT {correct}/{len(CASES)}   false-positive {fp}   false-negative {fn}")
