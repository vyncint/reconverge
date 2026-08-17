#!/usr/bin/env bash
# check-no-ai-attribution.sh — attribution-hygiene gate (CONTRIBUTING.md).
#
# No AI attribution anywhere: scans every commit in the range (merges
# included) — the full message for attribution trailers/markers, and the
# author/committer identities for bot or vendor identities.
#
# Usage: scripts/check-no-ai-attribution.sh <rev-range>
#   <rev-range> is anything `git rev-list` accepts: `base..head`, or a
#   single sha meaning that commit and its entire history (initial-push
#   case).
#
# Validation procedure (the project docs): see the header of
# scripts/check-dco.sh — the two scripts are validated together on a
# scratch branch with crafted commits.
set -euo pipefail

POLICY='AI assistance is welcome; AI attribution is not. Remove the trailer and recommit — you are the author of record.'

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <rev-range>" >&2
  exit 2
fi

bad=0
flag() {
  echo "FAIL: commit $1: $2" >&2
  bad=$((bad + 1))
}

while read -r sha; do
  body="$(git log -1 --format='%B' "$sha")"
  an="$(git log -1 --format='%an' "$sha")"
  ae="$(git log -1 --format='%ae' "$sha")"
  cn="$(git log -1 --format='%cn' "$sha")"
  ce="$(git log -1 --format='%ce' "$sha")"
  before=$bad

  if grep -qiE 'co-authored-by:[[:space:]]*.*\b(claude|anthropic|copilot|chatgpt|gpt|openai|cursor|devin|aider|codex|gemini|windsurf|jetbrains ai|amazon q|sweep|bot)\b' <<<"$body"; then
    flag "$sha" "AI attribution trailer in the message"
  fi
  if grep -qiE 'generated (with|by)\b' <<<"$body"; then
    flag "$sha" "'generated with/by' marker in the message"
  fi
  if grep -qF '🤖' <<<"$body"; then
    flag "$sha" "robot-emoji watermark in the message"
  fi
  if grep -qiE 'noreply\.anthropic\.com' <<<"$body"; then
    flag "$sha" "vendor noreply address in the message"
  fi

  if grep -qiE '\[bot\]@users\.noreply\.github\.com|@noreply\.anthropic\.com|actions@github\.com' <<<"$ae"; then
    flag "$sha" "author email is a bot/vendor identity: $ae"
  fi
  if grep -qiE '\[bot\]@users\.noreply\.github\.com|@noreply\.anthropic\.com|actions@github\.com' <<<"$ce"; then
    flag "$sha" "committer email is a bot/vendor identity: $ce"
  fi
  if grep -qiE '\b(claude|copilot|devin|aider|codex|gemini)\b' <<<"$an"; then
    flag "$sha" "author name looks like an AI agent: $an"
  fi
  if grep -qiE '\b(claude|copilot|devin|aider|codex|gemini)\b' <<<"$cn"; then
    flag "$sha" "committer name looks like an AI agent: $cn"
  fi

  if [ "$bad" -eq "$before" ]; then
    echo "ok: $(git log -1 --format='%h %s' "$sha")"
  fi
done < <(git rev-list "$@")

if [ "$bad" -ne 0 ]; then
  echo >&2
  echo "$POLICY" >&2
  exit 1
fi
echo "attribution check passed."
