# Artifact schemas

Versioned JSON Schemas for every artifact reconverge emits. They are the
contract between the analysis engine and every front-end: the CLI renders
text from them, and the TUI is a pure reader of them (it never re-implements
analysis — if a view needs data an artifact lacks, extend the schema here).

| Schema | Contents |
|---|---|
| `findings.v1.json` | code, span, confidence, message, provenance refs, suggested fix, explain code |
| `unimap.v1.json` | per function: value uniformity labels, provenance edges, CFG blocks with divergent-control bits, source-span mapping |
| `witness.v1.json` | kernel id, launch config, 32-lane event timeline (lane states delta-encoded), barrier arrival counts, mask values at warp ops, verdict |
| `baseline.v1.json` | reviewed suppressions: `(crate, kernel, code)` plus the written reason each one was accepted for |

## Rules

- Each schema is semver'd **independently** of the crates.
- **Additive-only within a major version.** Removing or repurposing a field
  means a new major version (a new `*.vN.json` file).
- **Every schema change updates `fixtures/` in the same PR** — the fixtures
  are the API tests.
- `reconverge-artifacts` is the Rust (serde) binding of these schemas; it
  must round-trip every fixture.

Three of the schemas are produced by the driver; `baseline.v1` is written
by `cargo reconverge triage`.

`baseline.v1` is the odd one out on purpose: it is *written by a human*
through triage and read by the CLI, never by the analysis engine. Keeping
suppression outside `findings.v1` is what lets the findings artifact stay a
faithful record of what was found, whatever anyone later decided about it.
