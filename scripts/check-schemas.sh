#!/usr/bin/env bash
# CI gate: every artifact this project ships or emits validates against the
# schema it declares.
#
# `schemas/README.md` calls these documents "the contract between the
# analysis engine and every front-end", and until 0.5.0 nothing in
# `crates/`, `.github/`, `scripts/` or the justfile validated a single
# artifact against a single schema. The Rust round-trip tests go through
# serde, which by design tolerates anything additive and never sees a
# `const` — so `witness.v1` could pin `lanes` at 32 for four minor versions
# while the driver wrote 64, 96 and 128, and the artifacts that broke the
# published bound were exactly the gating ones.
#
# Two corpora, because they fail differently: `fixtures/` is what a
# front-end author validates against, and an end-to-end `check` is what the
# driver actually emits. The second is the layer that failed here.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/schema-check"

cargo build -q -p cargo-reconverge -p reconverge-driver

rm -rf "$WORK"
mkdir -p "$WORK/run"
cp -r "$ROOT/crates/cargo-reconverge/tests/lint-samples/." "$WORK/run/"

# Exit 1 is the gating findings these samples exist to produce.
( cd "$WORK/run" \
  && RECONVERGE_DRIVER="$ROOT/target/debug/reconverge-driver" \
     "$ROOT/target/debug/cargo-reconverge" reconverge check --strict \
       --message-format json > "$WORK/findings.jsonl" ) || true

python3 "$ROOT/scripts/validate-schema.py" \
  --schema-dir "$ROOT/schemas" \
  --fixtures "$ROOT/fixtures" \
  --emitted "$WORK/run/target/reconverge" \
  --jsonl "$WORK/findings.jsonl"
