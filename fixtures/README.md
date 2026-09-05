# Fixtures

Golden artifact JSONs. Hand-written first, so the TUI could be built and
tested before the engine existed — and, until 0.5.0, hand-written still:
all three witness documents showed MIR statements no released driver has
ever emitted, and all three were stamped `tool.version 0.0.0`, which is the
tell. `scripts/record-fixtures.sh` now regenerates the witness fixtures from
a real `cargo reconverge check` over `crates/cargo-reconverge/tests/lint-samples`,
and CI diffs them (`--check`), so the API tests test the producer.

They serve two purposes:

- **API tests for the schemas** — every schema change updates the fixtures
  in the same PR; `reconverge-artifacts` must round-trip all of them.
- **termlens test inputs** — TUI golden-frame and flow tests spawn the TUI
  on these fixtures, and learn mode embeds three of them outright, so its
  lessons run with nothing on disk.

| Fixture | What it is |
|---|---|
| `findings/rc003-minimal.json` | one `deny` and one `warning` finding, the smallest realistic report |
| `unimap/divergent-barrier.json` | uniformity labels and provenance edges for the canonical divergent barrier |
| `witness/rc001-divergent-barrier.json` | recorded — the canonical RC001 replay: 16 lanes wait at a barrier the other 16 exited past |
| `witness/rc002-partial-mask.json` | recorded — a `ballot_sync` whose mask names 32 lanes while 16 arrive |
| `witness/rc001-multiwarp-barrier.json` | recorded — a `warp_id()`-guarded barrier under a declared 64-thread block: the 64-lane shape `witness.v1` used to reject |
| `witness/reconverged-clean.json` | **the one synthetic fixture**: the *fixed* kernel, all 32 lanes rejoin, the barrier releases, verdict `completed`. No run can record it — a witness is written only for a *confirmed* finding, and a kernel that reconverges has none — so it is written by hand in the driver's own voice and stamped the workspace version |
| `baseline/minimal.json` | one reviewed suppression, leaving the other finding open |
| `inspect/` | a whole scenario — real tool output plus the source it points at — for the Inspector's flow tests |
