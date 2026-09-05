#!/usr/bin/env bash
# Re-record the witness fixtures from a real `cargo reconverge check`, and
# fail when the committed copies differ.
#
# `fixtures/` is what `schemas/README.md` calls the API tests, and until
# 0.5.0 every one of them was hand-written: the three witness documents
# showed MIR statements no released driver has ever emitted, and all three
# were stamped `tool.version 0.0.0`, which is the tell. `round_trip_fixtures`
# parses a fixture, reserializes it and compares it to itself, so nothing in
# the repository ever compared an artifact the driver *wrote* against an
# expected shape — which is how the collective's lane strip could contradict
# its own mask in the shipping artifact while the golden frame stayed green.
#
# `--check` diffs instead of writing, which is what CI runs.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/fixture-recording"
CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

# Which driver artifact becomes which fixture. Kept explicit rather than
# globbed: a fixture is a document somebody chose, and a renamed kernel
# should fail here rather than silently re-point a fixture.
RECORDINGS="
rc001-divergent-barrier|witness-lint_samples-lib-rc001_divergent_barrier-RC001-0.json
rc002-partial-mask|witness-lint_samples-lib-rc002_divergent_collective-RC002-5.json
rc001-multiwarp-barrier|witness-lint_samples-lib-rc001_multiwarp_barrier-RC001-2.json
"

cargo build -q -p cargo-reconverge -p reconverge-driver

rm -rf "$WORK"
mkdir -p "$WORK"
cp -r "$ROOT/crates/cargo-reconverge/tests/lint-samples/." "$WORK/"

# `check` exits 1 on the gating findings these samples exist to produce.
( cd "$WORK" \
  && RECONVERGE_DRIVER="$ROOT/target/debug/reconverge-driver" \
     "$ROOT/target/debug/cargo-reconverge" reconverge check --strict >/dev/null ) || true

status=0
while IFS='|' read -r fixture artifact; do
  [ -z "$fixture" ] && continue
  src="$WORK/target/reconverge/$artifact"
  dst="$ROOT/fixtures/witness/$fixture.json"
  if [ ! -f "$src" ]; then
    echo "fixtures: FAIL — the run produced no $artifact" >&2
    echo "  (a renamed or reordered lint-sample kernel: fix the mapping in $0)" >&2
    status=1
    continue
  fi
  # The span's file is the kernel crate's own path, which is what a user's
  # own artifact carries too, so it is recorded verbatim.
  if [ "$CHECK" -eq 1 ]; then
    if ! diff -u "$dst" "$src" > "$WORK/$fixture.diff"; then
      echo "fixtures: FAIL — $fixture.json differs from what the driver writes:" >&2
      cat "$WORK/$fixture.diff" >&2
      echo "  (re-record with ./scripts/record-fixtures.sh and review the diff)" >&2
      status=1
    fi
  else
    cp "$src" "$dst"
    echo "recorded fixtures/witness/$fixture.json"
  fi
done <<EOF
$RECORDINGS
EOF

if [ "$CHECK" -eq 1 ] && [ "$status" -eq 0 ]; then
  echo "every recorded witness fixture matches what the driver writes"
fi
exit "$status"
