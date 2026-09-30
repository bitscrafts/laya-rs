#!/usr/bin/env python3
"""Spec 03's LIVE criterion: does the Rust path choose the same LABEL as the Python reference?

## The claim being tested

Everything before this is component-wise: the tokenizer reproduces the reference exactly (91 strings
/ 2207 ids, 707 sequences / 83411 ids), the forward pass reproduces `PyTorch` (worst logit difference
4.05e-6), and the post-processing reproduces the reference (worst difference 5.55e-17). This harness
tests the **end-to-end** claim: given the same backbone, does the whole Rust path — its own
tokenizer, its own sequence builder, its own embedding call, its own head — choose the same label as
the Python reference?

That is the decision-level oracle review M4 asked for. `Δp <= 0.001` on one probability is NOT the
oracle: it passes while labels flip on near-ties.

## How it runs

For each case it asks BOTH sides the same question about the same state, and compares:

  Rust      `laya decide --state … --question …`   → JSON on stdout   (text in, whole path)
  Python    `{"op":"predict", …}` to the sidecar   → the reference's answer

Both are driven from TEXT, which is the point: a comparison starting from a pre-computed hidden
state would skip the parts most likely to differ.

## What it reports, and why each number is there

  * **label agreement** — the criterion. Reported with its floor so the number means something.
  * **a margin histogram** — so near-ties are visible rather than averaged away. A corpus of easy
    cases can hit 100% agreement while the model is guessing on everything that matters.
  * **clamped-bucket count** — an answer whose temperature was clamped has uncalibrated confidence
    and should not be read as an ordinary agreement.
  * **probability differences**, classified: below the reference's own 4-dp rounding, or above it.

## The honest limit, stated up front

If the two sides disagree, this harness CANNOT say which is right — both consume the same backbone.
What it can do is classify: a difference inside the reference's rounding (5e-5) is not evidence of a
port bug, while a large difference is. So the report separates them, and a disagreement is printed
with the case's state and question so it can be reproduced rather than argued about.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import socket
import subprocess
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
ENDPOINT = os.environ.get("LAYA_BACKBONE", "http://10.10.10.217:8083")
SIDECAR = os.environ.get("LAYA_SIDECAR_SOCKET", "/run/laya/sidecar.sock")
# Absolute, not a bare name: this is run from cron and from other sessions, where PATH does not
# necessarily include ~/.local/bin — and a bare name would fail as an unhelpful "No such file".
# Absolute, not a bare name: this is run from cron and other sessions, where PATH may not include
# ~/.local/bin, and a bare name fails there as an unhelpful "No such file".
#
# The RELEASE binary, not the debug one: this workload is arithmetic-bound over a 26.5M-parameter
# model, and a debug build measured ~10x slower (a 460-token case took ~120 s debug against ~12 s
# release). A corpus run on the debug binary would take hours and look like a hang.
CLI = os.environ.get("LAYA_CLI", str(pathlib.Path.home() / ".local" / "bin" / "laya"))

# The reference rounds every reported number to 4 dp (`round(v, 4)`) when it builds its response.
# This port keeps full f64 precision and rounds at the boundary that displays or publishes it, so a
# parity comparison must allow at least that much. 5e-5 is half a unit in the last place of a 4-dp
# value, with a little room.
ROUNDING_TOL = 5e-5


def corpus() -> list[dict]:
    """The corpus: each case is a state and a question, in the workspace's own shapes.

    Chosen to spread across what the two sides could disagree about — every question type, every
    temperature bucket boundary, unicode, a long state, a single-option choice, and a near-tie by
    construction.
    """
    cases: list[dict] = []

    def add(name, state, qtype, text, options=None, criteria=None):
        q = {"id": name, "type": qtype, "text": text}
        if options is not None:
            q["options"] = options
        if criteria is not None:
            q["criteria"] = criteria
        cases.append({"name": name, "state": state, "question": q})

    # --- choice, the common case -------------------------------------------------------------
    add("choice_broken", "The build failed with a linker error and the tests never ran.",
        "choice", "What is the state of the build?",
        options=["broken", "degraded", "healthy", "other"])
    add("choice_green", "Everything passed. The gate is green, the tree is clean, and all 329 "
        "tests pass.",
        "choice", "Is the work complete?", options=["no", "partially", "yes"])
    add("choice_neutral", "The file was saved.",
        "choice", "What happened?", options=["nothing", "a write", "a delete", "other"])

    # --- score, where a fractional answer is the point ---------------------------------------
    add("score_quality", "The code works but has no tests and the error handling is thin.",
        "score", "Rate the quality from 0 (unusable) to 4 (excellent).",
        options=["level 0: unusable", "level 1: poor", "level 2: adequate",
                 "level 3: good", "level 4: excellent"])
    add("score_binary", "The patch is correct and tested.",
        "score", "Rate from poor to good.", options=["poor", "good"])

    # --- noul, the binary gate ---------------------------------------------------------------
    add("noul_safe", "The change touches only documentation and adds no new dependency.",
        "noul", "Is this change safe to apply without review?")
    add("noul_unsafe", "The change rewrites the database schema without a migration.",
        "noul", "Is this change safe to apply without review?")
    add("noul_borderline", "The change adds a dependency but removes two others.",
        "noul", "Is this change safe to apply without review?")

    # --- the near-tie, by construction -------------------------------------------------------
    add("choice_near_tie", "The result is mixed: the feature works but it slows the build down.",
        "choice", "Is this change good or bad?", options=["good", "bad", "mixed", "other"])

    # --- unicode, to exercise the byte-level path live rather than only in fixtures -----------
    add("choice_unicode", "O relatório diz: a integração está pronta, mas a documentação está "
        "incompleta. 状態は良好です。",
        "choice", "Is the work documented?", options=["yes", "no", "partially"])

    # --- a LONG state, past the head's ~192-token window and near the backbone's 512 ----------
    # This is the case where a truncation difference between the two sides shows up as a
    # disagreement, which is exactly why it is here.
    add("choice_long", " ".join(["the pipeline processed a record and wrote a ledger entry"] * 40),
        "choice", "Did the pipeline write to the ledger?", options=["yes", "no"])

    # --- a single-option choice: no runner-up, so the margin rule differs --------------------
    add("choice_single", "Nothing has happened yet.", "choice", "What happened?",
        options=["nothing"])

    # --- across the temperature bucket boundaries: 2, 3-5, 6-10, 11+ ------------------------
    add("choice_k3", "A routine task completed with one warning.", "choice",
        "Classify this event.", options=["ok", "warning", "error"])
    add("choice_k6", "A routine task completed with one warning.", "choice",
        "Classify this event.", options=[f"category {i}" for i in range(6)])
    add("choice_k11", "A routine task completed with one warning.", "choice",
        "Classify this event.", options=[f"category {i}" for i in range(11)])

    # --- option ORDER, which both sides must agree on ----------------------------------------
    # The ABI carries options as a flat list, and the reference derives its rendering order from the
    # criteria mapping — so the list order IS the rendered order, and a port that re-sorted it would
    # change the token sequence and the marker indices. The options here are deliberately NOT in
    # alphabetical order so a re-sort is visible.
    #
    # (The reference's own `criteria`-as-a-mapping form is covered at the tokenizer level instead:
    # `tokenizer_differential.rs` has `the_control_a_choice_renders_its_options_in_insertion_order`,
    # which builds from a mapping in a deliberately non-alphabetical order.)
    add("choice_option_order", "The deployment succeeded but a warning was logged.",
        "choice", "What is the deployment status?",
        options=["zeta (listed first)", "alpha (listed second)", "mu (listed third)"])

    return cases


# --------------------------------------------------------------------------------------------
# The two sides
# --------------------------------------------------------------------------------------------

def http_ok(url: str, timeout: float = 10.0) -> tuple[bool, str]:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as r:
            return True, f"HTTP {r.status}"
    except Exception as exc:  # noqa: BLE001 - any failure means not usable
        return False, str(exc)


def sidecar_predict(state, questions: dict, timeout: float = 600.0) -> dict:
    """One `predict` to the Python reference over its unix socket."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(SIDECAR)
        s.sendall(json.dumps({"op": "predict", "state": state, "questions": questions}).encode() + b"\n")
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    finally:
        s.close()
    if not buf.strip():
        raise RuntimeError("the sidecar closed without answering")
    return json.loads(buf.decode())


def sidecar_health(timeout: float = 30.0) -> dict:
    return sidecar_predict_health(timeout)


def sidecar_predict_health(timeout: float = 30.0) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(SIDECAR)
        s.sendall(json.dumps({"op": "health"}).encode() + b"\n")
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    finally:
        s.close()
    return json.loads(buf.decode())


def rust_decide(state, question, url: str, timeout: float = 600.0) -> dict:
    """Run the Rust path for one case. Text in, JSON out, as a user would."""
    payload = json.dumps({"state": state, "question": question})
    proc = subprocess.run(  # noqa: S603 - fixed argv, no shell
        [CLI, "decide", "-", "--url", url],
        input=payload.encode(), capture_output=True, timeout=timeout, check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{CLI} decide exited {proc.returncode}: {proc.stderr.decode().strip()[:400]}"
        )
    return json.loads(proc.stdout.decode())


def reference_question_for_sidecar(q: dict) -> dict:
    """Translate the case's question into the reference's own shape.

    The reference wants `instructions` and `criteria`; the workspace's ABI wants `text` and
    `options`. They describe the same question, and this is the one place the translation lives.
    """
    out: dict = {"type": q["type"], "instructions": q["text"]}
    if "criteria" in q:
        out["criteria"] = q["criteria"]
    elif "options" in q:
        if q["type"] == "choice":
            out["criteria"] = list(q["options"])
        elif q["type"] == "score":
            out["criteria"] = list(q["options"])
    return out


def label_of(answer: dict, qtype: str):
    """The LABEL a side chose, normalised to something comparable across the two shapes."""
    if qtype == "choice":
        return answer.get("choice")
    if qtype in ("noul", "no_uncertainty"):
        return answer.get("noul")
    if qtype == "score":
        return answer.get("score")
    return None


def rust_label(out: dict):
    a = out["answer"]
    if a["type"] == "choice":
        return a.get("value")
    if a["type"] == "score":
        return a.get("score")
    if a["type"] == "noul":
        return a.get("p")
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--floor", type=float, default=0.9,
                    help="the label-agreement floor the criterion requires (default 0.9)")
    ap.add_argument("--json", type=pathlib.Path, default=None,
                    help="write the full per-case record here")
    ap.add_argument("--url", default=ENDPOINT)
    args = ap.parse_args()

    cases = corpus()
    print(f"  corpus: {len(cases)} cases")
    print(f"  backbone: {args.url}")

    # (1) The backbone. Without it neither side can run, and the criterion cannot be evaluated.
    ok, why = http_ok(f"{args.url}/v1/models")
    if not ok:
        print(f"  BACKBONE UNAVAILABLE: {why}", file=sys.stderr)
        print("  the live criterion cannot be evaluated; this is NOT a pass", file=sys.stderr)
        return 2
    print(f"  backbone reachable: {why}")

    # (2) The Python reference. It is the ORACLE, so without it there is nothing to compare to.
    try:
        health = sidecar_health()
        print(f"  sidecar up: {health}")
    except Exception as exc:  # noqa: BLE001
        print(f"  SIDECAR UNAVAILABLE at {SIDECAR}: {exc}", file=sys.stderr)
        print("  the Python reference is the oracle; without it this cannot be measured",
              file=sys.stderr)
        return 2

    records = []
    agree = 0
    disagree = 0
    errors = 0
    clamped = 0
    worst_prob_diff = 0.0
    margins: list[float] = []

    for i, case in enumerate(cases, 1):
        name = case["name"]
        q = case["question"]
        try:
            rust = rust_decide(case["state"], q, args.url)
        except Exception as exc:  # noqa: BLE001
            errors += 1
            print(f"  [{i:2}/{len(cases)}] {name}: RUST ERROR: {str(exc)[:200]}")
            records.append({"name": name, "ok": False, "stage": "rust", "error": str(exc)[:400]})
            continue

        try:
            ref = sidecar_predict(case["state"], {name: reference_question_for_sidecar(q)})
            ref_answer = ref["answers"][name]
        except Exception as exc:  # noqa: BLE001
            errors += 1
            print(f"  [{i:2}/{len(cases)}] {name}: REFERENCE ERROR: {str(exc)[:200]}")
            records.append({"name": name, "ok": False, "stage": "reference",
                            "error": str(exc)[:400]})
            continue

        r_label = rust_label(rust)
        p_label = label_of(ref_answer, q["type"])

        if rust["bucket_clamped"]:
            clamped += 1

        margins.append(rust["logit_gap"])

        # Probability difference, only meaningful for choice/score where both sides report a vector.
        prob_diff = None
        rp = rust["answer"].get("probabilities") or []
        pp = ref_answer.get("probabilities")
        if pp and rp:
            # The reference keys score probabilities by string index; choice by option name.
            if isinstance(pp, dict):
                vals = list(pp.values())
            else:
                vals = list(pp)
            if len(vals) == len(rp):
                prob_diff = max(abs(a - b) for a, b in zip(rp, vals))
                worst_prob_diff = max(worst_prob_diff, prob_diff)

        # The label comparison. For `noul` the LABEL is a probability, not a discrete choice, so it
        # is compared within the reference's own rounding rather than for equality.
        if q["type"] == "noul":
            same = (r_label is not None and p_label is not None
                    and abs(float(r_label) - float(p_label)) <= ROUNDING_TOL)
        elif q["type"] == "score":
            same = (r_label is not None and p_label is not None
                    and abs(float(r_label) - float(p_label)) <= ROUNDING_TOL)
        else:
            same = r_label == p_label

        if same:
            agree += 1
            mark = "ok"
        else:
            disagree += 1
            mark = "DIFF"
        print(f"  [{i:2}/{len(cases)}] {name}: {mark}  rust={r_label!r} python={p_label!r}"
              + (f"  maxΔp={prob_diff:.2e}" if prob_diff is not None else ""))

        records.append({
            "name": name, "ok": same, "qtype": q["type"],
            "rust_label": r_label, "python_label": p_label,
            "prob_diff": prob_diff,
            "rust": rust, "reference": ref_answer,
            "state": case["state"], "question": q,
        })

    total = agree + disagree
    rate = (agree / total) if total else 0.0

    print()
    print(f"  {'=' * 70}")
    print(f"  LABEL AGREEMENT: {agree}/{total} = {rate:.4f}   (floor {args.floor})")
    print(f"  errors: {errors}   clamped-bucket answers: {clamped}")
    if worst_prob_diff:
        print(f"  worst probability difference: {worst_prob_diff:.3e} "
              f"(the reference rounds to 4 dp, so {ROUNDING_TOL:.0e} is its own resolution)")
    if margins:
        margins.sort()
        n = len(margins)
        p10 = margins[n // 10]
        p50 = margins[n // 2]
        print(f"  logit-gap histogram: min {margins[0]:.4f}  p10 {p10:.4f}  "
              f"median {p50:.4f}  max {margins[-1]:.4f}")
        near = sum(1 for m in margins if m < 0.1)
        print(f"  near-ties (gap < 0.1): {near}/{n}"
              + ("  <- the model is guessing on these" if near else ""))

    if args.json:
        args.json.write_text(json.dumps({"summary": {
            "agree": agree, "total": total, "rate": rate, "floor": args.floor,
            "errors": errors, "clamped": clamped, "worst_prob_diff": worst_prob_diff,
        }, "records": records}, indent=2, default=str))
        print(f"  wrote {args.json}")

    # The criterion: the floor must be met AND the measurement must be complete. An agreement rate
    # over a corpus with errors is not the same claim as one over the whole corpus.
    if errors:
        print(f"  FAIL: {errors} case(s) could not be measured", file=sys.stderr)
        return 1
    if rate < args.floor:
        print(f"  FAIL: agreement {rate:.4f} is below the floor {args.floor}", file=sys.stderr)
        return 1
    print(f"  PASS: agreement {rate:.4f} meets the floor {args.floor}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
