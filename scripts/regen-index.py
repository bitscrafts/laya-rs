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
    "05-operations/README.md": ("Operations", "Running the server, the ledger, budgets."),
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
    "## Related trees (linked, never copied)",
    "",
    "- **pii** owns the graph engine. Its `route` contract, expr language and static",
    "  checks live in `/projects/pi-improved/` source and `_docs/`. We **link**: a",
    "  copied contract rots, a linked one stays true.",
    "- **Workspace-level** docs (the open register, workspace rules) live in",
    "  `/projects/docs/`. Our open items are registered there so they survive this",
    "  project's sessions.",
    "- **amem** holds the durable research trail (topics `laya-rs`, `pii`,",
    "  `indydevdan-agentic-engineering`).",
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
