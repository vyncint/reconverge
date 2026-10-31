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
LAYOUT="$ROOT/scripts/upstream-layout.sh"
MODULES="warp thread barrier grid cooperative_groups cluster"

status=0
WORK=$(mktemp -d 2>/dev/null || mktemp -d -t reconverge-surface)
trap 'rm -rf "$WORK"' EXIT

# Build an upstream-shaped tree whose thread.rs holds $1. $3 is where the
# device crate sits: the pinned single-workspace layout by default, or
# upstream's monorepo layout (`cuda-oxide/crates/cuda-device`, #207).
tree_with() {
  local dir="$WORK/case-$2" crate=${3:-crates/cuda-device}
  rm -rf "$dir"
  mkdir -p "$dir/$crate/src"
  for m in $MODULES; do : > "$dir/$crate/src/$m.rs"; done
  printf '%s' "$1" > "$dir/$crate/src/thread.rs"
  echo "$dir"
}

check() {
  local want=$1 got=$2 label=$3
  if [ "$got" = "$want" ]; then
    printf '  ok    %-34s exit %s\n' "$label" "$got"
  else
    printf '  FAIL  %-34s exit %s, expected %s\n' "$label" "$got" "$want" >&2
    status=1
  fi
}

expect() {
  local want=$1 label=$2 source=$3 crate=${4:-} got=0
  local dir; dir=$(tree_with "$source" "$label" "$crate")
  bash "$GATE" "$dir" >/dev/null 2>&1 || got=$?
  check "$want" "$got" "$label"
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
  check 1 "$got" "present-include"
}
expect_present_include

# #207: upstream became a monorepo and the gate exited 2 on a path that was
# no longer there — which pins.yml then read as "no unknown functions". The
# new layout is scanned like the old one, so an unknown name in it is still
# caught and a clean one still passes.
MONO=cuda-oxide/crates/cuda-device
expect 0 "monorepo-clean"       'pub fn sync_threads() {}
' "$MONO"
expect 1 "monorepo-unknown"     'pub fn audit_new_barrier() {}
' "$MONO"
expect 2 "monorepo-missing-include" 'include!("generated/missing.rs");
' "$MONO"
# Neither layout is unreadable input, not a clean surface.
expect_no_crate() {
  local dir="$WORK/case-no-crate" got=0
  mkdir -p "$dir/crates/something-else/src"
  bash "$GATE" "$dir" >/dev/null 2>&1 || got=$?
  check 2 "$got" "no-device-crate"
}
expect_no_crate
# Both layouts at once: which one is upstream's would be decided by the
# order of a list, so the gate refuses rather than scanning either.
expect_both_layouts() {
  local dir; dir=$(tree_with 'pub fn audit_new_barrier() {}
' "both-layouts")
  mkdir -p "$dir/$MONO/src"
  local got=0
  bash "$GATE" "$dir" >/dev/null 2>&1 || got=$?
  check 2 "$got" "both-layouts"
}
expect_both_layouts

# The toolchain pins.yml compares is the one rustup applies to the device
# crate, not the checkout root's. At 2d47e43 the root file is the host
# workspace's stable `1.98.0` and the device toolchain sits beside the
# crate; comparing the root one reported drift that meant nothing (#207).
expect_toolchain() {
  local label=$1 crate=$2 want=$3 dir got
  dir=$(tree_with '' "$label" "$crate")
  printf '[toolchain]\nchannel = "host"\n' > "$dir/rust-toolchain.toml"
  case "$crate" in
    cuda-oxide/*) printf '[toolchain]\nchannel = "device"\n' > "$dir/cuda-oxide/rust-toolchain.toml" ;;
  esac
  got=$(bash "$LAYOUT" "$dir" toolchain 2>/dev/null) || got="exit $?"
  got=${got#"$(cd "$dir" && pwd -P)"/}
  if [ "$got" = "$want" ]; then
    printf '  ok    %-34s %s\n' "$label" "$got"
  else
    printf '  FAIL  %-34s %s, expected %s\n' "$label" "$got" "$want" >&2
    status=1
  fi
}
expect_toolchain "toolchain-pinned-layout" crates/cuda-device rust-toolchain.toml
expect_toolchain "toolchain-monorepo"      "$MONO"            cuda-oxide/rust-toolchain.toml

if [ "$status" -eq 0 ]; then
  echo "check-surface self-test: PASS — the gate fails on every input that should fail"
fi
exit "$status"
