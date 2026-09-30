#!/usr/bin/env python3
"""Run every exit criterion in specs/01..06 FROM ITS OWN TEXT.

Two bugs this had in its first version, both the "check reports nothing or reports wrong"
class, fixed here:
  1. a greedy `sed` took the span from the FIRST backtick to the LAST on a line, so a
     criterion quoting several spans produced a WRONG command (e.g. "laya-store");
  2. a criterion with no backticked span was `continue`d silently, so it vanished from the
     report — indistinguishable from a criterion that passed.

Now: every backticked span on a criterion line is evaluated (a line may quote more than one),
and a criterion with no span is REPORTED as prose rather than skipped.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path("/projects/laya-rs")
SPECS = ["01-decision-payload.md", "02-route-payload.md", "03-rust-head.md",
         "04-mcp-server-and-ledger.md", "05-laya-must-be-running.md",
         "06-priority-queue.md"]
CHECKBOX = re.compile(r"^\s*- \[([ x])\]")
SPAN = re.compile(r"`([^`]+)`")
# prose criteria we cannot evaluate mechanically, named so the count is honest
is_command = re.compile(r"(cargo|python3|test |^cd |grep|bash)")


def main() -> int:
    npass = nfail = nprose = 0
    for spec in SPECS:
        print(f"=== {spec}")
        for line in (ROOT / "specs" / spec).read_text().splitlines():
            if not CHECKBOX.match(line):
                continue
            spans = [s for s in SPAN.findall(line) if is_command.search(s)]
            if not spans:
                nprose += 1
                text = CHECKBOX.sub("", line).strip()
                print(f"  PROSE {text[:92]}")
                continue
            for cmd in spans:
                r = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True, text=True)
                if r.returncode == 0:
                    npass += 1
                    print(f"  PASS  {cmd[:88]}")
                else:
                    nfail += 1
                    tail = (r.stderr or r.stdout).strip().splitlines()[-2:]
                    print(f"  FAIL  {cmd[:88]}")
                    for t in tail:
                        print(f"        {t[:100]}")
    print(f"\nEXIT CRITERIA: pass={npass} fail={nfail} prose={nprose}")
    return 1 if nfail else 0


if __name__ == "__main__":
    sys.exit(main())
