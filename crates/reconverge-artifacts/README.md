# reconverge-artifacts

Serde bindings for [reconverge](https://github.com/vyncint/reconverge)'s versioned artifact schemas — the
contract between the analysis engine and every front-end.

| Schema | Contents |
|---|---|
| `findings.v1` | diagnostics: code, span, confidence tier, notes, provenance chain, suggested fix |
| `unimap.v1` | per-function uniformity labels, provenance edges, divergent-control bits, coverage |
| `witness.v1` | a 32-lane replay: launch config, delta-encoded lane timeline, barrier arrivals, masks, verdict |
| `baseline.v1` | reviewed suppressions — `(crate, kernel, code)` plus the written reason each was accepted for |

Schemas are semver'd independently of the crates and additive-only within a
major version; the [JSON Schema definitions](https://github.com/vyncint/reconverge/tree/main/schemas) and
golden fixtures live in the repository, and this crate must round-trip
every fixture.

`baseline.v1` is the odd one out on purpose: it is written by a *human*
(through `cargo reconverge triage`) and read by the CLI, never by the
analysis — which is what keeps `findings.v1` a faithful record of what was
found, whatever anyone later decided about it.

End users want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge),
the CLI built on top.
