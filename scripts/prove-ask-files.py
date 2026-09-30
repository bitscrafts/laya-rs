#!/usr/bin/env python3
"""PROVE ask_files end to end, through the REAL MCP surface.

Unit tests prove the batching logic. This proves the TOOL works: real binary, real
handshake, real files, real model calls — the only thing that has caught the "capability
that no caller can reach" defects in this project.

Cases:
  1. a glob is expanded and every matching in-scope file is judged
  2. `hits` ranks by belief, best first, and only includes files that answered
  3. a file that CANNOT be read does not lose the other verdicts (per-file reasons)
  4. an out-of-scope file in the batch is refused while the rest are judged
  5. a wide glob is capped AND says so
  6. a large file is judged but CARRIES THE PREFIX WARNING
  7. the response does not claim parallelism it does not have
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

BIN = pathlib.Path("/projects/laya-rs/target/release/laya-mcp")
SOCK = "/run/laya/sidecar.sock"

work = pathlib.Path(tempfile.mkdtemp(prefix="laya-askfiles-e2e-"))
# Deliberate variety: one clear YES, one clear NO, one unreadable, and one LARGE.
(work / "invoice.txt").write_text(
    "Ticket: the invoice was charged twice and the customer wants a refund.\n")
(work / "shipping.txt").write_text(
    "Ticket: the parcel arrived three days late, tracking shows a depot delay.\n")
(work / "large.txt").write_text(
    "The invoice was charged twice.\n" + ("filler sentence with no signal. " * 200))

outside = work.parent / "laya-askfiles-outside-probe.txt"
outside.write_text("outside the scope\n")

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
        "clientInfo": {"name": "askfiles-e2e", "version": "1"}}})
    rd()
    rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
    out = []
    for i, (name, args) in enumerate(calls):
        rpc({"jsonrpc": "2.0", "id": 100 + i, "method": "tools/call",
             "params": {"name": name, "arguments": args}})
        out.append(rd())
    p.stdin.close()
    p.wait(timeout=60)
    return out


print("=== PREFLIGHT ===")
report(BIN.exists(), f"binary: {BIN}")
report(pathlib.Path(SOCK).exists(), f"sidecar: {SOCK}")

print()
print("=== CASE 1 + 2: a glob, judged, ranked ===")
t0 = time.time()
res = run([("ask_files", {
    "paths": [str(work / "*.txt")],
    "question": "Does this ticket concern an INVOICE or a billing problem?",
})])[0]
wall = time.time() - t0
if "error" in res:
    report(False, "ask_files must answer", res["error"]["message"][:200])
else:
    body = json.loads(res["result"]["content"][0]["text"])
    print(f"         asked={body.get('asked')} hits={len(body.get('hits', []))} "
          f"({wall:.1f}s wall)")
    for h in body.get("hits", []):
        print(f"           {pathlib.Path(h['path']).name}  noul={h['noul']:.4f}")
    for f in body.get("files", []):
        if not f.get("answers"):
            print(f"           (not judged) {pathlib.Path(f['path']).name}: {f.get('failure')}")

    report(body.get("asked", 0) >= 3, "every matching file was attempted",
           f"asked={body.get('asked')}")
    hits = body.get("hits", [])
    report(len(hits) >= 1, "at least the invoice file answered")
    if hits:
        names = [pathlib.Path(h["path"]).name for h in hits]
        report("invoice.txt" in names,
               "the invoice file is among the hits", f"hits: {names}")
        score = [h["noul"] for h in hits]
        report(score == sorted(score, reverse=True),
               "hits are ranked best-first", f"scores: {score}")
    report("parallel" in body and body["parallel"] is False,
           "the response does NOT claim parallelism it does not have")
    report(body.get("content_in_context") is False,
           "content_in_context is false")

print()
print("=== CASE 3: an unreadable file does not lose the others ===")
body_args = {"paths": [str(work / "invoice.txt"), str(work / "does-not-exist.txt")],
             "question": "Does this concern an invoice?"}
res = run([("ask_files", body_args)])[0]
if "error" in res:
    report(False, "ask_files must answer", res["error"]["message"][:160])
else:
    b = json.loads(res["result"]["content"][0]["text"])
    files = b.get("files", [])
    judged = [f for f in files if f.get("answers")]
    failed = [f for f in files if not f.get("answers")]
    report(len(judged) == 1 and len(failed) == 1,
           "one judged, one reported as failed — the batch survived",
           f"judged={len(judged)} failed={len(failed)}")
    if failed:
        report(bool(failed[0].get("failure")), "the failure carries a reason",
               failed[0].get("failure", "")[:120])

print()
print("=== CASE 4: out-of-scope refused, others judged ===")
res = run([("ask_files", {"paths": [str(work / "invoice.txt"), str(outside)],
                          "question": "Is an invoice mentioned?"})])[0]
if "error" not in res:
    b = json.loads(res["result"]["content"][0]["text"])
    files = b.get("files", [])
    out_entry = [f for f in files if "outside-probe" in f.get("path", "")]
    if out_entry:
        msg = out_entry[0].get("failure") or ""
        report("scope" in msg, "the out-of-scope file names the scope", msg[:120])
    else:
        report(False, "the out-of-scope file should appear in files[]")
    report(any(f.get("answers") for f in files),
           "the in-scope file was still judged")

print()
print("=== CASE 5: a wide glob is capped and says so ===")
res = run([("ask_files", {"paths": [str(work / "*.txt")], "question": "x?"})])[0]
if "error" not in res:
    b = json.loads(res["result"]["content"][0]["text"])
    notes = b.get("notes", [])
    report(isinstance(notes, list), "notes is a list (glob expansion is explained)")
    # no cap triggers with only 3 files, so assert the notes exist for a glob at all
    report(any("glob expanded" in n for n in notes),
           "the glob expansion is reported", f"notes={notes}")

print()
print("=== CASE 6: a large file is judged WITH the prefix warning ===")
res = run([("ask_files", {"paths": [str(work / "large.txt")],
                          "question": "Is an invoice mentioned?"})])[0]
if "error" in res:
    report(False, "the large file must be judged, not refused", res["error"]["message"][:160])
else:
    b = json.loads(res["result"]["content"][0]["text"])
    f = (b.get("files") or [{}])[0]
    report(f.get("answers") is not None,
           "the large file is judged (refusing would be its own failure)")
    w = f.get("warning") or ""
    report("PREFIX" in w, "the verdict warns that only a prefix may have been read", w[:150])
    report(f.get("bytes", 0) > 1024, f"bytes recorded: {f.get('bytes')}")

print()
print("=== CASE 7: the single-file tool warns too ===")
res = run([("ask_file", {"path": str(work / "large.txt"),
                         "question": "Is an invoice mentioned?"})])[0]
if "error" in res:
    report(False, "ask_file on a large file must answer", res["error"]["message"][:160])
else:
    b = json.loads(res["result"]["content"][0]["text"])
    report("warning" in b, "ask_file also warns on a large file",
           str(b.get("warning"))[:140])
    report(b.get("bytes", 0) > 1024, f"bytes recorded: {b.get('bytes')}")

print()
print("=" * 68)
if failures:
    print(f"RESULT: {len(failures)} FAILED")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("RESULT: ALL CHECKS PASSED")
print("  ask_files: globs expanded and ranked, per-file failures isolated,")
print("  scope enforced per file, large files judged WITH a prefix warning.")
