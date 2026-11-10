#!/usr/bin/env bash
# Mark an issue `ready`. Usage: scripts/mark-ready.sh <issue-number> [--owner-approved] [owner/repo]
# `ready` means "specified well enough to start", so only a maintainer may set it. This script is how the
# maintainer's assistant does it without opening the door to strangers: an issue written by anyone other
# than the repository owner is refused unless the owner approved it explicitly (--owner-approved).
set -eu
N="${1:?usage: mark-ready.sh <issue-number> [--owner-approved] [owner/repo]}"; shift
APPROVED=0; [ "${1:-}" = "--owner-approved" ] && { APPROVED=1; shift; }
REPO="${1:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"
OWNER="${LAYA_REPO_OWNER:-${REPO%%/*}}"
AUTHOR=$(gh issue view "$N" --repo "$REPO" --json author -q .author.login)
STATE=$(gh issue view "$N" --repo "$REPO" --json state -q .state)
[ "$STATE" = "OPEN" ] || { echo "REFUSE: #$N is $STATE"; exit 3; }
if [ "$AUTHOR" != "$OWNER" ] && [ "$APPROVED" != 1 ]; then
  echo "REFUSE: #$N was written by $AUTHOR, not $OWNER. Read it, then re-run with --owner-approved if $OWNER says so."
  exit 4
fi
gh issue edit "$N" --repo "$REPO" --remove-label triage --add-label ready >/dev/null
echo "ready: #$N (author $AUTHOR$([ "$AUTHOR" != "$OWNER" ] && echo ', owner-approved'))"
