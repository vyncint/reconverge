#!/usr/bin/env bash
# Where, inside an upstream cuda-oxide checkout, the device crate and the
# toolchain that governs it live.
#
# Upstream became a monorepo (2d47e43, 2026-10-30): the device crate moved
# from `crates/cuda-device` to `cuda-oxide/crates/cuda-device`, and the
# root `rust-toolchain.toml` became the *host* workspace's stable pin while
# the device toolchain (nightly-class, with rustc-dev) moved down beside the
# crate. pins.yml kept comparing the root file — `1.98.0` against our
# nightly, a difference that means nothing — and check-surface.sh exited 2
# on a path that was no longer there, which the workflow then read as an
# empty list of unknown functions (#207). Both answers come from here so the
# two cannot disagree about which layout they are looking at.
#
# Usage: scripts/upstream-layout.sh <upstream-checkout> device|toolchain
#   device     prints the device crate's directory
#   toolchain  prints the toolchain file rustup would apply to that crate:
#              the nearest `rust-toolchain.toml` / `rust-toolchain` walking
#              up from it, stopping at the checkout root
#   exit 2     usage, no device crate in either layout, both layouts at once,
#              or no toolchain file — never a guess
set -euo pipefail
UPSTREAM=${1:?usage: upstream-layout.sh <upstream-checkout> device|toolchain}
WHAT=${2:?usage: upstream-layout.sh <upstream-checkout> device|toolchain}
[ -d "$UPSTREAM" ] || { echo "upstream-layout: $UPSTREAM is not a directory" >&2; exit 2; }
ROOT=$(cd "$UPSTREAM" && pwd -P)

# The single-workspace layout up to 2026-10, then the monorepo one. Plain
# strings rather than an array: an empty array under `set -u` is an unbound
# variable on the bash 3.2 macOS ships.
found=0
DEVICE=""
for rel in crates/cuda-device cuda-oxide/crates/cuda-device; do
  if [ -d "$ROOT/$rel/src" ]; then
    found=$((found + 1))
    DEVICE="$ROOT/$rel"
  fi
done
case $found in
  1) ;;
  0)
    echo "upstream-layout: no crates/cuda-device or cuda-oxide/crates/cuda-device under $UPSTREAM — not a cuda-oxide checkout, or a layout this script does not know yet" >&2
    exit 2 ;;
  *)
    # Picking one would decide which surface and which toolchain are
    # "upstream's" by the order of a list.
    echo "upstream-layout: both crates/cuda-device and cuda-oxide/crates/cuda-device exist under $UPSTREAM; refusing to choose" >&2
    exit 2 ;;
esac

case "$WHAT" in
  device)
    echo "$DEVICE" ;;
  toolchain)
    # rustup reads `rust-toolchain` before `rust-toolchain.toml` in the same
    # directory, so the legacy name is checked first.
    dir=$DEVICE
    while :; do
      for name in rust-toolchain rust-toolchain.toml; do
        if [ -f "$dir/$name" ]; then echo "$dir/$name"; exit 0; fi
      done
      [ "$dir" = "$ROOT" ] && break
      dir=$(dirname "$dir")
    done
    echo "upstream-layout: no rust-toolchain file between ${DEVICE#"$ROOT"/} and the root of $UPSTREAM" >&2
    exit 2 ;;
  *)
    echo "upstream-layout: unknown query '$WHAT' (expected device or toolchain)" >&2
    exit 2 ;;
esac
