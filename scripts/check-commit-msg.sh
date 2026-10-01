#!/usr/bin/env bash
# check-commit-msg.sh -- refuse a commit message that names an AI assistant or tool.
#
# WHY THIS EXISTS (HARD RULE 0c, and this is its second recurrence)
# ----------------------------------------------------------------
# The rule: commits must be indistinguishable from a human engineer's -- never name
# an AI assistant or tool in the message, author name or email.
#
# It was broken for 35 commits in this repo (register row A24, found 2026-09-30) and
# again on 2026-10-01, in THREE commits made while implementing spec 07:
#
#   a9e9310  "Implemented by a delegated sonnet session against the spec..."
#   9cf2aa4  "Implemented by a delegated sonnet session; verified and corrected here."
#   a02af96  "...it `claude -p` hangs on a file-write approval print mode cannot surface."
#
# All three have a clean AUTHOR. The author field was already fixed by A24 -- which is
# exactly why this recurred: the rule was guarded in ONE of the three places it
# applies. A rule enforced on the author but not the body is half a rule, and half a
# rule fails silently, which is the shape this workspace keeps paying for.
#
# A4 (the previous lesson) says the fix is to make the rule LOAD, not to remember it.
# A commit-message scan is the mechanical form: it cannot be forgotten under load.
#
# USAGE
#   scripts/check-commit-msg.sh <.git/COMMIT_EDITMSG path>   # as a commit-msg hook
#   scripts/check-commit-msg.sh <(echo "msg")                # ad hoc
#
# Install (per clone, and it is cheap to re-install after any reclone):
#   ln -sf ../../scripts/check-commit-msg.sh .git/hooks/commit-msg
#
# EXIT: 1 if a tool is named (the commit is refused), 0 otherwise.

set -uo pipefail

MSG_FILE="${1:-}"
[ -n "$MSG_FILE" ] && [ -r "$MSG_FILE" ] || { echo "usage: $0 <commit-msg-file>" >&2; exit 2; }

# Names that must never appear. Deliberately blunt: a false positive costs one
# reworded sentence, a false negative ships a tool name into permanent history.
# Word-boundary `pi` because it is two letters and appears inside ordinary words.
NAMES='claude|hermes|\bpi\b|laguna|sonnet|anthropic|openai|gpt-|copilot|deepseek|gemini|ollama|AI assistant|AI agent|LLM|language model'

# The SUBJECT and BODY only -- not the `# Please enter the commit message` help
# text, and not commented lines, which git strips anyway. Scanning the whole file
# would flag git's own template.
LINE=$(grep -vE '^#' "$MSG_FILE" | grep -inE "$NAMES" || true)

if [ -n "$LINE" ]; then
  echo "" >&2
  echo "COMMIT REFUSED -- the message names an AI assistant or tool:" >&2
  echo "$LINE" | sed 's/^/    /' >&2
  echo "" >&2
  echo "HARD RULE 0c: commits must be indistinguishable from a human engineer's." >&2
  echo "This applies to the MESSAGE as well as the author name and email." >&2
  echo "" >&2
  echo "Describe the CHANGE, not who or what typed it:" >&2
  echo "  bad : 'Implemented by a delegated sonnet session; verified here.'" >&2
  echo "  good: 'The 15 tests are pasted unsummed; the control was re-run.'" >&2
  exit 1
fi

# The author must be neutral too -- the half that WAS guarded, kept here so the
# whole rule lives in one place a reader can find.
WHO=$(git config user.name 2>/dev/null || true)
MAIL=$(git config user.email 2>/dev/null || true)
if printf '%s %s' "$WHO" "$MAIL" | grep -qiE "$NAMES"; then
  echo "COMMIT REFUSED -- the configured author names a tool: '$WHO <$MAIL>'" >&2
  echo "Set a neutral identity: git config user.name '...'; git config user.email '...'" >&2
  exit 1
fi

exit 0
