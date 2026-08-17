#!/usr/bin/env bash
# Hardware session #1 probe runner — HUMAN-PROVISIONED HOSTS ONLY.
#
# This script requires a CUDA-capable GPU and the CUDA toolkit; it exists
# for the calibration session of docs/hardware/session-1.md and is NEVER
# invoked by CI (CONTRIBUTING.md: nothing in build/test/run may require the
# SDK — a rented GPU host running this by hand is the sanctioned path).
#
# Usage: scripts/hardware-probe.sh <upstream-checkout> <output.tsv>
#
# For each probe kernel, builds a minimal host runner with upstream's
# tooling, launches it under `timeout`, and records the outcome.
set -euo pipefail

UPSTREAM=${1:?usage: hardware-probe.sh <upstream-checkout> <output.tsv>}
OUT=${2:?usage: hardware-probe.sh <upstream-checkout> <output.tsv>}
WATCHDOG_SECS=${WATCHDOG_SECS:-20}

if ! command -v nvidia-smi >/dev/null; then
  echo "hardware-probe: no GPU driver found — this script only runs on the" >&2
  echo "human-provisioned calibration host (docs/hardware/session-1.md)." >&2
  exit 2
fi

CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1)
echo "hardware-probe: GPU compute capability $CC, watchdog ${WATCHDOG_SECS}s"

# Probe kernels live as upstream-style examples so `cargo oxide run` can
# launch them; the mutation corpus plugs in here.
PROBES=(rc001_divergent_barrier rc002_divergent_collective)

: > "$OUT"
for probe in "${PROBES[@]}"; do
  echo "hardware-probe: running $probe"
  start=$(date +%s)
  set +e
  (cd "$UPSTREAM" && timeout "$WATCHDOG_SECS" cargo oxide run "$probe") \
    > "$probe.log" 2>&1
  status=$?
  set -e
  elapsed=$(( $(date +%s) - start ))
  case "$status" in
    0) outcome="pass" ;;
    124) outcome="hang" ;;
    *) outcome="error($status)" ;;
  esac
  printf '%s\t%s\t%s\t%s\n' "$probe" "$CC" "$outcome" "$elapsed" >> "$OUT"
done

echo "hardware-probe: results in $OUT"