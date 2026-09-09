#!/usr/bin/env bash
# The known-unknown gate: every module-level public function in the
# synchronization- and collective-bearing modules of upstream `cuda_device`
# must be either classified by the dialect (named in simt.rs) or listed in
# conformance/SURFACE_ALLOW with a reason.
#
# Why this exists: `classify_call` maps an unrecognized `cuda_device::` call
# to `CallKind::Other`, which is counted as coverage and never a finding. For
# a helper that is the right default. For a barrier or a masked collective it
# is a silent false negative — a divergent call to a primitive we have not
# named is not RC001/RC002, it is a note. Upstream adds such primitives
# (eight `redux_sync_*_f32` in one release), so "unknown" has to be a
# decision someone wrote down, not the state a new name lands in.
#
# Usage: scripts/check-surface.sh <upstream-checkout>
#   exit 0  every surface function is classified or allowlisted
#   exit 1  unknown names (printed one per line) — classify them in
#           crates/reconverge-dialect-oxide/src/simt.rs or add them to
#           conformance/SURFACE_ALLOW with a reason
#   exit 2  usage, missing checkout, or an `include!`d source that is not on
#           disk — unreadable input is never reported as a clean surface
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
UPSTREAM=${1:?usage: check-surface.sh <upstream-checkout>}
DEVICE="$UPSTREAM/crates/cuda-device/src"
[ -d "$DEVICE" ] || { echo "check-surface: $DEVICE is not a cuda-oxide checkout" >&2; exit 2; }
SIMT="$ROOT/crates/reconverge-dialect-oxide/src/simt.rs"
ALLOW="$ROOT/conformance/SURFACE_ALLOW"

# The modules whose functions are synchronization or collectives (or the
# markers and index reads the dialect must recognize). Adding a module here
# widens the gate; removing one is a decision for conformance/README.md.
MODULES=(warp thread barrier grid cooperative_groups cluster)

unknown=0
missing=0
for m in "${MODULES[@]}"; do
  f="$DEVICE/$m.rs"
  [ -f "$f" ] || { echo "check-surface: note — upstream has no $m.rs at this pin" >&2; continue; }
  # Module-level `pub fn` only (no indentation): methods are reached through
  # their receiver type and classified by path fragment, not by bare name.
  # A module also `include!`s generated files (`warp.rs` pulls in
  # `generated/warp_sreg.rs`, `cluster.rs` its barrier and memory files),
  # whose functions are called as `warp::warpid()` exactly like the ones
  # written in the module — so those files are scanned as part of it.
  sources=("$f")
  while read -r inc; do
    [ -n "$inc" ] || continue
    # A named include that is not on disk means this module's surface was
    # never enumerated. Saying PASS then is the exact failure this gate
    # exists to prevent, one level up: `cat ... 2>/dev/null` used to hide
    # the missing file, the scan silently covered fewer functions than it
    # claimed, and the script still printed PASS (#134). Unreadable input
    # is a hard error, never an empty result set.
    if [ ! -f "$DEVICE/$inc" ]; then
      echo "check-surface: $m.rs includes \"$inc\", which is not at $DEVICE/$inc" >&2
      missing=$((missing + 1))
      continue
    fi
    sources+=("$DEVICE/$inc")
  done < <(grep -oE 'include!\("[^"]+"\)' "$f" | sed -E 's/include!\("([^"]+)"\)/\1/')
  while read -r name; do
    [ -n "$name" ] || continue
    if grep -q "\"$name\"" "$SIMT"; then continue; fi
    if grep -qE "^$name[[:space:]]" "$ALLOW"; then continue; fi
    echo "unknown: cuda_device::$m::$name"
    unknown=$((unknown + 1))
  done < <(cat "${sources[@]}" | grep -oE '^pub (unsafe )?fn [A-Za-z_0-9]+' | awk '{print $NF}' | sort -u)
done

if [ "$missing" -gt 0 ]; then
  echo "check-surface: FAIL — $missing included source file(s) could not be read; the surface was not enumerated" >&2
  exit 2
fi
if [ "$unknown" -gt 0 ]; then
  echo "check-surface: FAIL — $unknown upstream surface function(s) neither classified nor allowlisted" >&2
  exit 1
fi
echo "check-surface: PASS — every surface function is classified or allowlisted"
