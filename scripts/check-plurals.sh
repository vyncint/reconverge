#!/usr/bin/env bash
# CI gate: no count in this project's output may disagree with its own noun.
#
# `reconverge_artifacts::plural` is the rule; this is the check that a new
# `finding(s)` cannot creep back past it. No compiler catches this class —
# the strings are correct Rust and correct shell — and the last sweep (#62)
# closed without a gate, which is why `466 gating finding(s)` was still the
# headline of the published precision table two releases later.
#
# `conformance/` is swept too: it is outside every workspace-scoped gate,
# and it is what writes that table.
#
# What is flagged is a *count* followed by a parenthesized plural: a literal
# number or a format placeholder, then the noun. Deliberately generic prose —
# "the element(s) its own index selects" — names no count and is left alone,
# which is the distinction the issue asked the gate to keep.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# A backtick before the count exempts the span: prose that quotes the bad
# form in order to name it — the rule's own doc comment, this gate's own
# rationale — is documentation, not output.
COUNT_THEN_PLURAL='(^|[^`])(\{[^}]*\}|[0-9]+)( +[A-Za-z_-]+)* +[A-Za-z_]+\((s|es|ies)\)'

HITS=$(grep -rnE "$COUNT_THEN_PLURAL" \
  --include='*.rs' --include='*.sh' --include='*.md' --include='*.yml' \
  --exclude-dir=target \
  crates conformance scripts action docs README.md 2>/dev/null \
  | grep -v '^scripts/check-plurals.sh:' \
  || true)

if [ -n "$HITS" ]; then
  echo "PLURAL VIOLATION: a count is printed with a parenthesized plural." >&2
  echo "Use reconverge_artifacts::plural (or the shell plural() the gate" >&2
  echo "scripts define) so the noun follows the number:" >&2
  printf '%s\n' "$HITS" >&2
  exit 1
fi

echo "no count disagrees with its noun"
