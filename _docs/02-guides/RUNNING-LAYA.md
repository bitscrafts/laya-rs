# Running Laya + llama.cpp

How to stand up the whole decision layer on `lnx`, from nothing to a verified answer.

**Audience**: an operator or agent who has not read the source. Every command is real
and was run; where a number appears it was measured, not estimated.

**The stack in one line**: llama.cpp serves the ModernBERT **backbone** as a container;
a Python **sidecar** holds the decision **head** (the tensors the GGUF does not carry);
`laya-mcp` exposes both to agents over MCP stdio.

---

## 1. Why there are two processes, not one

This is the fact everything else follows from. The published GGUF contains **only the
backbone**. From the GGUF README, verbatim: *"the decision head ... must run outside
llama.cpp"*. So:

| Piece | What it is | Where it runs | Why there |
|---|---|---|---|
| Backbone | ModernBERT-large encoder (~421M) | `llamacpp-laya` container (llama.cpp) | llama.cpp is faster and quantised (421 MB vs 843 MB) |
| Head | 26,512,134 params, 2 layers | `laya-sidecar` (Python + torch) | ~100 tensor ops the GGUF does not contain |
| Bridge | `laya-mcp` (Rust) | MCP stdio, per agent session | one binary for every consumer |

Measured parity between the two paths: **Δp = 0.0006** (0.9670 pure PyTorch vs 0.9664
hybrid). The head's own config lives **inside** the safetensors — read it, never
hardcode it.

**Both halves are mandatory.** A backbone that can embed but a head that cannot decide
is not a decision layer. `laya doctor` checks both (see §5).

---

## 2. Start the backbone (container, host-level)

The backbone is a **host** container — it is *not* started from inside this container.

```bash
# on lnx (the host)
systemctl --user status llamacpp-laya.service
# or, if it is not up:
systemctl --user start llamacpp-laya.service
```

Verify — from **inside** this container:

```bash
curl -s http://10.10.10.217:8083/health      # -> {"status":"ok"}
curl -s http://10.10.10.217:8083/v1/models   # -> /models/laya-Q8_0.gguf
```

Two traps, both real and both hit during development:

* **The capability string is `["completion"]`, not `"embedding"`.** The GGUF holds the
  backbone only, so a readiness check that insists on an `"embedding"` capability
  reports a *working* server as broken. Check the **model id** instead.
* **`/embedding` is `POST`.** A `GET` returns a confusing error.

Throughput measured: `/embedding` **35 ms median** sequential; **8 concurrent = 121 ms
wall (5.8× parallel)**. Embedding is not the bottleneck, so there is no queue in front
of it.

---

## 3. Install the head sidecar (one-off, inside the container)

```bash
cd /projects/laya-rs
bash scripts/setup-runtime.sh
```

This creates `.venv-laya` (pinned to **laya==0.3.5**) and a launcher at
`~/.local/bin/laya-sidecar`. Expect **~5.4 GB** — it installs torch. CPU-only is fine
(`torch.cuda.is_available()` is False here and the whole layer is designed for that).

Why its **own** venv: `import laya` fails from system python (verified), and reaching
into another project's venv is exactly the fragile coupling that makes a runtime
unreproducible. Pin it, own it.

Check the head loads without starting a server:

```bash
laya-sidecar --check
# expect: head tensors=36 matched=36 ... d_hidden 1024, ready in ~24-29 s
```

**The 24 s load is the reason the sidecar is persistent.** A warm decision is
~0.4–0.7 s; a per-call spawn would pay 24 s *every time*.

---

## 4. Run the head

**Supervised (recommended, survives reboot).** See
[SUPERVISION.md](../05-operations/SUPERVISION.md) — install the host unit once, and you never start the
head by hand again.

**By hand (for a quick look):**

```bash
laya-sidecar                      # foreground; ~24-29 s to load, then serves
laya-sidecar --socket /tmp/x.sock # a different socket
```

The socket is `0600`, owner-only, under `/run/laya/sidecar.sock` by default. It is a
**unix socket rather than a port on purpose**: the sidecar answers arbitrary questions
about arbitrary state, so it must not be reachable over the network.

`/run` is **tmpfs inside the container** — a container restart empties it. That is one
of the two reasons a socket can be missing, and the diagnostic says so.

---

## 5. Verify — `laya doctor`

```bash
laya doctor
```

```
laya: READY
  endpoint  : http://10.10.10.217:8083
  /health   : ok
  /v1/models: ok (serving the expected model)
  head      : ok (sidecar at /run/laya/sidecar.sock)
```

Exit **0** ready, **1** not. This is the check to put in a cron job or a health probe —
it is the only one that covers the whole path.

> **A defect this check had, and why it is documented here.** `doctor` used to probe
> only the backbone and print **READY** while the sidecar was dead — so it certified a
> decision layer that could not answer a single question. It now checks the head too,
> and a head failure reads differently from a backbone failure on purpose:
>
> ```
> laya: the backbone is UP but the DECISION HEAD is not usable
>   endpoint : ... (backbone — healthy, do NOT restart it)
>   reason   : no sidecar socket at ... — the head was never started, or the
>              container restarted and its tmpfs /run was emptied
>   check    : laya-sidecar --check
>   fix      : start the head — `laya-sidecar` loads in ~24 s
> ```
>
> Restarting the backbone would be the wrong fix there, and the message says so. A
> check that passes while the live path is broken is worse than no check: it converts
> an open defect into a certified one.

Override the socket with the same env var the server uses, so the two can never
disagree about where the head is:

```bash
LAYA_SIDECAR_SOCKET=/run/laya/other.sock laya doctor
```

---

## 6. A real decision, end to end

```bash
python3 - <<'PY'
import json, socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(60)
s.connect("/run/laya/sidecar.sock")
req = {
    "op": "predict",
    "state": {"input": "Our invoice was charged twice and nobody answers my emails."},
    "questions": {
        "department": {"type": "choice",
                       "instructions": "Which team should handle this?",
                       "criteria": {"billing": "money", "technical": "the product",
                                    "sales": "buying"}},
        "churn": {"type": "noul",
                  "instructions": "Does the user threaten to leave?"},
    },
}
s.sendall((json.dumps(req) + "\n").encode())
buf = b""
while not buf.endswith(b"\n"):
    buf += s.recv(65536)
print(json.dumps(json.loads(buf.decode()), indent=2))
PY
```

Observed on this machine:

```
department -> choice "billing", confidence 0.7283,
              probabilities {billing 0.9311, sales 0.0347, technical 0.0342}
churn      -> noul 0.2644, confidence 0.7356
```

> **`act_probability` is 1.0 on every question type, including gibberish.** It is
> inert. Never gate on it. Confidence is the field that means something.

> **A `noul`'s confidence is a different quantity from a `choice`'s.** noul is
> `max(p, 1−p)`; choice/score is `1 − H(p)/log k`. Verified against the live model:
> noul 0.1914 → confidence 0.8086 = `max(0.1914, 0.8086)` exactly. This is why the
> policy carries **per-type** thresholds — one global band would make "0.6 confident"
> mean two different things.

---

## 7. Temperature: Laya's own clamp, do not reimplement it away

The checkpoint ships a temperature for `choice:11+` of **0.1006**. That value is
**outside** the range Laya will apply, and the runtime says so **verbatim**:

```
RuntimeWarning: laya: this checkpoint ships temperatures outside [0.5, 5] which would
distort confidence; clamping choice:11+=0.1006. Treat confidence from the affected
buckets as uncalibrated.
```

The library clamps to `[0.5, 5.0]`. **A naive port that used the raw config value would
sharpen eleven-option logits ~10× and publish a 0.24 top probability as 0.99** — which
defeats every confidence gate downstream. `laya-head::clamp_temperature` reproduces the
clamp, NaN→1.0 included, and there is a test for the NaN case.

---

## 8. Determinism, precisely

Laya's answer is an **argmax** (`laya/agent.py`), with **no sampling anywhere** in the
decision path (grep for `multinomial` / `torch.rand` / `np.random`: no hits).
Temperature is applied to the probabilities *before* the argmax, so it changes the
**confidence**, never the **edge** — argmax is invariant under monotone scaling. Route
stability and calibration are therefore separable.

Two honest limits:

* **Near-ties can flip an edge.** The measured drift between the pure and hybrid paths
  is Δp = 0.0006, which is enough to flip a 0.5001-vs-0.4999 decision. The payload
  therefore carries the **top1−top2 margin** so a near-tie can be escalated instead of
  silently decided.
* **Determinism is within one fixed build.** Record provenance (laya version, head
  hash, backbone quantisation) alongside a decision, or "replayable" is not true.

---

## 9. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `connection failed` to `:8083` | backbone container down | on the host: `systemctl --user start llamacpp-laya` |
| `no sidecar socket at ...` | head never started, **or** the container restarted and emptied the tmpfs `/run` | `systemctl --user restart laya-sidecar` (on the host) |
| `head_tensors: 0` or `matched < tensors` | the `.safetensors` did not load | check `LAYA_HEAD` points at the real file |
| `DatabaseAlreadyOpen` | two processes on one redb store (by design: redb is single-writer) | serialise the writers; this is a typed error, not corruption |
| `laya-mcp` exits immediately, no handshake | **preflight failed** — this is correct behaviour | run `laya doctor` for the reason |
| an MCP tool answers `laya-rs: ... not implemented` | the client is bound to a **stale binary** from before a rebuild | restart the client session; MCP clients do not hot-reload |

---

## 10. What is verified

```
cargo xtask gate                     PASS (fmt + clippy -D warnings + test)
tests                                148, whole workspace
clippy --workspace                   0 warnings
cargo fmt --all --check              clean
doc links                            48 checked, 0 broken
laya doctor                          READY (backbone + head)
supervision                          kill the head -> restored in ~30 s, unattended
```

Numbers above are from this container on 2026-09-29/30. Re-running the commands is the
only way to keep them true.
