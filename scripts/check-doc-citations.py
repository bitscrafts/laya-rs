#!/usr/bin/env python3
"""Doc-honesty gate: every citation in our docs must resolve to something that EXISTS.

WHY THIS EXISTS (2026-09-30). Writing `_docs/03-reference/GRAPH-SEAM.md` I put
`laya-core::validate` in a table — a module that does not exist. I invented it. Nothing caught it:
prose is not compiled, and the existing `link check` only verifies that a relative LINK resolves,
not that a CLAIM is true.

WHAT IT CHECKS (mechanical, and only mechanical):

  1. `laya-<crate>::<name>` — resolves if `<name>` is a module in that crate OR a public item
     (`fn`/`const`/`struct`/`enum`/`trait`/`type`) declared in it. A citation is often a function,
     so treating every `::` as a module would be wrong.
  2. `path/to/file.ext:NN[-MM]` — the file must exist AND have at least line NN.
     Relative to our repo first; then, for bare filenames, to the locations our docs legitimately
     cite: the pinned laya virtualenv. A bare `expr.rs:3-8` is a real citation and must resolve, not be silenced.
  3. Third-party citations (a dependency's source, e.g. rmcp's `tool_handler.rs`) are declared in
     `ALLOWED_EXTERNAL` with the reason, so they are documented rather than silently skipped.

It does NOT attempt prose semantics — that is not mechanical, and a checker that guesses would be
worse than none. It checks citations, which is exactly where the invented claim lived.

Exit 1 if any citation does not resolve.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
VENV_SITE = next(ROOT.glob(".venv-laya/lib/python*/site-packages"), None)

DOC_GLOBS = ["_docs/**/*.md", "specs/**/*.md", "AGENTS.md", "HANDOFF.md", "README.md"]

# Citations into third-party source we do not ship. Declared, with the reason, rather than skipped:
# "allowlisted" and "not checked" are different things, and only one of them is honest.
ALLOWED_EXTERNAL = {
    "tool_handler.rs": "rmcp (the MCP framework) — cited in the code review for its generated "
                       "get_info(); the crate is a dependency, not our source",
}

# HISTORICAL documents, exempted as a CLASS and with the reason stated.
#
# These are records of what was found at a point in time by reading OTHER projects' source. The
# citations are part of the finding, and rewriting them would falsify the record — the opposite of
# what this project values ("never erase a superseded claim; the disagreement is the finding").
#
# They are exempted as a *category* rather than by silencing individual lines: a new citation in
# one of these files is still fine, and a new citation ANYWHERE ELSE is still an error.
#
# FORWARD docs — specs, README, AGENTS.md, HANDOFF.md, and 01/02/03/05 — are NOT exempt and must
# be self-contained: they state what we rely on as a fact about the interface (user, 2026-09-30:
# no association with any other project).
EXEMPT_HISTORICAL = (
    "_docs/06-research/",              # research transcripts, incl. an external model's analysis
    "_docs/04-development/DESIGN-AMENDMENTS-",   # the review response as it was written
)

CRATE_ITEM = re.compile(r"\blaya-([a-z]+)::([a-z_]+)\b")
FILE_LINE = re.compile(r"\b([\w./-]+\.(?:rs|toml|py|sh)):(\d+)(?:-(\d+))?\b")

_PUB_ITEM_CACHE: dict[str, str] = {}


def crate_source(crate: str) -> str:
    """Concatenated source of one of our crates, cached."""
    if crate not in _PUB_ITEM_CACHE:
        d = ROOT / "crates" / f"laya-{crate}" / "src"
        text = ""
        if d.is_dir():
            for f in d.rglob("*.rs"):
                text += f.read_text(errors="replace")
        _PUB_ITEM_CACHE[crate] = text
    return _PUB_ITEM_CACHE[crate]


def module_or_item_exists(crate: str, name: str) -> bool:
    """`laya-core::policy` may be a module OR `laya-head::clamp_temperature` a public function."""
    if name in ("self", "crate", "super"):
        return True
    src = ROOT / "crates" / f"laya-{crate}" / "src"
    if not src.is_dir():
        return False
    if (src / f"{name}.rs").is_file() or (src / name / "mod.rs").is_file():
        return True
    if re.search(rf"pub mod {name}\b", crate_source(crate)):
        return True
    # A public item, declared at COLUMN 0. The anchor is load-bearing: `Policy::validate` is
    # indented inside an `impl`, and without it `laya-core::validate` — a module that does not
    # exist — matched that method, which is a false negative on the very citation that prompted
    # this checker.
    return bool(re.search(
        rf"^pub (?:const |async )?(?:fn|struct|enum|trait|type|static) {name}\b",
        crate_source(crate), re.MULTILINE))


# The roots a citation may resolve in. NOTE WHAT IS NOT HERE: no OTHER PROJECT. Our docs must
# be self-contained — a citation into another project's source is an association we neither have
# nor want, and it is unverifiable from here. Allowed: this repo, the evidence directory the
# measurements were recorded in, and the pinned laya virtualenv.
EVIDENCE = pathlib.Path("/projects/_tmp")
RESOLVE_ROOTS = [ROOT, EVIDENCE, VENV_SITE]


def _candidates(rel: str) -> list[pathlib.Path]:
    """Every file a citation could mean, across the roots, in priority order."""
    out: list[pathlib.Path] = []
    for base in RESOLVE_ROOTS:
        if base is None:
            continue
        cand = base / rel
        if cand.is_file():
            out.append(cand)
    if "/" in rel:
        return out  # an explicit path is not searched by name
    for base in RESOLVE_ROOTS:
        if base is None:
            continue
        # Prefer the laya package for its own module names, then the shallowest match.
        preferred = base / "laya" / rel
        if preferred.is_file():
            out.append(preferred)
        out.extend(sorted((h for h in base.rglob(rel) if h.is_file()),
                          key=lambda h: (len(h.parts), str(h))))
    return out


def resolve_citation(rel: str, line: int) -> tuple[bool, str]:
    """Is a `file:NN` citation resolvable?

    A BARE FILENAME IS AMBIGUOUS — `main.rs:44` could be a binary's, xtask's or the head's, and
    nothing in the citation says which. So the honest question is not "does the FIRST candidate
    have that line" (that produced false alarms on xtask's 19-line `main.rs`) but "does ANY
    candidate have it". A citation is checked for RESOLVABILITY, never for which file was meant;
    pinning the ambiguity would need the doc to write the full path, which is a doc fix, not a
    checker fix.
    """
    cands = _candidates(rel)
    if not cands:
        return False, ("nothing by that path in this repo, the evidence dir, or the pinned venv "
                       "(citations into another project are not accepted)")
    tried = []
    for c in cands:
        total = sum(1 for _ in c.open(errors="replace"))
        if total >= line:
            return True, ""
        tried.append(f"{c} ({total} lines)")
    return False, f"no candidate is {line}+ lines long: " + "; ".join(tried[:3])


def check_text(text: str, label: str) -> list[str]:
    problems: list[str] = []

    for crate, name in sorted(set(CRATE_ITEM.findall(text))):
        if not module_or_item_exists(crate, name):
            problems.append(f"{label}: cites `laya-{crate}::{name}` — no such module or pub item")

    for rel, a, b in FILE_LINE.findall(text):
        if any(tok in rel for tok in ("{", "}", "*", "<", ">")):
            continue
        if rel in ALLOWED_EXTERNAL:
            continue
        first, last = int(a), (int(b) if b else None)
        ok, why = resolve_citation(rel, last or first)
        if not ok:
            problems.append(f"{label}: cites `{rel}:{a}{'-' + b if b else ''}` — {why}")

    return problems


def owned_docs():
    """Docs whose citations must resolve in this repo. Historical records are excluded."""
    for glob in DOC_GLOBS:
        for p in ROOT.glob(glob):
            if "99-archive" in p.parts:  # archived scripts cite paths that legitimately moved
                continue
            rel = str(p.relative_to(ROOT))
            if any(rel.startswith(prefix) for prefix in EXEMPT_HISTORICAL):
                continue
            if p.is_file():
                yield p


def main() -> int:
    problems: list[str] = []
    docs = list(owned_docs())
    for path in docs:
        problems.extend(check_text(path.read_text(errors="replace"), str(path.relative_to(ROOT))))

    if problems:
        print(f"doc-honesty: {len(problems)} unresolved citation(s) across {len(docs)} docs")
        for p in problems:
            print(f"  {p}")
        return 1
    print(f"doc-honesty: {len(docs)} docs, every citation resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
