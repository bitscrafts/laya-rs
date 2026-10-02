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
    "03-reference/GRAPH-FORMAT-CONTRACT.md": (
        "The flow format contract — what changes, why, and what it costs",
        "The twelve changes that make every declared field either work, be refused by name, or be "
        "documented as carrying no behaviour. With the measured 'before' for each: a typo of any key "
        "is silently inert, four silent misroutes pass a validate, and the reference doc advertises "
        "$.inputs, a feature that does not exist."
    ),
    "03-reference/EXAMPLES-NEED-LAYA-MCP-ON-PATH.md": (
        "The examples need laya-mcp on PATH — measured, and it is NOT intermittent",
        "Every worked example runs a decision node, and the command spawns the server through the "
        "caller's PATH, so the examples fail from a shell without the install directory. Measured three "
        "ways. First mistaken for a flake: a nondeterministic cause and a differing environment look "
        "identical from a single result."
    ),
    "03-reference/GRAPH-FEATURES.md": (
        "Graph features — composability, branches, loops: what is implemented",
        "Measured by running, not reading. Branches: yes, working. Composability: implemented in the "
        "library with tests, UNREACHABLE from the CLI (no library flag, and the error message blames "
        "the wrong field). Loops: no, refused by design with the cycle path named."
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
    "04-development/OPUS-REVIEW-GENERIC-GRAPHS.md": (
        "Opus review — running generic graphs through laya",
        "An independent review that re-ran every claim with laya graph run. Found that routing is not "
        "a function of the graph (node ARRAY ORDER decides execution), that the near-tie target runs "
        "unconditionally, and that 'no data flow at all' was wrong: data CAN reach the head, it "
        "cannot reach an edge. Five contradictions, three re-verified here."
    ),
    "04-development/GENERIC-GRAPHS-ANALYSIS.md": (
        "Analysis — why a generic graph cannot run today",
        "The engine has no data flow at all: every decide input is a literal compiled into the flow "
        "file, and all four mechanisms that could carry run-time data are absent. The two-phase "
        "execution model, what a flow can actually express, and a ranked set of fixes."
    ),
    "04-development/ENGINE-DEFECTS-2026-10-02.md": (
        "Engine defects — three confirmed findings in the graph executor",
        "depends_on does not order a decide before its shell (measured: the shell ran LAST); "
        "escalate_on_near_tie is validated but inert; and a decide's question type is hardcoded to "
        "choice, so a flow can never ask the noul the head answers best."
    ),
    "04-development/SCHEDULER-SINGLE-GUARD.md": (
        "The scheduler's single guard, and the mutants that hang it",
        "dependency_tables is the shared source of the graph's shape for topo_order, refuse_cycle and "
        "the Tracker, and nothing checks it is about the graph at all. refuse_cycle is the only cycle "
        "guard and is built on the same table. There is no spin bound anywhere, so a node that never "
        "settles is waited on forever -- the two mutants the gate reports as timeouts."
    ),
    "04-development/STALE-REFUSALS-2026-10-02.md": (
        "Stale refusals — a rule that outlives its reason is a defect too",
        "Spec 20 refused timeout and allow_failure on a decide 'until spec 17'. Spec 17 landed and the "
        "timeout now works: a decide with timeout zero fails as timed out. The refusal was stale and "
        "blocked a working feature. The other half is worse than refused -- allow_failure is accepted, "
        "looks honoured, then the synthetic route node stops the run anyway."
    ),
    "02-guides/wc-classify-measured.flow.json": (
        "wc-classify-measured.flow.json — a word count that drives the branch",
        "The end-to-end example for the placeholder: the shell emits only the number and the decision "
        "reads it, so the branch flips with the document."
    ),
    "02-guides/GRAPH-EXAMPLE-GUESSING.md": (
        "Graph example — the guessing game, and the boundary it is built to state",
        "The second worked example: a shell produces a round's secret and guess, the decide's prompt "
        "carries the {{setup}} placeholder so the head judges the actual round, and the graph routes "
        "to the advice. One round per run -- the no-loop boundary documented and measured (the cycle "
        "refusal is run in the document), with a control pair and a break-the-port drill."
    ),
    "02-guides/guess-the-number.flow.json": (
        "guess-the-number.flow.json — one round of the guessing game as a graph",
        "setup (shell) prints the round's facts, judge (decide) reads them through {{setup}} and "
        "routes on label to advise_lower / advise_higher / escalate, with emit_move joining on the "
        "judgement (an AND edge from a pruned branch would prune the join -- measured)."
    ),
    "02-guides/guess-the-number.control.flow.json": (
        "guess-the-number.control.flow.json — the pair's control",
        "The same graph with the guess on the other side of the secret; it must reach the OTHER "
        "advice node, and the pair collapses to identical branches when routing is broken."
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
    "05-operations/TEST-HARNESS-FLAKE.md": (
        "A flake in the test harness — ETXTBSY on the fake MCP server",
        "The suite is not deterministic: fake_server names its scratch file from the process id plus "
        "a tag, every test shares one pid, so two tests with the same tag write and exec the same "
        "path and the kernel refuses with Text file busy. Measured 1 failure in ~20 binary runs, "
        "load-dependent, and proven not to come from the change that was being verified."
    ),
    "05-operations/RESUMING-OPUS.md": (
        "Resuming the opus analysis session",
        "How to continue the claude+opus review session instead of starting fresh: the session id, "
        "the overridable BRIEF/REPORT variables, the exit code that means a dispatch failed, and the "
        "two failure modes that cost a round on 2026-10-02 -- blocked tools answering from prior "
        "context, and a prompt that hardcoded the round-1 brief while the preflight printed the "
        "override."
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
# Checks two things per link: the FILE resolves, and if the link carries a `#fragment`
# the target file has a heading whose slug matches it.
#
# Why the fragment is checked rather than ignored (fixed 2026-10-02): the earlier version passed the
# whole link to `.exists()`, so `OTHER.md#some-anchor` was treated as a FILENAME and every anchored
# link reported BROKEN -- a false positive that taught nobody anything. Worse, stripping the fragment
# and checking only the file would silently accept a link pointing at a heading that does not exist.
# A cross-reference is only useful if the reader lands on the section it names.
def heading_slugs(path):
    """GitHub-style heading slugs: lowercase, drop punctuation, spaces -> hyphens."""
    slugs = set()
    for line in path.read_text(errors="replace").splitlines():
        m = re.match(r"^(#{1,6})\s+(.*?)\s*$", line)
        if not m:
            continue
        text = m.group(2)
        text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)   # links keep their text
        text = text.replace("`", "")                            # inline code
        slug = re.sub(r"[^\w\s-]", "", text.lower())
        slug = re.sub(r"\s+", "-", slug.strip())
        slugs.add(slug)
    return slugs


files = list((root / "_docs").rglob("*.md")) + [root / "README.md", root / "DOCS.md"]
broken = []
checked = 0
anchors_checked = 0
for f in files:
    if not f.exists():
        continue
    for m in re.finditer(r"\[([^\]]+)\]\(([^)]+)\)", f.read_text()):
        link = m.group(2)
        if link.startswith(("http://", "https://", "mailto:")):
            continue
        checked += 1
        path_part, _, fragment = link.partition("#")
        target = (f.parent / path_part) if path_part else f
        if not target.resolve().exists():
            broken.append(f"{f.relative_to(root)} -> {link}")
            continue
        if fragment:
            if not path_part:                     # same-file anchor
                target = f
            if fragment not in heading_slugs(target):
                broken.append(f"{f.relative_to(root)} -> {link}  (no heading slugs to `{fragment}`)")
            anchors_checked += 1
print(f"\nlink check: {checked} relative links checked, {anchors_checked} of them anchored, "
      f"{len(broken)} broken")
for b in broken:
    print(f"  BROKEN: {b}")
