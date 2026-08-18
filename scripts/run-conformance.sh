#!/usr/bin/env bash
# Conformance gate (the project docs): run the pinned upstream examples'
# kernels through reconverge and diff the gating findings against the
# reviewed baseline in conformance/EXPECTED.
#
# - Any gating finding NOT in EXPECTED is a false positive -> FAIL.
# - Any EXPECTED finding that stops appearing is a regression -> FAIL.
# - At least EXTRACTION_FLOOR example crates must survive extraction and
#   host-side compilation, so silent corpus shrinkage also fails.
#
# Why extraction instead of checking the examples verbatim: their host half
# (cuda-bindings) runs bindgen against cuda.h in every build — the build
# script hard-fails without a CUDA toolkit, and requiring the SDK anywhere
# is forbidden (SECURITY.md). conformance/extractor splices out the
# device side, which is the analysis surface anyway. See conformance/README.md.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/conformance"
CORPUS="$WORK/corpus"

# Extracted-and-compiling corpus floor; update deliberately when the pin
# moves or the extractor learns new item kinds. At the current pin,
# 210 examples -> 186 extract (24 lack an inline #[cuda_module] mod)
# -> 143 compile device-side-only (43 pruned: generic-kernel examples are
# host-coupled by upstream design, and a few kernels reach cuda_core).
EXTRACTION_FLOOR=143

read -r SHA REPO < <(grep -v '^#' "$ROOT/conformance/PIN" | head -1)
if [ -z "${SHA:-}" ] || [ "$SHA" = "UNSET" ]; then
  echo "conformance: conformance/PIN is not set" >&2
  exit 2
fi

# 1. Materialize the pinned upstream checkout.
CHECKOUT="$WORK/upstream"
if [ ! -d "$CHECKOUT/.git" ] || [ "$(git -C "$CHECKOUT" rev-parse HEAD 2>/dev/null)" != "$SHA" ]; then
  rm -rf "$CHECKOUT"
  mkdir -p "$CHECKOUT"
  git -C "$CHECKOUT" init -q
  git -C "$CHECKOUT" fetch -q --depth 1 "https://github.com/$REPO" "$SHA"
  git -C "$CHECKOUT" checkout -q FETCH_HEAD
fi
echo "conformance: upstream $REPO @ $SHA"

# 2. Build the tool and the extractor; extract the corpus.
cargo build -q -p reconverge-driver -p cargo-reconverge
(cd "$ROOT/conformance/extractor" && cargo test -q --locked >/dev/null && cargo run -q --locked -- extract "$CHECKOUT" "$CORPUS")

# 3. Prune corpus members whose kernels reach host items the extractor
#    deliberately does not carry over (they fail host-side compilation).
cd "$CORPUS"
FAILING=$(cargo check --workspace --keep-going --message-format=json 2>/dev/null \
  | jq -r 'select(.reason == "compiler-message" and .message.level == "error") | .target.name' \
  | sort -u) || true
PRUNED=0
if [ -n "$FAILING" ]; then
  while IFS= read -r target; do
    # target names are crate names: conformance_<example, underscored>,
    # which matches the member directory name exactly.
    member=${target#conformance_}
    sed -i "\#crates/${member}\",#d" Cargo.toml
    echo "${member}	pruned	does not compile device-side-only" >> extraction-report.tsv
    PRUNED=$((PRUNED + 1))
  done <<< "$FAILING"
fi

ANALYZED=$(grep -c 'crates/' Cargo.toml)
echo "conformance: analyzing $ANALYZED example kernel crates ($PRUNED pruned)"
if [ "$ANALYZED" -lt "$EXTRACTION_FLOOR" ]; then
  echo "conformance: FAIL — corpus shrank below the floor ($ANALYZED < $EXTRACTION_FLOOR)" >&2
  echo "conformance: see $CORPUS/extraction-report.tsv" >&2
  exit 1
fi

# 4. Run the tool over the whole corpus and compare against the baseline.
set +e
JSON=$(RECONVERGE_DRIVER="$ROOT/target/debug/reconverge-driver" \
  "$ROOT/target/debug/cargo-reconverge" reconverge check --message-format json)
STATUS=$?
set -e
if [ "$STATUS" -ge 2 ]; then
  echo "conformance: FAIL — cargo reconverge check errored (exit $STATUS)" >&2
  exit 2
fi
# Persist the unmutated baseline: the mutation-corpus runner scores
# injected bugs against exactly this run.
printf '%s\n' "$JSON" > "$WORK/findings.jsonl"

ACTUAL=$(printf '%s\n' "$JSON" | jq -r '
  . as $doc
  | .findings[]
  | select(.confidence == "deny" or .confidence == "confirmed")
  | [$doc.crate, .code, (.kernel // "-"), (.confidence | tostring)]
  | @tsv' | sort)
WARNINGS=$(printf '%s\n' "$JSON" | jq -r '
  [.findings[] | select(.confidence == "warning")] | length' | paste -sd+ - | bc)
EXPECTED=$(grep -v '^#' "$ROOT/conformance/EXPECTED" | sed '/^$/d' | sort)

if ! diff <(printf '%s\n' "$EXPECTED") <(printf '%s\n' "$ACTUAL") > "$WORK/findings.diff"; then
  echo "conformance: FAIL — findings differ from the reviewed baseline:" >&2
  echo "  ('>' lines are unreviewed findings — false positives unless proven" >&2
  echo "   otherwise; '<' lines are expected findings that disappeared)" >&2
  cat "$WORK/findings.diff" >&2
  exit 1
fi

# 5. Chain ratchet (T1 gate, extended to RC002 in M3): every divergence
#    finding must show a complete source→sink chain — at least a branch
#    step plus derivation steps, terminating at a named divergence source
#    (witness/atomic) or a declared degrade.
BROKEN=$(printf '%s\n' "$JSON" | jq -r '
  . as $doc | .findings[] | select(.code == "RC001" or .code == "RC002")
  | select((.provenance | length) < 2
           or ((.provenance | last | .what)
               | test("witness|atomic|opaque|irreducible|lane") | not))
  | [$doc.crate, .code, (.kernel // "-")] | @tsv')
if [ -n "$BROKEN" ]; then
  echo "conformance: FAIL — divergence findings with incomplete provenance chains:" >&2
  printf '%s\n' "$BROKEN" >&2
  exit 1
fi
CHAINS=$(printf '%s\n' "$JSON" | jq -r '
  [.findings[] | select(.code == "RC001" or .code == "RC002")] | length' | paste -sd+ - | bc)

echo "conformance: PASS — gating findings match the baseline exactly"
echo "conformance: $WARNINGS warning-confidence finding(s) (informational)"
echo "conformance: $CHAINS RC001/RC002 chain(s) complete (source-terminated provenance)"
