#!/usr/bin/env python3
"""Re-run the word-count THRESHOLD measurement, and fail if the head stops tracking.

This exists because the first `wc` example failed silently: a 3-option `choice` answered
MEDIUM for both a 13-word and a 412-word document, so the example's control certified nothing
while every check still passed. A demo that cannot fail is the defect class this project has
paid for repeatedly, so the measurement is a check with a non-zero exit.

What it asserts, and why these two thresholds: 300 and 100 are the boundaries the chained
example routes on. Both were measured 0-wrong over 7 points each, INCLUDING the exact boundary
(299/300/301 and 100/101), because an off-by-one here would put a document in the wrong band
while still "working".

Exit 0 only if every comparison is right. Any wrong answer exits 1 with the table printed, so
a silent regression in the head's number handling cannot pass as a green run.
"""
import json
import os
import subprocess
import sys

MCP = os.environ.get("LAYA_MCP_BIN", "/root/.local/bin/laya-mcp")

THRESHOLDS = [
    # (threshold, counts to test -- the exact boundary is non-negotiable)
    (300, [5, 100, 299, 300, 301, 412, 1000, 5000]),
    (100, [5, 13, 50, 100, 101, 150, 299]),
]


def meta():
    return {
        "io.modelcontextprotocol/protocolVersion": "2024-11-05",
        "io.modelcontextprotocol/clientCapabilities": {},
    }


def send(proc, msg):
    proc.stdin.write((json.dumps(msg) + "\n").encode())
    proc.stdin.flush()


def recv(proc, want):
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("the server closed the stream")
        try:
            d = json.loads(line)
        except Exception:
            continue
        if d.get("id") == want:
            return d


def ask(proc, i, count, threshold):
    state = f"The word count is {count}."
    question = f"Is the word count greater than {threshold}?"
    send(proc, {
        "jsonrpc": "2.0", "id": i, "method": "tools/call",
        "params": {
            "name": "decide",
            "arguments": {"state": state, "questions": {"q": {"type": "noul", "instructions": question}}},
            "_meta": meta(),
        },
    })
    r = recv(proc, i)
    if "error" in r:
        return None, None, r["error"].get("message", "")[:80]
    body = json.loads(r["result"]["content"][0]["text"])
    pay = body.get("payloads", {}).get("q", {})
    return pay.get("label"), pay.get("confidence"), None


def main():
    env = dict(os.environ)
    env.setdefault("LAYA_LEDGER_DIR", "/var/lib/laya-ledger")
    proc = subprocess.Popen([MCP], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, env=env)
    send(proc, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "wc-boundary", "version": "1"}, "_meta": meta()}})
    recv(proc, 1)
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})

    wrong = 0
    total = 0
    i = 100
    for threshold, counts in THRESHOLDS:
        print(f"threshold: greater than {threshold}?")
        for n in counts:
            i += 1
            want = "yes" if n > threshold else "no"
            got, conf, err = ask(proc, i, n, threshold)
            total += 1
            if err:
                print(f"  {n:>5}  ERROR {err}")
                wrong += 1
                continue
            ok = got == want
            if not ok:
                wrong += 1
            print(f"  {n:>5}  want={want:<4} got={str(got):<5} conf={conf}  {'OK' if ok else 'WRONG'}")
        print()

    proc.terminate()

    print(f"VERDICT: {total - wrong}/{total} correct across {len(THRESHOLDS)} thresholds")
    if wrong:
        print(f"FAIL: {wrong} wrong. The head stopped tracking the number -- the chained")
        print("      example would route documents into the wrong band while still running.")
        return 1
    print("PASS: both thresholds track the count, including the exact boundary.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
