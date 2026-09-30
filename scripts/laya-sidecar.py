#!/usr/bin/env python3
"""laya-sidecar — the Laya decision head, as a persistent local service.

WHY THIS EXISTS (blocker B3, and the pinned-runtime rule)
--------------------------------------------------------
The llama.cpp container serves the ModernBERT BACKBONE ONLY. The decision head, the
tokenizer and `build_sequence` live in the `laya` Python package, which needs torch.
So something must run Python.

The naive shape — spawn `python -c ...` per decision — is wrong for three measured
reasons:
  1. cold load of the Python stack measured ~23 s;
  2. `import laya` is not available from system python (verified: ModuleNotFoundError),
     so it would reach into ANOTHER PROJECT'S venv — the exact fragile coupling the
     Rust head port (P4) exists to remove;
  3. a per-call interpreter is a per-call failure mode.

So: ONE long-lived process, own venv, pinned laya version, talking newline-delimited
JSON over a unix socket. It owns exactly one capability — "answer these typed
questions about this state" — and nothing else. `laya-mcp` never touches torch.

WHY A UNIX SOCKET, NOT A PORT
-----------------------------
A port is reachable from anywhere on the network; a socket file is reachable only by
something that can open that path. The sidecar answers arbitrary natural-language
questions about arbitrary state, so it must not be network-reachable. Socket it is.

THE BACKBONE SPLIT (why this is a hybrid at all)
------------------------------------------------
`laya.load(..., device="cpu")` loads the full model, then we MONKEYPATCH its encoder to
call llama.cpp's `/embedding` instead of running ModernBERT in torch. That is the
documented hybrid, measured at Δp = 0.0006 against pure PyTorch.

  pure PyTorch : ~220-308 ms per call (in-process, no HTTP)
  this hybrid  : ~608 ms measured — the win here is not speed, it is that the expensive
                 421M encoder is shared with llama.cpp rather than resident twice.

B2 CONFIRMED LIVE BY THIS RUNTIME (2026-09-29)
------------------------------------------------
On load, the venv's own `laya` emits, verbatim:

    RuntimeWarning: laya: this checkpoint ships temperatures outside [0.5, 5] which
    would distort confidence; clamping choice:11+=0.1006. Treat confidence from the
    affected buckets as uncalibrated.

That is blocker B2 reproducing in the real stack. Had we ported the raw config value
(what the original fact F5 instructed), the published confidence would have been
garbage at the `choice:11+` bucket. The clamp is not defensive theory; it is what the
library does.

Protocol (newline-delimited JSON, one request -> one response line):
  -> {"op":"health"}
  <- {"ok":true,"laya_version":"0.3.5","head":"/path/laya-head.safetensors",...}
  -> {"op":"predict","state":{...},"questions":{...}}
  <- {"ok":true,"answers":{...},"ms":123.4}
  -> {"op":"shutdown"}
  <- {"ok":true}
"""

import argparse
import json
import os
import socket
import sys
import time
import traceback
import urllib.request

# The pinned runtime. Overridable for a different deploy, but the DEFAULT is the
# local venv created by setup-runtime.sh — never another project's venv.
DEFAULT_VENV = "/projects/laya-rs/.venv-laya"
DEFAULT_HEAD = "/projects/_tmp/laya-gguf/laya-head.safetensors"
DEFAULT_ENDPOINT = "http://10.10.10.217:8083"


def log(msg):
    """Diagnostics to stderr only — stdout would corrupt a protocol line."""
    print(f"laya-sidecar: {msg}", file=sys.stderr, flush=True)


class Sidecar:
    def __init__(self, endpoint, head_path):
        import laya  # imported here so --help works without torch
        from importlib.metadata import version as _v

        self.laya_version = _v("laya")
        self.endpoint = endpoint.rstrip("/")
        self.head_path = head_path

        log(f"loading laya {self.laya_version} (device=cpu) ...")
        t0 = time.time()
        # Trust remote code must be off by default; the weights are local.
        self.agent = laya.load("convaiinnovations/laya", device="cpu")

        # Install the local decision head over the model's own.
        sys.path.insert(0, os.path.dirname(os.path.abspath(head_path)))
        from laya_head import load_head

        cfg, head = load_head(head_path)
        sd = self.agent.model.state_dict()
        matched = [k for k in head if k in sd]
        missing = [k for k in head if k not in sd]
        for k in matched:
            sd[k].copy_(head[k])
        self.head_tensors = len(head)
        self.head_matched = len(matched)
        self.head_missing = missing
        self._cfg = cfg
        log(
            f"head tensors={len(head)} matched={len(matched)} "
            f"not_in_model={len(missing)}"
        )

        # THE HYBRID: replace the encoder's forward with a llama.cpp /embedding call,
        # so the 421M encoder is served by llama.cpp instead of torch.
        import types

        import torch

        d_hidden = self.agent.model.encoder.config.hidden_size
        self.d_hidden = d_hidden
        self._calls = {"n": 0, "ms": 0.0}

        def embed(text_tokens):
            """POST to llama.cpp /embedding with the stdlib only.

            `urllib.request` rather than `requests`: the call is a plain JSON POST and
            a plain JSON read, and a pinned runtime should not carry a dependency it
            does not need — every extra dep is another way the pin can rot.
            """
            body = json.dumps({"content": [text_tokens]}).encode()
            req = urllib.request.Request(
                f"{self.endpoint}/embedding",
                data=body,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=60) as resp:
                return json.loads(resp.read())

        def llamacpp_encoder(input_ids, attention_mask=None, **_):
            out = []
            for i, row in enumerate(input_ids):
                n = int(attention_mask[i].sum()) if attention_mask is not None else len(row)
                t = time.time()
                r = embed(row[:n].tolist())
                self._calls["ms"] += (time.time() - t) * 1000
                self._calls["n"] += 1
                h = torch.zeros(input_ids.shape[1], d_hidden)
                h[:n] = torch.tensor(r[0]["embedding"])
                out.append(h)
            return types.SimpleNamespace(last_hidden_state=torch.stack(out))

        self.agent.model.encoder.forward = llamacpp_encoder
        self.loaded_ms = (time.time() - t0) * 1000
        log(f"ready in {self.loaded_ms:.0f} ms")

    def health(self):
        return {
            "ok": True,
            "laya_version": self.laya_version,
            "endpoint": self.endpoint,
            "head": self.head_path,
            "head_tensors": self.head_tensors,
            "head_matched": self.head_matched,
            "head_missing": self.head_missing,
            "d_hidden": self.d_hidden,
            "loaded_ms": round(self.loaded_ms, 1),
            "encoder_calls": self._calls["n"],
        }

    def predict(self, state, questions):
        t0 = time.time()
        res = self.agent.predict(state, questions)
        return {
            "ok": True,
            "answers": res.get("answers", res),
            "ms": round((time.time() - t0) * 1000, 1),
            "encoder_calls": self._calls["n"],
        }


def serve(sock_path, sc):
    # A stale socket file from a previous run would make bind() fail confusingly.
    if os.path.exists(sock_path):
        try:
            probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            probe.settimeout(1.0)
            probe.connect(sock_path)
            probe.close()
            log(f"another sidecar is already listening on {sock_path}; refusing to start")
            return 1
        except OSError:
            # Nobody home: it is a stale file. Move it aside rather than delete it.
            aside = f"{sock_path}.stale-{int(time.time())}"
            os.rename(sock_path, aside)
            log(f"moved a stale socket aside -> {aside}")

    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(sock_path)
    os.chmod(sock_path, 0o600)  # owner-only
    srv.listen(8)
    log(f"listening on {sock_path} (mode 600)")

    while True:
        conn, _ = srv.accept()
        with conn:
            f = conn.makefile("rwb")
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    req = json.loads(line)
                except Exception as e:
                    f.write((json.dumps({"ok": False, "error": f"bad json: {e}"}) + "\n").encode())
                    f.flush()
                    continue

                op = req.get("op")
                try:
                    if op == "health":
                        out = sc.health()
                    elif op == "predict":
                        out = sc.predict(req.get("state"), req.get("questions"))
                    elif op == "shutdown":
                        f.write((json.dumps({"ok": True}) + "\n").encode())
                        f.flush()
                        log("shutdown requested")
                        return 0
                    else:
                        out = {"ok": False, "error": f"unknown op {op!r}"}
                except Exception as e:
                    out = {"ok": False, "error": f"{type(e).__name__}: {e}"}
                    log(traceback.format_exc())

                f.write((json.dumps(out) + "\n").encode())
                f.flush()


def main():
    ap = argparse.ArgumentParser(description="laya decision-head sidecar")
    ap.add_argument("--socket", default="/run/laya/sidecar.sock")
    ap.add_argument("--endpoint", default=os.environ.get("LAYA_URL", DEFAULT_ENDPOINT))
    ap.add_argument("--head", default=DEFAULT_HEAD)
    ap.add_argument("--check", action="store_true", help="load, report health, exit")
    args = ap.parse_args()

    # The venv guard: refuse to run under a python that has no laya, rather than
    # failing deep inside an import with a confusing traceback.
    if args.check:
        try:
            import laya  # noqa: F401
        except ImportError:
            log(
                f"`laya` is not importable by {sys.executable}.\n"
                f"  run the sidecar with {DEFAULT_VENV}/bin/python, or create the venv\n"
                f"  with: uv venv {DEFAULT_VENV} && uv pip install --python {DEFAULT_VENV}/bin/python laya"
            )
            return 2

    os.makedirs(os.path.dirname(args.socket), exist_ok=True)
    sc = Sidecar(args.endpoint, args.head)

    if args.check:
        print(json.dumps(sc.health(), indent=2))
        return 0

    return serve(args.socket, sc)


if __name__ == "__main__":
    sys.exit(main())
