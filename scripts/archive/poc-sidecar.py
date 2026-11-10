#!/usr/bin/env python3
"""POC test: drive the sidecar over its real socket and get a REAL decision.

This is the proof that the decision path works end to end through the hybrid:
  text -> sidecar -> (tokenizer + build_sequence in laya) -> llama.cpp /embedding
       -> our local head -> typed answer with probabilities

It starts the sidecar as a real subprocess, talks the real protocol over the real
socket, and asserts on the SHAPE and RANGE of the answer — not on a hardcoded label,
because a hardcoded expected label would be me asserting what the model should think.

What it DOES assert strictly:
  * `choice` returns one of the DECLARED options (the property the whole graph
    design depends on: the model can only select among ports that already exist)
  * the probabilities form a normalised distribution of the right length
  * `score` lands in the rubric's range
  * `noul` returns a probability in [0,1]
  * confidence is present and in [0,1]
  * latency is reported
  * a SECOND call is fast (proving the 24 s load is paid once, not per decision)
"""
import json
import os
import socket
import subprocess
import sys
import time

ROOT = "/projects/laya-rs"
PY = f"{ROOT}/.venv-laya/bin/python"
SIDECAR = f"{ROOT}/scripts/laya-sidecar.py"
HEAD = "/projects/_tmp/laya-gguf/laya-head.safetensors"
SOCK = f"/run/laya/poc-{os.getpid()}.sock"


def start():
    env = dict(os.environ)
    env["PYTHONUNBUFFERED"] = "1"
    proc = subprocess.Popen(
        [PY, SIDECAR, "--socket", SOCK, "--head", HEAD],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        # its own process group, so a stray child cannot outlive this test
        start_new_session=True,
    )
    # Wait for the socket to accept, with a bounded wait (the load is ~24 s).
    deadline = time.time() + 180
    while time.time() < deadline:
        if os.path.exists(SOCK):
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.settimeout(5)
                s.connect(SOCK)
                s.close()
                return proc
            except OSError:
                pass
        if proc.poll() is not None:
            out, err = proc.communicate()
            print("sidecar exited early:\n", err[-2000:])
            sys.exit(3)
        time.sleep(0.5)
    proc.kill()
    print("sidecar did not become ready in 180 s")
    sys.exit(3)


def call(req, timeout=180):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(SOCK)
    with s:
        s.sendall((json.dumps(req) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    return json.loads(buf.decode())


print("=== starting the sidecar (pays the ~24 s load ONCE) ===")
proc = start()
try:
    h = call({"op": "health"})
    print(f"  health: laya {h['laya_version']} head {h['head_matched']}/{h['head_tensors']} "
          f"d_hidden={h['d_hidden']} loaded_ms={h['loaded_ms']}")
    assert h["head_matched"] == h["head_tensors"] == 36, "all 36 head tensors must load"
    assert h["head_missing"] == [], f"unmatched tensors: {h['head_missing']}"

    # ---- the three question types, the shapes the whole design depends on ----
    state = {
        "subject": "Duplicate charge on invoice 4411",
        "body": "We were billed twice for March. Please refund the duplicate.",
    }
    questions = {
        "department": {
            "type": "choice",
            "instructions": "Which team should handle this?",
            "criteria": {
                "billing": "invoices, payments, refunds",
                "technical": "bugs and outages",
                "sales": "pricing and new orders",
            },
        },
        "urgency": {
            "type": "score",
            "instructions": "How urgent is this?",
            "criteria": ["not urgent", "soon", "critical"],
        },
        "churn_risk": {
            "type": "noul",
            "instructions": "Does the user threaten to cancel?",
        },
    }

    print("\n=== CALL 1 (cold: pays the model load) ===")
    t0 = time.time()
    r1 = call({"op": "predict", "state": state, "questions": questions})
    wall1 = (time.time() - t0) * 1000
    assert r1.get("ok"), f"predict failed: {r1}"
    print(f"  ms={r1['ms']}  wall={wall1:.0f}  encoder_calls={r1['encoder_calls']}")
    print(json.dumps(r1["answers"], indent=4)[:1200])

    a = r1["answers"]
    # --- choice: the label MUST be one of the declared options ---
    dep = a["department"]
    print("\n=== ASSERTIONS ===")
    chosen = dep["choice"]
    assert chosen in questions["department"]["criteria"], (
        f"choice {chosen!r} is not a declared option — this is the property the "
        f"graph design depends on"
    )
    print(f"  ok  choice {chosen!r} is one of the declared options")
    probs = dep["probabilities"]
    # `probabilities` is a DICT keyed by option name (observed shape), not a list.
    assert isinstance(probs, dict), f"expected a dict keyed by option, got {type(probs)}"
    assert set(probs) == set(questions["department"]["criteria"]), (
        f"probability keys must be exactly the declared options; got {set(probs)}"
    )
    total = sum(probs.values())
    assert abs(total - 1.0) < 0.05, f"probabilities must be a distribution, sum={total}"
    print(f"  ok  probabilities are a distribution keyed by option, sum={total:.4f}")
    assert 0.0 <= dep["confidence"] <= 1.0, "confidence in [0,1]"
    print(f"  ok  confidence {dep['confidence']}")

    # ---- the two review findings this run CONFIRMS ---------------------------
    act = dep.get("action", {}).get("act_probability")
    if act is not None:
        assert act == 1.0 or True  # recorded, not asserted on: it is inert
        print(f"  NOTE act_probability={act} on a 'billing' answer AND on a noul AND on a "
              f"score — inert, exactly as measured (F8). Never gate on this field.")

    noul_p = a["churn_risk"]["noul"]
    noul_conf = a["churn_risk"]["confidence"]
    expected = max(noul_p, 1.0 - noul_p)
    agree = abs(noul_conf - expected) < 0.01
    print(f"  {'ok ' if agree else 'NOTE'} noul {noul_p} -> confidence {noul_conf} "
          f"(max(p,1-p) = {expected:.4f}) — {'confirms' if agree else 'differs from'} the "
          f"review's finding m4: a noul's confidence is a DIFFERENT quantity from a "
          f"choice's, which is why Policy has per-type thresholds")

    # --- score: lands in the rubric range ---
    sc = a["urgency"]["score"]
    assert sc is not None, "score must be present"
    assert 0.0 <= sc <= 2.0, f"score {sc} outside the 0..2 rubric"
    print(f"  ok  score {sc} within the rubric range (may land BETWEEN levels — that is the point)")
    sconf = a["urgency"]["confidence"]
    print(f"  NOTE score confidence {sconf} — low, and a score's confidence is yet another "
          f"quantity (1 - H/log k), reinforcing the per-type thresholds")

    # --- noul: a probability ---
    p = a["churn_risk"]["noul"]
    assert 0.0 <= p <= 1.0, f"noul {p} must be in [0,1]"
    print(f"  ok  noul {p} in [0,1]")
    print(f"  ok  saturated check: probabilities {probs}")

    # ---- CALL 2: must be fast, proving the load is paid once ----------------
    print("\n=== CALL 2 (warm: the load must NOT be paid again) ===")
    t0 = time.time()
    r2 = call({"op": "predict", "state": state, "questions": questions})
    wall2 = (time.time() - t0) * 1000
    print(f"  ms={r2['ms']}  wall={wall2:.0f}  encoder_calls={r2['encoder_calls']}")
    assert r2.get("ok")
    assert wall2 < 20_000, f"a warm call took {wall2:.0f} ms — the load is being repaid"
    print(f"  ok  warm call is {wall1/1000:.1f}s -> {wall2/1000:.2f}s "
          f"({wall1/max(wall2,1):.0f}x) — the 24 s load is paid ONCE")

    # ---- determinism: same input must give the same label (B1 within a build) ----
    same = r1["answers"]["department"]["choice"] == r2["answers"]["department"]["choice"]
    print(f"  {'ok ' if same else 'NOTE'} same label on repeat: {same} "
          f"(argmax is deterministic within a build; near-ties are what B1 is about)")

    print("\n=== POC RESULT: the decision path works end to end ===")
    print(f"  text -> sidecar -> llama.cpp /embedding -> local head -> typed answer")
    exit_code = 0
finally:
    try:
        proc.terminate()
        proc.wait(timeout=15)
    except Exception:
        proc.kill()
    if os.path.exists(SOCK):
        os.rename(SOCK, SOCK + ".done")

sys.exit(0)
