#!/usr/bin/env bash
# check-dco.sh — DCO gate (docs/ARCHITECTURE.md).
#
# Every non-merge commit in the range must carry a Signed-off-by trailer
# matching the commit's author email. Merge commits are exempt (squash-only
# merging means main never has any).
#
# Usage: scripts/check-dco.sh <rev-range>
#   <rev-range> is anything `git rev-list` accepts: `base..head`, or a
#   single sha meaning that commit and its entire history (initial-push
#   case).
#
# Validation procedure (the project docs) — rerun whenever this script or
# check-no-ai-attribution.sh changes:
#   1. git checkout -b policy-validation
#   2. craft three commits:
#        a. one WITHOUT a sign-off             (git commit --no-verify)
#        b. one with an AI attribution trailer (git commit -s --no-verify)
#        c. one clean, signed-off commit       (git commit -s)
#   3. scripts/check-dco.sh main..policy-validation
#        → must FAIL, naming commit (a) only
#      scripts/check-no-ai-attribution.sh main..policy-validation
#        → must FAIL, naming commit (b) only
#   4. verify a clean range passes both scripts (e.g. HEAD~1..HEAD on main)
#   5. git checkout main && git branch -D policy-validation
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <rev-range>" >&2
  exit 2
fi

bad=0
while read -r sha; do
  author_email="$(git log -1 --format='%ae' "$sha")"
  body="$(git log -1 --format='%B' "$sha")"
  signoffs="$(grep -i '^signed-off-by:' <<<"$body" || true)"
  if [ -n "$signoffs" ] && grep -qF "<${author_email}>" <<<"$signoffs"; then
    echo "ok: $(git log -1 --format='%h %s' "$sha")"
  else
    echo "FAIL: commit $sha lacks a Signed-off-by trailer matching its author <${author_email}>" >&2
    bad=1
  fi
done < <(git rev-list --no-merges "$@")

if [ "$bad" -ne 0 ]; then
  echo >&2
  echo "DCO check failed. Sign off every commit with \`git commit -s\` (or" >&2
  echo "rebase with --signoff); the trailer must match the author identity." >&2
  exit 1
fi
echo "DCO check passed."
