# reconverge-tui

The terminal views behind [`cargo-reconverge`](https://github.com/vyncint/reconverge): four Ratatui
interfaces over reconverge's versioned artifacts.

- **inspect** — browse source with per-value uniformity labels and walk a
  value's provenance back to its divergence source.
- **witness** — step one warp through a recorded replay: 32 lanes, barrier
  arrivals, participation mask against the lanes that actually arrive.
- **learn** — four embedded SIMT lessons (divergence, barriers, masks,
  reconvergence) that drive the same replay engine, fully offline.
- **triage** — review findings and record accepted ones, with reasons, in
  the baseline.

**You probably want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge)**,
which launches these views on the right artifacts — `cargo reconverge
setup` installs the matching version of this binary.

The TUI is a pure reader: it depends on no reconverge crate except the
artifact bindings, never re-runs analysis, and every frame is a function of
(artifacts, key sequence) — no timers, no clock, `NO_COLOR` honored, and an
`--ascii` mode for terminals without the glyphs.
