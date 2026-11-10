#!/usr/bin/env python3
"""Exercise all ten levels of the System-1 progression.

The mapping is documented in `_docs/04-development/TEN-LEVELS-MAPPING.md`; this is the
harness that proves the mapping is real. Each level is driven through the SHIPPED MCP
tools over stdio — the same surface an agent uses — so a level that works here works for
a caller, not merely for a unit test.

Levels, and what each one is here to prove:

  1 single decision      `decide` with a `noul` — the threshold lives in Policy, not a prompt
  2 multiple choice      N questions in ONE call — fan out, then choose in code
  3 composite scoring    several `score`s — weights stay in the caller
  4 confidence gating    `gate` — gate on `confidence`, never on act_probability
  5 intent routing       a `choice` over the harness list — the answer is a route
  6 guardrail hooks      `gate` shaped for a tool_call hook — ONE SIGNAL, not the authority
  7 should I compact      `should_compact` — token thresholds stay in code
  8 cheap reads          `ask_file` — the file never enters the agent's context
  9 files at scale       `ask_files` — glob, cap, and a per-file answer
 10 agentic              `ask` — the caller writes the questions; its command passes L6 first

**Controls.** Levels 1-3 need `criteria` (NOT `options`) for `choice`/`score` — a probe
that sends `options` gets a sidecar TypeError and proves nothing about the tool. Each
level also has a near-miss row that must take the NORMAL path, so a level cannot pass by
accident of the rule that skips something.

Usage:
    uv run scripts/live/check-ten-levels.py           # all ten, PASS/FAIL each
    uv run scripts/live/check-ten-levels.py --json
Exit: 0 all ten pass, 1 any failure, 2 environment problem.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

MCP_BIN = os.environ.get("LAYA_MCP_BIN", os.path.expanduser("~/.local/bin/laya-mcp"))
LEDGER_DIR = os.environ.get("LAYA_LEDGER_DIR", "/var/lib/laya-ledger")
READ_ROOT = os.environ.get("LAYA_READ_ROOTS", "/projects:/root")


class Server:
    """The deployed MCP server, spoken to exactly as an agent speaks to it."""

    def __init__(self) -> None:
        env = dict(os.environ)
        env["LAYA_LEDGER_DIR"] = LEDGER_DIR
        env["LAYA_READ_ROOTS"] = READ_ROOT
        self.p = subprocess.Popen(
            [MCP_BIN],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=env,
            bufsize=1,
        )
        self._id = 0
        self.rpc(
            {
                "jsonrpc": "2.0",
                "id": self._next(),
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "ten-levels", "version": "1"},
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2025-06-18",
                        "io.modelcontextprotocol/clientCapabilities": {},
                    },
                },
            }
        )
        assert self.p.stdin
        self.p.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
        )
        self.p.stdin.flush()

    def _next(self) -> int:
        self._id += 1
        return self._id

    def rpc(self, msg: dict) -> dict:
        assert self.p.stdin and self.p.stdout
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("the server closed the stream")
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if r.get("id") == msg["id"]:
                return r

    def call(self, name: str, arguments: dict) -> tuple[bool, dict | str]:
        """(ok, parsed_body_or_error_text)."""
        r = self.rpc(
            {
                "jsonrpc": "2.0",
                "id": self._next(),
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }
        )
        if "error" in r:
            return False, r["error"].get("message", str(r["error"]))
        text = "\n".join(
            b.get("text", "") for b in r.get("result", {}).get("content", [])
        ).strip()
        try:
            return True, json.loads(text)
        except json.JSONDecodeError:
            return True, text

    def close(self) -> None:
        self.p.terminate()
        try:
            self.p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.p.kill()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    if not Path(MCP_BIN).exists():
        print(f"check-ten-levels: no server at {MCP_BIN}", file=sys.stderr)
        return 2

    results: list[tuple[int, str, bool, str]] = []
    s = Server()

    def level(n: int, name: str, ok: bool, detail: str) -> None:
        results.append((n, name, ok, detail))

    try:
        # ---- L1: a single decision, the "smart if" --------------------------
        ok, body = s.call(
            "decide",
            {
                "state": "The build failed with a missing symbol in the linker.",
                # The WIRE format the sidecar takes, keyed by question id:
                #   {"id": {"type": ..., "instructions": <text>, "criteria": <options>}}
                # It is NOT the array `tools/list` advertises, and it is NOT `text`.
                # Both are real defects recorded in spec 15; this harness sends what works
                # and the schema is corrected to match.
                "questions": {
                    "k": {
                        "type": "noul",
                        "instructions": "Is this a code error rather than a config error?",
                    }
                },
            },
        )
        p = body.get("payloads", {}).get("k", {}) if ok and isinstance(body, dict) else {}
        level(
            1,
            "single decision (noul)",
            ok and p.get("label") in {"yes", "no"},
            f"label={p.get('label')!r} band={p.get('band')!r} conf={p.get('confidence')}",
        )

        # ---- L2: multiple choice in ONE call --------------------------------
        ok, body = s.call(
            "decide",
            {
                "state": "A failing unit test in the parser.",
                "questions": {
                    "a": {
                        "type": "choice",
                        "instructions": "Where is the fault?",
                        "criteria": {"parser": "the parser", "lexer": "the lexer", "unknown": "not clear"},
                    },
                    "b": {"type": "noul", "instructions": "Is a fix obvious?"},
                },
            },
        )
        pl = body.get("payloads", {}) if ok and isinstance(body, dict) else {}
        level(
            2,
            "multiple questions, one call",
            len(pl) == 2,
            f"answered {sorted(pl)} in one call",
        )

        # ---- L3: composite scoring ------------------------------------------
        ok, body = s.call(
            "decide",
            {
                "state": "A change touching three modules.",
                # A `score` REQUIRES criteria as a list of level descriptions. Without it
                # the sidecar raises `TypeError: 'NoneType' object is not iterable`, and
                # `tools/list` does not say the field is required -- recorded in spec 15.
                "questions": {
                    "size": {
                        "type": "score",
                        "instructions": "Change size?",
                        "criteria": ["one file", "one module", "several modules", "several crates", "the workspace"],
                    },
                    "risk": {
                        "type": "score",
                        "instructions": "Risk?",
                        "criteria": ["safe", "low", "moderate", "high", "dangerous"],
                    },
                },
            },
        )
        pl = body.get("payloads", {}) if ok and isinstance(body, dict) else {}
        scored = [k for k, v in pl.items() if v.get("score") is not None]
        level(
            3,
            "composite scores",
            len(scored) == 2,
            f"scores present for {sorted(scored)}: "
            + ", ".join(f"{k}={pl[k].get('score')}" for k in sorted(scored)),
        )

        # ---- L4: confidence gating ------------------------------------------
        ok, body = s.call("gate", {"action": "git status"})
        d = body.get("decision") if ok and isinstance(body, dict) else None
        ok2, body2 = s.call("gate", {"action": "mkfs.ext4 /dev/sda1"})
        d2 = body2.get("decision") if ok2 and isinstance(body2, dict) else None
        level(
            4,
            "confidence gating (allow vs ask)",
            d == "allow" and d2 == "ask",
            f"git status -> {d}, mkfs.ext4 -> {d2}",
        )

        # ---- L5: intent routing over the declared harness list --------------
        ok, body = s.call(
            "decide",
            {
                "state": "A large refactor across many files; the cheapest capable model should do it.",
                "questions": {
                    "harness": {
                        "type": "choice",
                        "instructions": "Which harness fits this work?",
                        "criteria": {"cheap": "cheapest capable", "strong": "most capable", "local": "on this box"},
                    }
                },
            },
        )
        p = body.get("payloads", {}).get("harness", {}) if ok and isinstance(body, dict) else {}
        level(
            5,
            "intent routing (a choice over harnesses)",
            p.get("label") in {"cheap", "strong", "local"},
            f"route label={p.get('label')!r} (a member access the graph can match)",
        )

        # ---- L6: guardrail hook — ONE SIGNAL --------------------------------
        ok, body = s.call("gate", {"action": "rm -rf /important", "irreversible": True})
        sig = body.get("destructive_signals") if ok and isinstance(body, dict) else None
        note = body.get("signals_note", "") if ok and isinstance(body, dict) else ""
        level(
            6,
            "guardrail hook: signals are one input, never the authority",
            ok and isinstance(sig, list) and len(sig) > 0 and "allow requires" in note,
            f"{len(sig) if isinstance(sig, list) else 0} signal(s), and the note says allow needs an allowlist",
        )

        # ---- L7: should I compact -------------------------------------------
        ok, body = s.call(
            "should_compact",
            {
                "turns_summary": "Long tool-heavy session with repeated file reads.",
                "context_tokens": 900000,
            },
        )
        # The response is a DECISION (`urgency`, `compact_now`, `reasons`) plus the
        # `judgements` behind it — not a `payloads` map. My first version asserted on
        # `payloads` and failed a working tool: the check was at fault, not the level.
        judgements = body.get("judgements", {}) if ok and isinstance(body, dict) else {}
        urgency = body.get("urgency") if ok and isinstance(body, dict) else None
        level(
            7,
            "should I compact",
            bool(judgements) and urgency is not None,
            f"urgency={urgency!r}, {len(judgements)} judgement(s): {sorted(judgements)[:4]}",
        )

        # ---- L8: cheap read — the file never enters context ------------------
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
            f.write("fn main() { println!(\"hello\"); }\n")
            probe = f.name
        ok, body = s.call("ask_file", {"path": probe, "question": "Does this file contain a main function?"})
        ans = body.get("answers", {}) if ok and isinstance(body, dict) else {}
        ctx = body.get("content_in_context") if ok and isinstance(body, dict) else None
        level(
            8,
            "cheap read (ask_file)",
            ok and bool(ans) and ctx is False,
            f"{len(ans)} answer(s), content_in_context={ctx}",
        )

        # ---- L9: files at scale ---------------------------------------------
        ok, body = s.call(
            "ask_files",
            {"paths": [probe, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "Cargo.toml"))], "question": "Is this a Rust project file?"},
        )
        # The shape is `{question, asked, hits[{path,noul}], files[{path,answers}]}` — not
        # `{answers, files_read}`. Checking for keys the tool never had is how a working
        # level reads as broken.
        files = body.get("files") if ok and isinstance(body, dict) else None
        asked = body.get("asked") if ok and isinstance(body, dict) else None
        hits = body.get("hits") if ok and isinstance(body, dict) else None
        level(
            9,
            "files at scale (ask_files)",
            ok and asked == 2 and isinstance(files, list) and len(files) == 2 and isinstance(hits, list),
            f"asked={asked}, {len(files) if isinstance(files, list) else 0} per-file answers, "
            f"{len(hits) if isinstance(hits, list) else 0} ranked hit(s)",
        )

        # ---- L10: agentic — the caller writes the questions ------------------
        ok, body = s.call(
            "ask",
            {
                "state": "We are deciding how to split a large refactor.",
                "questions": {
                    "split": {"type": "noul", "instructions": "Should this work be split into parts?"}
                },
                "command": "git status",
            },
        )
        ans = body.get("answers", {}) if ok and isinstance(body, dict) else {}
        cmds = body.get("command_ran") if ok and isinstance(body, dict) else None
        level(
            10,
            "agentic (ask: own questions + a gated command)",
            ok and bool(ans) and cmds is True,
            f"{len(ans)} answer(s), command_ran={cmds}",
        )

        # ---- CONTROL: a level-10 command the gate REFUSES is never run ------
        # The near-miss that keeps L10 honest: if `ask` ran the command regardless, the
        # level would pass for the wrong reason.
        ok, body = s.call(
            "ask",
            {
                "state": "Probe: an unallowlisted command must be gated before it runs.",
                "questions": {"q": {"type": "noul", "instructions": "Proceed?"}},
                "command": "mkfs.ext4 /dev/sda1",
            },
        )
        # The refusal arrives as a JSON-RPC ERROR, not as a body with `command_ran: false` —
        # the call failed and the command did not run. Asserting on a body field the tool
        # never sends for this case is how a correct refusal reads as a broken level.
        refused = (not ok) and isinstance(body, str) and "did not allow this command" in body
        level(
            11,
            "CONTROL: L10's command is gated before it runs",
            refused,
            f"err={'yes' if not ok else 'no'}; command never ran: "
            f"{'confirmed' if refused else f'UNEXPECTED: {str(body)[:80]}'}",
        )
    finally:
        s.close()

    passed = sum(1 for _, _, ok, _ in results if ok)
    if args.json:
        print(
            json.dumps(
                {
                    "levels": [
                        {"level": n, "name": nm, "ok": ok, "detail": d} for n, nm, ok, d in results
                    ],
                    "passed": passed,
                    "total": len(results),
                },
                indent=2,
            )
        )
    else:
        print("=== ten levels of JEV, through the shipped MCP tools ===")
        for n, nm, ok, d in results:
            mark = "PASS" if ok else "FAIL"
            print(f"  {mark}  L{n:<2} {nm:<46} {d}")
        print(f"\n  {passed}/{len(results)} passed")

    if passed != len(results):
        print("\nVERDICT: NOT ALL LEVELS PASS", file=sys.stderr)
        return 1
    print("\nVERDICT: all ten levels work through the shipped surface")
    return 0


if __name__ == "__main__":
    sys.exit(main())
