#!/usr/bin/env bash
# The vendored termlens skill must name the version this repository depends on.
#
# `.claude/skills/termlens/SKILL.md` is a copy of the file termlens ships for
# coding agents, and AGENTS.md makes it normative: "PTY tests follow the
# termlens skill, vendored at …". It is refreshed by hand, and the failure
# mode is silent — the dependency gets bumped, the copy does not, and every
# agent working here is then handed guidance for a version that is no longer
# present: wrong signatures, absent APIs, advice that was true one release
# ago. The most recent commit before this gate existed was literally
# "docs: refresh the termlens skill copy for the 0.9 drag signature (#114)",
# so the lockstep is real and was maintained only by memory.
#
# Nothing can diff the copy against upstream: the published crate does not
# ship the skill, so there is no registry copy to compare with. What *is*
# checkable is that the two versions agree, which is exactly the drift that
# happens.
#
# Compares major.minor only. A termlens patch release does not rewrite the
# skill, and demanding a re-copy for every one of them would make this noise.
#
# Both manifests are checked, and against each other. termlens is deliberately
# *not* in `[workspace.dependencies]` (see the policy comment in
# crates/reconverge-tui/Cargo.toml), so a bump is two independent edits and
# missing one resolves two termlens versions — which deny.toml only downgrades
# to `multiple-versions = "warn"`, so nothing else in CI would say a word.
#
# Usage: check-skill-version.sh [SKILL.md] [Cargo.toml…]
set -euo pipefail

# The singular or the plural of a word, chosen by a count — the shell half of
# `reconverge_artifacts::plural`, copied from scripts/run-conformance.sh.
# scripts/check-plurals.sh is the gate that keeps `manifest(s)` out of a line
# that lands in a CI log.
plural() { if [ "$1" = "1" ]; then printf '%s' "$2"; else printf '%s' "$3"; fi; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="${1:-$root/.claude/skills/termlens/SKILL.md}"
shift || true
if [ "$#" -gt 0 ]; then
  manifests=("$@")
else
  manifests=(
    "$root/crates/reconverge-tui/Cargo.toml"
    "$root/crates/cargo-reconverge/Cargo.toml"
  )
fi

[ -f "$skill" ] || { echo "::error::$skill does not exist"; exit 1; }

# "Written against **termlens 0.10.1**." -> 0.10.1
skill_version="$(sed -n 's/.*Written against \*\*termlens \([0-9][0-9.]*\)\*\*.*/\1/p' "$skill" | head -1)"
[ -n "$skill_version" ] || {
  echo "::error::$skill has no 'Written against **termlens X.Y.Z**' line to check"
  exit 1
}
skill_minor="$(echo "$skill_version" | cut -d. -f1,2)"

# termlens = { version = "0.10", default-features = false }  ->  0.10
first_version=""
first_manifest=""
for manifest in "${manifests[@]}"; do
  [ -f "$manifest" ] || { echo "::error::$manifest does not exist"; exit 1; }
  dep_version="$(sed -n 's/^termlens = .*version = "\([0-9][0-9.]*\)".*/\1/p' "$manifest" | head -1)"
  [ -n "$dep_version" ] || {
    echo "::error::no termlens dependency with a version found in $manifest"
    exit 1
  }
  if [ -z "$first_version" ]; then
    first_version="$dep_version"
    first_manifest="$manifest"
  elif [ "$dep_version" != "$first_version" ]; then
    echo "::error::$first_manifest depends on termlens ${first_version} but $manifest depends on ${dep_version}."
    echo "::error::The two must move together, or the build resolves two termlens versions and the goldens are rendered by different emulators."
    exit 1
  fi

  dep_minor="$(echo "$dep_version" | cut -d. -f1,2)"
  if [ "$skill_minor" != "$dep_minor" ]; then
    echo "::error::the vendored termlens skill is written against ${skill_version} but $manifest depends on ${dep_version}."
    echo "::error::Refresh it: cp ../termlens/skills/termlens/SKILL.md ${skill}"
    exit 1
  fi
done

echo "the vendored termlens skill (${skill_version}) matches the dependency (${first_version}) in ${#manifests[@]} $(plural "${#manifests[@]}" manifest manifests)"
