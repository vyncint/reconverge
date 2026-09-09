#!/usr/bin/env bash
# Does the known-unknown gate actually fail?
#
# `check-surface.sh` reported PASS when a module's `include!`d source was
# missing (#134): `cat ... 2>/dev/null` hid the unreadable file, the scan
# covered fewer functions than it claimed, and the script still said every
# surface function was classified. A gate whose failure path is never
# exercised is a gate that reports.
#
# So this drives it over synthetic upstream trees and asserts the exit code
# for each outcome. No network, no compiler, no cuda-oxide checkout.
#
# Portability: macOS ships bash 3.2 — no `declare -A`, no `local -n`, no
# GNU-only `mktemp -d --tmpdir` (scripts/check-pins.sh, the hard way).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
GATE="$ROOT/scripts/check-surface.sh"
MODULES="warp thread barrier grid cooperative_groups cluster"

status=0
WORK=$(mktemp -d 2>/dev/null || mktemp -d -t reconverge-surface)
trap 'rm -rf "$WORK"' EXIT

# Build an upstream-shaped tree whose thread.rs holds $1.
tree_with() {
  local dir="$WORK/case-$2"
  rm -rf "$dir"
  mkdir -p "$dir/crates/cuda-device/src"
  for m in $MODULES; do : > "$dir/crates/cuda-device/src/$m.rs"; done
  printf '%s' "$1" > "$dir/crates/cuda-device/src/thread.rs"
  echo "$dir"
}

expect() {
  local want=$1 label=$2 source=$3 got=0
  local dir; dir=$(tree_with "$source" "$label")
  bash "$GATE" "$dir" >/dev/null 2>&1 || got=$?
  if [ "$got" = "$want" ]; then
    printf '  ok    %-34s exit %s\n' "$label" "$got"
  else
    printf '  FAIL  %-34s exit %s, expected %s\n' "$label" "$got" "$want" >&2
    status=1
  fi
}

echo "check-surface self-test:"
# A tree with nothing unknown in it passes.
expect 0 "empty-surface"        ''
# A name the dialect does not classify and the allowlist does not name.
expect 1 "unknown-primitive"    'pub fn audit_new_barrier() {}
'
# An allowlisted name is not unknown.
expect 0 "allowlisted-primitive" 'pub fn mbarrier_wait() {}
'
# A classified name is not unknown.
expect 0 "classified-primitive" 'pub fn sync_threads() {}
'
# #134: a named include that is not on disk means the surface was never
# enumerated. Unreadable input must never read as a clean surface.
expect 2 "missing-include"      'include!("generated/missing.rs");
'
# An include that *is* on disk is scanned as part of the module, so an
# unknown name inside it is still caught.
expect_present_include() {
  local dir; dir=$(tree_with 'include!("generated/present.rs");
' "present-include")
  mkdir -p "$dir/crates/cuda-device/src/generated"
  printf 'pub fn audit_generated_barrier() {}\n' \
    > "$dir/crates/cuda-device/src/generated/present.rs"
  local got=0
  bash "$GATE" "$dir" >/dev/null 2>&1 || got=$?
  if [ "$got" = "1" ]; then
    printf '  ok    %-34s exit %s\n' "present-include" "$got"
  else
    printf '  FAIL  %-34s exit %s, expected 1\n' "present-include" "$got" >&2
    status=1
  fi
}
expect_present_include

if [ "$status" -eq 0 ]; then
  echo "check-surface self-test: PASS — the gate fails on every input that should fail"
fi
exit "$status"
