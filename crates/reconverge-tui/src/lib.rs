//! Ratatui front-end: inspector, witness debugger, learn mode, triage.
//!
//! The TUI is a **pure reader of versioned artifacts** — it never
//! re-implements analysis, and it may depend on no workspace crate other
//! than `reconverge-artifacts` (CI-enforced by
//! `scripts/check-isolation.sh`). If a view needs data the artifacts lack,
//! extend the schema instead.
//!
//! Four views, each a module of its own: [`inspect`] browses uniformity
//! labels and provenance chains (`unimap.v1`), [`witness`] steps one warp
//! through a recorded replay (`witness.v1`), [`learn`] teaches SIMT from
//! embedded lessons over shipped replays, and [`triage`] reviews findings
//! (`findings.v1`) and records accepted ones (`baseline.v1`) — the one view
//! that writes, and only ever the file its launcher named. Run without a
//! view, the binary is a shell that summarizes whatever artifacts it is
//! given.
//!
//! Standards baked in from day one (docs/ARCHITECTURE.md): redraw only on input or
//! data change (no timers); deterministic frames (no wall-clock, PID, or
//! absolute paths — dynamic values go through the redaction helpers in
//! [`view`]); `NO_COLOR` honored; `--ascii` glyph fallback; strings are
//! NFC-normalized on load; any screen is a function of (artifact, key
//! sequence).

#![forbid(unsafe_code)]

pub mod inspect;
pub mod learn;
pub mod load;
pub mod triage;
pub mod view;
pub mod witness;
