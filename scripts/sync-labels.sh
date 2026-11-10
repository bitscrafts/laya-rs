#!/usr/bin/env bash
# Make the repository's labels match .github/labels.yml. Creates or updates; never deletes a label.
# Needs the GitHub CLI, logged in with write access. Usage: scripts/sync-labels.sh [owner/repo]
set -eu
cd "$(dirname "$0")/.."
REPO="${1:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"
python3 - "$REPO" <<'PY'
import re, subprocess, sys
repo = sys.argv[1]
for line in open(".github/labels.yml"):
    m = re.match(r'- \{name: (\S+),\s+color: (\w+),\s+description: "(.*)"\}', line)
    if not m:
        continue
    name, color, desc = m.groups()
    r = subprocess.run(["gh", "label", "create", name, "--repo", repo, "--color", color,
                        "--description", desc, "--force"], capture_output=True, text=True)
    print(("ok   " if r.returncode == 0 else "FAIL ") + name, r.stderr.strip()[:80])
PY
