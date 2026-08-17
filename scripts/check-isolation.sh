#!/usr/bin/env bash
# CI gate: enforce the crate-isolation invariants of docs/ARCHITECTURE.md /
# docs/ARCHITECTURE.md. Run from the repo root.
set -euo pipefail

status=0

# Invariant 1: reconverge-core must never depend on reconverge-dialect-oxide.
# The engine stays dialect-agnostic behind the SimtDialect trait.
core_tree="$(cargo tree -p reconverge-core)"
if grep -q 'reconverge-dialect-oxide' <<<"$core_tree"; then
  echo "ISOLATION VIOLATION: reconverge-core depends on reconverge-dialect-oxide." >&2
  echo "Invariant (docs/ARCHITECTURE.md): the engine is dialect-agnostic; dialects plug" >&2
  echo "in via the SimtDialect trait, never the other way around." >&2
  status=1
fi

# Invariant 2: reconverge-tui may depend on no workspace crate other than
# reconverge-artifacts. The TUI is a pure reader of versioned artifacts.
tui_tree="$(cargo tree -p reconverge-tui -e normal)"
if grep -qE 'reconverge-(core|dialect-oxide|driver|witness)|cargo-reconverge' <<<"$tui_tree"; then
  echo "ISOLATION VIOLATION: reconverge-tui depends on a workspace crate other" >&2
  echo "than reconverge-artifacts." >&2
  echo "Invariant (docs/ARCHITECTURE.md): the TUI is a pure reader of versioned" >&2
  echo "artifacts; if a view needs data the artifacts lack, extend the schema." >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "isolation invariants hold:"
  echo "  - reconverge-core is free of reconverge-dialect-oxide"
  echo "  - reconverge-tui touches only reconverge-artifacts"
fi
exit "$status"
