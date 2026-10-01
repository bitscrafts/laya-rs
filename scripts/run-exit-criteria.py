#!/usr/bin/env python3
"""Run every exit criterion in `specs/` FROM THE SPECS' OWN TEXT.

Three bugs this has had, all the "check reports nothing or reports wrong" class.
The first two were fixed before 2026-09-30; the third is fixed here:

  1. a greedy `sed` took the span from the FIRST backtick to the LAST on a line, so a
     criterion quoting several spans produced a WRONG command (e.g. "laya-store");
  2. a criterion with no backticked span was `continue`d silently, so it vanished from
     the report — indistinguishable from a criterion that passed;
  3. **the spec list was HARDCODED to `01..06`**, so when spec 07 was written its
     criteria were never run. MEASURED 2026-09-30: twelve spec files on disk, six
     evaluated. A criterion that is never run is indistinguishable from one that
     passed — the same defect as (2), one level up. The list is now DISCOVERED.

Behaviour for `01..06` is deliberately unchanged: the span logic below is the exact
code that was measured against them, so a change in the numbers is a signal, not noise.

Discovering rather than listing means a NEW spec needs no edit here — which is the
whole point: the previous design required someone to remember, and nobody did.
"""
import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path("/projects/laya-rs")
SPECS_DIR = ROOT / "specs"
CHECKBOX = re.compile(r"^\s*- \[([ x])\]")
SPAN = re.compile(r"`([^`]+)`")
# prose criteria we cannot evaluate mechanically, named so the count is honest
is_command = re.compile(r"(cargo|python3|test |^cd |grep|bash)")
# `README.md` is an index, not a spec. Everything else in specs/ IS one, including
# part files (`07-...-part1.md`) and master specs whose criteria are a pointer table.
NOT_A_SPEC = {"README.md"}


def discover() -> list[Path]:
    """Every spec file, ordered by its leading number then name.

    A numeric sort, not a lexical one: `10-...` must not sort before `9-...`.
    """
    def key(p: Path) -> tuple[int, str]:
        m = re.match(r"(\d+)", p.name)
        return (int(m.group(1)) if m else 9999, p.name)

    return sorted(
        (p for p in SPECS_DIR.glob("*.md") if p.name not in NOT_A_SPEC),
        key=key,
    )


# A spec's own Status line decides whether a failing criterion is a DEFECT or an
# EXPECTATION. A spec that is SPECIFIED-but-not-dispatched cannot have passing
# criteria -- that is what "not yet dispatched" MEANS -- so its failures are
# PENDING, reported loudly, and do not fail the gate.
#
# This is NOT a silent pass, and the distinction is the whole point: PENDING is
# printed with its count and its spec names, and it stops being PENDING the moment
# the Status line changes. Verify with the control the spec names (R15): flip a
# spec's status to COMPLETE and its criteria must start counting.
STATUS = re.compile(r"\*\*Status:?\*{0,2}:?\s*(.+?)(?:\*\*|$)", re.IGNORECASE)
DONE_WORDS = ("COMPLETE", "IMPLEMENTED", "IN PROGRESS", "DONE", "WORKING")
# How far into a spec to look for its status line. Comfortably covers the header
# (title, the metadata bullets, `## Overview`) without letting a later prose mention
# of "Status:" masquerade as the declaration.
HEADER_LINES = 60


def status_of(path: Path) -> str:
    """The spec's declared status, or 'UNKNOWN' when it carries none.

    Handles both house forms, because a status line that fails to parse fails in the
    UNSAFE direction -- see below:
      * `**Status:** SPECIFIED -- not yet dispatched`
      * `**Status: IMPLEMENTED, AND DELIBERATELY UNWIRED.** <prose>`

    UNKNOWN counts as DONE -- i.e. its failures are DEFECTS. A spec whose status
    could not be read has not proved it is incomplete, so it gets no benefit of the
    doubt. Getting this backwards would let an unparseable status silently convert
    real failures into pending work, which is the defect this whole script fixes.

    Scanned over the first HEADER_LINES only, and NOT stopped at the first `## `,
    because the house style puts `## Overview` BEFORE the status line (measured:
    specs 01-06 and 07 alike). Stopping at the first section heading made every
    spec read UNKNOWN -- a bug that fails SAFE here only by luck.
    """
    for line in path.read_text().splitlines()[:HEADER_LINES]:
        m = STATUS.search(line)
        if m:
            return m.group(1).strip().rstrip(".").upper()
    return "UNKNOWN"


def main() -> int:
    ap = argparse.ArgumentParser(description="Run every spec's exit criteria from its own text.")
    ap.add_argument(
        "--check-pending",
        action="store_true",
        help="ALSO run the criteria of specs declared SPECIFIED (slower; for auditing "
             "that pending work really does fail)",
    )
    ap.add_argument(
        "--list-specs",
        action="store_true",
        help="print each spec's status line and exit WITHOUT running any criterion. "
             "This is the mode a spec's own 'is my spec discovered?' criterion must use "
             "-- see the re-entrancy note below.",
    )
    args = ap.parse_args()

    # RE-ENTRANCY. A criterion of the form
    #   python3 scripts/run-exit-criteria.py 2>&1 | grep -q '<spec name>'
    # is evaluated BY this script, so running it starts another full evaluation,
    # which runs the criterion again, which starts another... Measured 2026-10-01:
    # four spec parts each carried such a criterion and the run left **18 nested
    # runner processes**, each doing whole-workspace cargo runs. It did not hang --
    # it multiplied, which is worse, because it looks like slow progress.
    #
    # Two fixes, both here:
    #   1. `--list-specs` gives those criteria a mode that touches no criterion, so
    #      it cannot recurse by construction. A spec must use it (spec-author says so).
    #      It is checked BEFORE the guard and is deliberately EXEMPT: it is a leaf
    #      that runs nothing, so a child doing it is not a recursion, it is the point.
    #   2. The guard makes a FULL nested evaluation fail loudly instead of recursing,
    #      so a spec that still carries the old form is corrected rather than allowed
    #      to multiply.
    if args.list_specs:
        # Cheap and non-recursive: status lines only, no criterion is executed.
        for path in discover():
            criteria = [ln for ln in path.read_text().splitlines() if CHECKBOX.match(ln)]
            status = status_of(path)
            mark = "--" if not criteria else "  "
            print(f"{mark} {path.name}   [{status}]   ({len(criteria)} criteria)")
        return 0

    if os.environ.get("LAYA_EXIT_CRITERIA_RUNNING"):
        print(
            "REFUSING TO RE-ENTER: run-exit-criteria.py is already running (this is a "
            "criterion invoking the runner that is evaluating it -- that multiplies "
            "processes instead of measuring anything). A criterion that asks 'is my "
            "spec discovered?' must use `--list-specs`, which runs no criteria.",
            file=sys.stderr,
        )
        return 2

    os.environ["LAYA_EXIT_CRITERIA_RUNNING"] = "1"

    npass = nfail = nprose = 0
    pending = 0
    specs: list[Path] = []
    empty: list[str] = []
    pending_specs: list[str] = []

    for path in discover():
        criteria = [ln for ln in path.read_text().splitlines() if CHECKBOX.match(ln)]
        if not criteria:
            # Reported, never skipped. A spec with zero criteria is either a master
            # spec whose criteria live in its parts (spec 07, deliberately) or a
            # defect -- either way it must be visible rather than absent.
            empty.append(path.name)
            continue
        specs.append(path)
        status = status_of(path)
        is_pending = "SPECIFIED" in status and not any(w in status for w in DONE_WORDS)
        print(f"=== {path.name}   [{status}]")

        # A spec declared SPECIFIED has not been dispatched, so its criteria cannot
        # pass yet and RUNNING them is wasted work -- measured 2026-09-30: three
        # whole-workspace test runs per part, minutes of the gate spent proving what
        # the status line already says. They are therefore COUNTED but not executed,
        # and printed PENDING: never silently.
        #
        # The audit path is explicit: `--check-pending` really runs them, so
        # "does the pending work actually fail?" stays answerable in one command
        # instead of being taken on faith.
        if is_pending and not args.check_pending:
            for line in criteria:
                first = SPAN.findall(line)
                if first and is_command.search(first[0]):
                    pending += 1
            pending_specs.append(f"{path.name} ({len(criteria)} criteria)")
            print(f"  PENDING x{len(criteria)} (not run: spec not dispatched)")
            continue

        for line in criteria:
            # ONLY THE FIRST SPAN IS THE COMMAND. The original took every
            # backticked span on the line ("a line may quote more than one"), which
            # meant a criterion whose PROSE also backticks a fragment executed that
            # fragment as a command. Measured 2026-09-30: the line
            #   `test -d crates/laya-graph && ! grep -rqE '...' crates/laya-graph/src/`
            #   ... the guard exists because `! grep -r` on a missing path inverts to
            #   success ...
            # ran `test -d` on its own -- and `test -d` is `test STRING`, i.e.
            # non-empty, i.e. TRUE -- then printed PASS. A check that always passes
            # is worse than no check: it certifies the defect it was written to catch.
            # `spec-author` states the rule ("the first backticked span is the
            # command; put it first"); this script was not following it.
            first = SPAN.findall(line)
            spans = [first[0]] if first and is_command.search(first[0]) else []
            if not spans:
                nprose += 1
                text = CHECKBOX.sub("", line).strip()
                print(f"  PROSE {text[:92]}")
                continue
            for cmd in spans:
                r = subprocess.run(
                    cmd, shell=True, cwd=ROOT, capture_output=True, text=True
                )
                if r.returncode == 0:
                    npass += 1
                    print(f"  PASS  {cmd[:88]}")
                else:
                    nfail += 1
                    tail = (r.stderr or r.stdout).strip().splitlines()[-2:]
                    print(f"  FAIL  {cmd[:88]}")
                    for t in tail:
                        print(f"        {t[:100]}")

    print(f"\nSPECS EVALUATED: {len(specs)} (discovered, not listed)")
    if empty:
        # Named so the reader can tell a deliberate pointer-doc from a spec that
        # lost its criteria. Silence here is what hid spec 07 for a whole session.
        print(f"SPECS WITH NO CRITERIA (reported, not skipped): {len(empty)}")
        for name in empty:
            print(f"  -- {name}")
    if pending:
        # Loud, named, and counted -- never quiet. The number is the reminder that
        # this work exists and its criteria are not yet evidence of anything.
        print(f"PENDING (declared SPECIFIED, not yet dispatched): {pending} criteria")
        for name in pending_specs:
            print(f"  ?? {name}")
    print(f"EXIT CRITERIA: pass={npass} fail={nfail} prose={nprose} pending={pending}")
    return 1 if nfail else 0


if __name__ == "__main__":
    sys.exit(main())
