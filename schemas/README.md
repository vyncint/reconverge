# Artifact schemas

Versioned JSON Schemas for every artifact reconverge emits. They are the
contract between the analysis engine and every front-end: the CLI renders
text from them, and the TUI is a pure reader of them (it never re-implements
analysis — if a view needs data an artifact lacks, extend the schema here).

| Schema | Contents |
|---|---|
| `findings.v1.json` | one document **per compiled target** (`crate` + `target`), a run-level coverage tally, and per finding: code, span, confidence, message, provenance refs, suggested fix, explain code |
| `unimap.v1.json` | per function: value uniformity labels, provenance edges, CFG blocks with divergent-control bits, source-span mapping |
| `witness.v1.json` | kernel id, launch config, an event timeline over one warp — or the declared block when a contract names several whole warps (lane states delta-encoded), barrier arrival counts, mask values at warp ops, verdict |
| `baseline.v1.json` | reviewed suppressions: `(crate, kernel, code)` plus the written reason each one was accepted for |

## Rules

- Each schema is semver'd **independently** of the crates.
- **Additive-only within a major version.** Removing or repurposing a field
  means a new major version (a new `*.vN.json` file).
- **Every schema change updates `fixtures/` in the same PR** — the fixtures
  are the API tests.
- `reconverge-artifacts` is the Rust (serde) binding of these schemas; it
  must round-trip every fixture.
- **Round-tripping is not validating.** serde tolerates anything additive and
  never sees a `const`, so `scripts/check-schemas.sh` validates `fixtures/`
  *and* what an end-to-end `check` emits against these files in CI. Without
  it, `witness.v1` pinned `lanes` at 32 for four minor versions while the
  driver wrote 64, 96 and 128 — and the artifacts that failed the published
  contract were exactly the gating ones.
- **The witness fixtures are recorded, not written.**
  `scripts/record-fixtures.sh` regenerates them from a real run and CI diffs
  them, so a fixture cannot describe an artifact nobody produces. The one
  exception is `witness/reconverged-clean.json`, which no run can record —
  a witness is only written for a *confirmed* finding, and a kernel that
  reconverges has none — and `fixtures/README.md` says so.

Three of the schemas are produced by the driver; `baseline.v1` is written
by `cargo reconverge triage`.

`baseline.v1` is the odd one out on purpose: it is *written by a human*
through triage and read by the CLI, never by the analysis engine. Keeping
suppression outside `findings.v1` is what lets the findings artifact stay a
faithful record of what was found, whatever anyone later decided about it.
