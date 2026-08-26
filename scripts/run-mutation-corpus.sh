#!/usr/bin/env bash
# Mutation-corpus gate (the project docs): mechanically inject the labeled bug
# classes into the extracted conformance corpus, run the tool over every
# mutant, and score precision/recall against the labels.
#
# - Precision at default confidence must be 1.0 (any unattributed gating
#   finding on a mutant is a false positive) -> FAIL.
# - Recall is REPORTED, per class, at default and --strict confidence; the
#   generated report must match the committed conformance/MUTATION.md
#   exactly, so any movement is a deliberate, reviewed change -> diff FAIL.
# - Mutants that do not compile are pruned and counted, never silently
#   dropped (a swap into slice-only API is expected to fail sometimes).
#
# Reuses the conformance run's extracted corpus and unmutated baseline
# (target/conformance): run-conformance.sh is invoked first if either is
# missing, so this script is self-sufficient locally and cheap in CI where
# conformance has just run.
set -euo pipefail

# The singular or the plural of a word, chosen by a count — the shell half of
# `reconverge_artifacts::plural`. These lines land in CI logs and in issues,
# and "1 finding(s)" reads there exactly as badly as it did in the tool.
plural() { if [ "$1" = "1" ]; then printf '%s' "$2"; else printf '%s' "$3"; fi; }

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/conformance"
MWORK="$ROOT/target/mutation"
MUTANTS="$MWORK/mutants"

# 1. Ensure the corpus and the unmutated baseline exist.
if [ ! -f "$WORK/findings.jsonl" ] || [ ! -d "$WORK/corpus" ]; then
  "$ROOT/scripts/run-conformance.sh"
fi

# 2. Generate the mutants workspace + labels.
cargo build -q -p reconverge-driver -p cargo-reconverge
mkdir -p "$MWORK"
(cd "$ROOT/conformance/extractor" && cargo run -q --locked -- mutate "$WORK/corpus" "$MUTANTS")

# 3. Prune mutants that do not compile (counted; e.g. a DisjointSlice ->
#    &mut [T] swap whose body needs DisjointSlice-only API).
cd "$MUTANTS"
FAILING=$(cargo check --workspace --keep-going --message-format=json 2>/dev/null \
  | jq -r 'select(.reason == "compiler-message" and .message.level == "error") | .target.name' \
  | sort -u) || true
PRUNED=0
if [ -n "$FAILING" ]; then
  while IFS= read -r target; do
    # Mutant crate names equal their member directory names exactly.
    sed -i "\#crates/${target}\",#d" Cargo.toml
    PRUNED=$((PRUNED + 1))
  done <<< "$FAILING"
fi
EMITTED=$(grep -vc '^#' labels.tsv)
ANALYZED=$(grep -c 'crates/' Cargo.toml)
echo "mutation: $EMITTED $(plural "$EMITTED" mutant mutants) emitted, $ANALYZED compile, $PRUNED pruned"

# 4. Run the tool over every compiling mutant.
set +e
JSON=$(RECONVERGE_DRIVER="$ROOT/target/debug/reconverge-driver" \
  "$ROOT/target/debug/cargo-reconverge" reconverge check --message-format json)
STATUS=$?
set -e
if [ "$STATUS" -ge 2 ]; then
  echo "mutation: FAIL — cargo reconverge check errored (exit $STATUS)" >&2
  exit 2
fi
printf '%s\n' "$JSON" > "$MWORK/findings.jsonl"

# 5. Score: precision must be 1.0, and the published table must match the
#    committed copy exactly.
(cd "$ROOT/conformance/extractor" && cargo run -q --locked -- score \
  "$MUTANTS" "$WORK/findings.jsonl" "$MWORK/findings.jsonl" "$MWORK/MUTATION.md")

if ! diff -u "$ROOT/conformance/MUTATION.md" "$MWORK/MUTATION.md" > "$MWORK/mutation.diff"; then
  echo "mutation: FAIL — results differ from the committed conformance/MUTATION.md:" >&2
  echo "  (review the diff; if the movement is intended, copy the generated" >&2
  echo "   file over the committed one in the same change)" >&2
  cat "$MWORK/mutation.diff" >&2
  exit 1
fi

echo "mutation: PASS — precision 1.0 at default confidence; published table unchanged"
