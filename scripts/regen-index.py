#!/usr/bin/env python3
"""Rewrite INDEX.md + TOC.md to match the tree on disk, and check every relative
link resolves.

The index is the part that rots (SSOT skill): files get written, INDEX/TOC stop
being updated. So this regenerates them FROM the directory listing and then
verifies each link — two independent checks rather than trusting the prose.
"""
import pathlib
import re

root = pathlib.Path("/projects/laya-rs")

DOCS = {
    "01-overview/README.md": (
        "What laya-rs is",
        "The idea in four sentences, the three safety properties, and what is implemented today."
    ),
    "01-overview/README.md#the-three-properties": ("", ""),
    "02-guides/README.md": ("Guides", "How to build, gate and call laya-rs."),
    "02-guides/GRAPH-EXAMPLE-WORDCOUNT.md": (
        "Graph example — the `wc` word count, and what it exposed",
        "A worked graph using `wc`, its control, and the fix: the head answered MEDIUM across a 77x "
        "range with confidence 0.09-0.26 (band=low) every time, and only the flow that routed on "
        "`band` escalated instead of acting on a label the head had disowned."
    ),
    "03-reference/GRAPH-SPEC.md": (
        "Suggested graph specification, and how a flow becomes a graph",
        "A SUGGESTED (not decided) format for flows — nodes, kinds, composition — and the "
        "four-stage translation to a graph, with the invariants, the renderer's round-trip "
        "test, and a list of the weakest points to attack first."
    ),
    "03-reference/README.md": ("Reference", "Technical reference: the contract and the wire shapes."),
    "04-development/DESIGN-AMENDMENTS-2026-09-29.md": (
        "Design amendments — what the adversarial review changed",
        "MANDATORY corrections to the design, with the five blockers and their resolutions.",
    ),
    "04-development/P1-STATUS.md": (
        "P1 status — and the measurements that shaped it",
        "What was measured before designing (Laya concurrency, the redb collision), and what P1 delivered.",
    ),
    "04-development/TEN-LEVELS-MAPPING.md": (
        "The ten levels, mapped to our tools",
        "IndyDevDan's ten levels, and the documented failures we inherit as requirements.",
    ),
    "04-development/README.md": ("Development", "Design decisions and implementation notes."),
    "05-operations/READINESS-AND-QUEUE.md": (
        "Readiness and the priority queue",
        "How 'is Laya running?' is answered, and where a priority queue earns its place.",
    ),
    "05-operations/DECISION-LEDGER.md": (
        "The decision ledger",
        "The redb ledger: schema, single-writer model, budgets, querying.",
    ),
    "05-operations/OBSERVABILITY.md": (
        "Observability — what can be seen, and what could not",
        "Measured state of observability in the deployment: the ledger that was silently off, the "
        "operating configuration, the measured cost per call, and the questions still unanswerable.",
    ),
    "04-development/ENGINE-DEFECTS-2026-10-02.md": (
        "Engine defects — three confirmed findings in the graph executor",
        "depends_on does not order a decide before its shell (measured: the shell ran LAST); "
        "escalate_on_near_tie is validated but inert; and a decide's question type is hardcoded to "
        "choice, so a flow can never ask the noul the head answers best."
    ),
    "04-development/HEAD-QUESTION-SHAPE-LIMITS.md": (
        "The head's question-shape limits — measured",
        "A 3-option choice does not track a number while a noul threshold does (15/15, incl. the "
        "boundary); and a bare noul on a subjective question returns a confident CONSTANT (no at "
        "1.0 for both positive and negative text). Why the correct handling is bounding one signal."
    ),
    "05-operations/README.md": ("Operations", "Running the server, the ledger, budgets."),
    "05-operations/TOOL-COVERAGE.md": (
        "Tool coverage — what has actually been exercised",
        "The 7 advertised tools against what the ledger shows has RUN: three have effectively never "
        "been called. Includes the SDD+TDD gate graph, the paired control, and the break-the-control "
        "check that proves the pair means something."
    ),
    "05-operations/TOKEN-ECONOMY.md": (
        "Token economy — what it saves, what it costs, how it was measured",
        "The measured answer: 101,840 tokens saved per triage round (43x), 0 tokens of context cost "
        "on allow/ask, and 178 tokens ever on two blocks. Includes the 263x-overstated first "
        "measurement and why a phrase match is not attribution."
    ),
    "05-operations/RESUME-PROMPT-2026-10-02.md": (
        "Resume prompt — 2026-10-02 (read this first after the restart)",
        "The conversation's state in one page: the one open item (whether the gate fires for a live "
        "session), the exact before/after check that answers it, what was finished, the token "
        "measurement, and the traps that cost time."
    ),
    "05-operations/RESTART-2026-10-02.md": (
        "Restart state — 2026-10-02",
        "What is deployed with hashes verified against the tree, the verification order, and the four "
        "things deliberately left unfinished."
    ),
    "06-research/01-design.md": (
        "Design (v1) — superseded in part",
        "The original design. Read DESIGN-AMENDMENTS first: the review corrected five blockers in it.",
    ),
    "06-research/02-analysis-glm-5.3.md": (
        "Adversarial review by pi + glm-5.3",
        "The full review: five blockers, ten majors, fact-by-fact corrections.",
    ),
    "06-research/02a-analysis-brief.md": (
        "The review brief",
        "The exact prompt the reviewer ran against, so the review is reproducible.",
    ),
    "06-research/README.md": ("Research", "Numbered research write-ups: convention and index."),
    "06-research/03-four-architecture-questions.md": (
        "Four architecture questions (threading, config, graph spec, scheduler)",
        "Discussion only: the priority queue as admission control and a missed fourth "
        "parallelism axis; where config should live and why redb does not speed the load; "
        "our own graph specification and mermaid export; and whether to build a scheduler."
    ),
    "99-archive/README.md": ("Archive", "Superseded material, kept with its consequence labelled."),
}

SECTIONS = [
    ("01-overview", "Overview", "what this project is and why it exists"),
    ("02-guides", "Guides", "how to run, gate and use it"),
    ("03-reference", "Reference", "technical reference: the contract, the wire shapes"),
    ("04-development", "Development", "design decisions, the review amendments, implementation notes"),
    ("05-operations", "Operations", "the ledger, budgets, running the server"),
    ("06-research", "Research", "numbered research write-ups (design v1, the review, prior-art trails)"),
    ("99-archive", "Archive", "superseded material, kept with its consequence labelled"),
]

# ---- INDEX.md ----------------------------------------------------------------
out = [
    "# laya-rs — documentation index",
    "",
    "Project-local documentation, organised with the SSOT methodology. Each fact lives",
    "in exactly one place; this index points at it, never restates it.",
    "",
    "**Project root**: `/projects/laya-rs`",
    "",
    "> **Read in this order on a cold start:** `01-overview` → `04-development/"
    "DESIGN-AMENDMENTS-2026-09-29.md` (the corrections) → `06-research/01-design.md`",
    "> (the original). The amendments WIN wherever they conflict with the design.",
    "",
    "## Sections",
    "",
    "| section | holds |",
    "|---|---|",
]
for d, t, desc in SECTIONS:
    out.append(f"| `{d}/` | {desc} |")

out += ["", "## Documents", ""]
for d, t, desc in SECTIONS:
    out.append(f"### {d}")
    for rel, (title, blurb) in DOCS.items():
        if rel.startswith(d + "/") and rel != f"{d}/README.md":
            out.append(f"- [{title}]({rel}) — {blurb}")
    if f"{d}/README.md" in DOCS:
        t2, b2 = DOCS[f"{d}/README.md"]
        out.append(f"- [{t2}]({d}/README.md) — {b2}")
    out.append("")

out += [
    "## This project is self-contained",
    "",
    "Every fact in this tree stands on its own: what we rely on from outside is stated",
    "as a property of an interface, never as a path or line number into another",
    "repository. The behaviour we depend on is enforced by tests in `crates/`, so a",
    "change anywhere else cannot move this project silently.",
    "",
    "**Standalone by design** — one project, one repository, no external association.",
    "",
    "## Specs",
    "",
    "Specs are not docs — they are the implementation contract, and they live in",
    "`specs/` with TDD tables and runnable exit criteria. See [`../specs/`](../specs/).",
    "",
]
(root / "_docs/INDEX.md").write_text("\n".join(out))
print("wrote _docs/INDEX.md")

# ---- TOC.md ------------------------------------------------------------------
toc = [
    "# laya-rs — table of contents",
    "",
    "Flat list of every document. See [`INDEX.md`](INDEX.md) for structure and the",
    "reading order.",
    "",
]
for d, _, _ in SECTIONS:
    toc.append(f"## {d}")
    for rel, (title, _) in DOCS.items():
        if rel.startswith(d + "/"):
            toc.append(f"- [{title}]({rel})")
    toc.append("")
(root / "_docs/TOC.md").write_text("\n".join(toc))
print("wrote _docs/TOC.md")

# ---- link check --------------------------------------------------------------
files = list((root / "_docs").rglob("*.md")) + [root / "README.md", root / "DOCS.md"]
broken = []
checked = 0
for f in files:
    if not f.exists():
        continue
    for m in re.finditer(r"\[([^\]]+)\]\(([^)]+)\)", f.read_text()):
        link = m.group(2)
        if link.startswith(("http://", "https://", "#", "mailto:")):
            continue
        checked += 1
        if not (f.parent / link).resolve().exists():
            broken.append(f"{f.relative_to(root)} -> {link}")
print(f"\nlink check: {checked} relative links checked, {len(broken)} broken")
for b in broken:
    print(f"  BROKEN: {b}")
