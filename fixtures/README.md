# Fixtures

Golden artifact JSONs. Hand-written first, so the TUI could be built
and tested before the engine existed, then generated-and-reviewed. They
serve two purposes:

- **API tests for the schemas** — every schema change updates the fixtures
  in the same PR; `reconverge-artifacts` must round-trip all of them.
- **termlens test inputs** — TUI golden-frame and flow tests spawn the TUI
  on these fixtures, and learn mode embeds three of them outright, so its
  lessons run with nothing on disk.

| Fixture | What it is |
|---|---|
| `findings/rc003-minimal.json` | one `deny` and one `warning` finding, the smallest realistic report |
| `unimap/divergent-barrier.json` | uniformity labels and provenance edges for the canonical divergent barrier |
| `witness/rc001-divergent-barrier.json` | the canonical RC001 replay: 16 lanes wait at a barrier the other 16 exited past |
| `witness/rc002-partial-mask.json` | a `ballot_sync` whose mask names 32 lanes while 16 arrive |
| `witness/reconverged-clean.json` | the *fixed* kernel: all 32 lanes rejoin, the barrier releases, verdict `completed` |
| `baseline/minimal.json` | one reviewed suppression, leaving the other finding open |
| `inspect/` | a whole scenario — real tool output plus the source it points at — for the Inspector's flow tests |
