#!/usr/bin/env bash
# Hardware session #2 probe runner — HUMAN-PROVISIONED HOSTS ONLY.
#
# This script requires a CUDA-capable GPU and the CUDA toolkit (for
# compute-sanitizer and upstream's own build); it exists for the
# cross-check session of docs/hardware/session-2.md and is NEVER invoked
# by CI (CONTRIBUTING.md: nothing in build/test/run may require the SDK —
# a rented GPU host running this by hand is the sanctioned path).
#
# Usage: scripts/hardware-mutation-probe.sh <upstream-checkout> <example> <output.tsv>
#
# Applies the SAME mutation operators the static corpus uses — via
# `conformance-extractor mutate` in single-file mode — to the full upstream
# example (host side included), then runs each mutant under
# `compute-sanitizer --tool synccheck`, recording what real hardware and
# NVIDIA's own dynamic checker observe per labeled bug class. Static
# verdicts and synccheck verdicts then get compared in a normal session;
# misses in either direction are data.
set -euo pipefail

UPSTREAM=${1:?usage: hardware-mutation-probe.sh <upstream-checkout> <example> <output.tsv>}
EXAMPLE=${2:?usage: hardware-mutation-probe.sh <upstream-checkout> <example> <output.tsv>}
OUT=${3:?usage: hardware-mutation-probe.sh <upstream-checkout> <example> <output.tsv>}
WATCHDOG_SECS=${WATCHDOG_SECS:-30}
ROOT=$(cd "$(dirname "$0")/.." && pwd)

if ! command -v nvidia-smi >/dev/null; then
  echo "hardware-mutation-probe: no GPU driver found — this script only runs" >&2
  echo "on the human-provisioned host (docs/hardware/session-2.md)." >&2
  exit 2
fi
if ! command -v compute-sanitizer >/dev/null; then
  echo "hardware-mutation-probe: compute-sanitizer not on PATH (CUDA toolkit)" >&2
  exit 2
fi

CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1)
MAIN="$UPSTREAM/crates/rustc-codegen-cuda/examples/$EXAMPLE/src/main.rs"
[ -f "$MAIN" ] || { echo "hardware-mutation-probe: no such example: $EXAMPLE" >&2; exit 2; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"; cp -f "$MAIN.orig" "$MAIN" 2>/dev/null || true' EXIT
cp "$MAIN" "$MAIN.orig"

# Same operators, same labels as the static corpus (single-file mode).
(cd "$ROOT/conformance/extractor" && cargo run -q -- mutate "$MAIN" "$WORK/mutants")

echo "hardware-mutation-probe: GPU cc $CC, example $EXAMPLE, watchdog ${WATCHDOG_SECS}s"
: > "$OUT"
while IFS=$'\t' read -r file class expected kernel line detail; do
  [ "${file#\#}" = "$file" ] || continue # header
  echo "hardware-mutation-probe: $class @ $kernel:$line"
  cp "$WORK/mutants/$file" "$MAIN"
  start=$(date +%s)
  set +e
  (cd "$UPSTREAM" && timeout "$WATCHDOG_SECS" \
    compute-sanitizer --tool synccheck --error-exitcode 42 \
    cargo oxide run "$EXAMPLE") > "$WORK/$file.log" 2>&1
  status=$?
  set -e
  elapsed=$(( $(date +%s) - start ))
  case "$status" in
    0)   outcome="pass" ;;
    42)  outcome="synccheck-flagged" ;;
    124) outcome="hang" ;;
    *)   outcome="error($status)" ;;
  esac
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$EXAMPLE" "$class" "$expected" "$kernel" "$CC" "$outcome" "$elapsed" >> "$OUT"
  cp "$MAIN.orig" "$MAIN"
done < "$WORK/mutants/labels.tsv"

echo "hardware-mutation-probe: results in $OUT (keep the logs in $WORK if needed)"
