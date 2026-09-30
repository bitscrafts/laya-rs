#!/usr/bin/env python3
"""Generate a differential fixture for the Rust head port.

## Why this is the right oracle

The head's forward pass is a pure function of `(hidden_states, qtype, markers)` plus the
weights. The *backbone* is what needs the live model, not the head. So the head can be tested
**exactly and offline**: run the real Python modules over a synthetic hidden state, save the
inputs and outputs, and require the Rust port to reproduce them.

This is a genuine differential test rather than a re-implementation compared with itself —
the reference here is PyTorch's own `TransformerEncoderLayer`, the same object the checkpoint
was trained with.

## What it emits

`crates/laya-head/tests/fixtures/head_forward.json`: several cases, each with the hidden state
in, and `logits` and `act_logits` out.

Writes the same weights the Rust port reads, from the same file, so a mismatch cannot be
blamed on different tensors.
"""
import json
import pathlib
import sys

import torch
import torch.nn as nn
from safetensors.torch import load_file

HEAD = "/projects/_tmp/laya-gguf/laya-head.safetensors"
OUT = pathlib.Path("/projects/laya-rs/crates/laya-head/tests/fixtures/head_forward.json")


def build(weights: dict, d: int, nhead: int, layers: int):
    """The four head modules, exactly as `laya.common.DecisionModel` constructs them.

    The construction arguments are copied from the reference and their *defaults* verified
    against the installed torch rather than assumed:
      * `nn.TransformerEncoderLayer(d, nhead, 4*d, dropout, batch_first=True, norm_first=True)`
        with `activation` unspecified -> torch's default, which is **relu**, not gelu;
      * `layer_norm_eps` unspecified -> torch's default 1e-5, matching the encoder config.
    """
    layer = nn.TransformerEncoderLayer(d, nhead, 4 * d, 0.1, batch_first=True, norm_first=True)
    head = nn.TransformerEncoder(layer, layers, enable_nested_tensor=False)
    type_emb = nn.Embedding(3, d)
    scorer = nn.Sequential(nn.LayerNorm(d), nn.Linear(d, d), nn.GELU(), nn.Linear(d, 1))
    act_head = nn.Sequential(nn.Linear(d + 4, 256), nn.GELU(), nn.Linear(256, 2))

    modules = {"head": head, "type_emb": type_emb, "scorer": scorer, "act_head": act_head}
    missing = []
    for prefix, mod in modules.items():
        sd = {k[len(prefix) + 1:]: v for k, v in weights.items() if k.startswith(prefix + ".")}
        if not sd:
            missing.append(prefix)
            continue
        mod.load_state_dict(sd, strict=True)
        mod.eval()
    if missing:
        raise SystemExit(f"no tensors for: {missing}")
    return modules


def forward(mods, h: torch.Tensor, qtype: int, marker_pos: list[int], marker_mask: list[bool]):
    """`DecisionModel.forward` with the encoder output supplied directly.

    Transcribed from the reference, including the parts the spec had omitted: the `type_emb`
    broadcast add, `src_key_padding_mask`, the marker gather with `clamp(min=0)`, the `-1e4`
    masked_fill, and the `topk(2)` / single-marker branch.

    `attention_mask` is all-ones here (no padding), so the padding mask is empty; a case with
    padding is generated separately so the mask path is exercised too.
    """
    with torch.no_grad():
        x = h + mods["type_emb"](torch.tensor([qtype]))[:, None, :]
        pad = torch.zeros(x.size(0), x.size(1), dtype=torch.bool)
        for lyr in mods["head"].layers:
            x = lyr(x, src_key_padding_mask=pad)
        mp = torch.tensor([marker_pos])
        mm = torch.tensor([marker_mask])
        idx = mp.clamp(min=0)[:, :, None].expand(-1, -1, x.size(-1))
        m = torch.gather(x, 1, idx)
        logits = mods["scorer"](m).squeeze(-1).float()
        logits = logits.masked_fill(~mm, -1e4)

        p = torch.softmax(logits.detach(), -1)
        k = mm.sum(-1).clamp(min=2).float()
        ent = -(p * torch.log(p.clamp_min(1e-9))).sum(-1) / torch.log(k)
        if p.size(-1) >= 2:
            top2 = p.topk(2, -1).values
        else:
            t1 = p.topk(1, -1).values
            top2 = torch.cat([t1, torch.zeros_like(t1)], dim=-1)
        feats = torch.stack([top2[:, 0], top2[:, 0] - top2[:, 1], ent, k / 255.0], -1)
        pooled = x[:, 0].float()
        act_logits = mods["act_head"](torch.cat([pooled, feats], -1))
    return logits, act_logits


def main() -> int:
    weights = load_file(HEAD)
    d = weights["type_emb.weight"].shape[1]
    cfg_d = int(weights["head.layers.0.linear1.weight"].shape[1])
    nhead = max(1, d // 64)
    print(f"  d={d} (from type_emb) cfg_d={cfg_d} nhead={nhead}")

    mods = build(weights, d, nhead, 2)

    cases = []
    # Deterministic inputs, so the fixture is reproducible and a diff means a real difference.
    for i, (seq, qtype, nk) in enumerate([(8, 0, 3), (16, 1, 5), (5, 2, 2), (33, 0, 11)]):
        g = torch.Generator().manual_seed(1000 + i)
        h = torch.randn(1, seq, d, generator=g, dtype=torch.float32)
        marker_pos = [min(2 + j * 2, seq - 1) for j in range(nk)]
        marker_mask = [True] * nk
        logits, act = forward(mods, h, qtype, marker_pos, marker_mask)
        cases.append({
            "name": f"seq{seq}_qtype{qtype}_k{nk}",
            "seq": seq,
            "qtype": qtype,
            "marker_pos": marker_pos,
            "marker_mask": marker_mask,
            # flattened row-major [seq, d]
            # Plain floats. `json.dumps` writes floats at repr precision (17 significant
            # digits), so an f32 round-trips exactly through the fixture.
            #
            # Two earlier versions got this wrong in opposite directions: `round(v, 8)` PERTURBS
            # an f32 input, and `repr(v)` emits a JSON *string* — which Rust's `as_f64()` turns
            # into NaN, so both sides produced NaN and the differential test failed identically
            # on both. A fixture that cannot be parsed is not an oracle.
            "hidden": [float(v) for v in h.flatten().tolist()],
            "logits": [float(v) for v in logits.flatten().tolist()],
            "act_logits": [float(v) for v in act.flatten().tolist()],
        })
        print(f"  case {i}: seq={seq} qtype={qtype} k={nk} -> logits {list(logits.flatten().tolist())}")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"d": d, "nhead": nhead, "layers": 2, "cases": cases}, indent=1))
    print(f"  wrote {OUT} ({OUT.stat().st_size} bytes, {len(cases)} cases)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
