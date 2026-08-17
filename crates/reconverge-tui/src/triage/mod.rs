//! `triage` — interactive findings review.
//!
//! Reads `findings.v1` documents and the reviewed baseline
//! (`baseline.v1`), and lets a maintainer walk the findings deciding which
//! are accepted and why. Keyboard model:
//!
//! - `j`/`k` (or arrows): move through the findings
//! - `s`: accept the selected finding — opens the reason editor
//! - `u`: withdraw an acceptance
//! - `w`: write the baseline
//! - `q`: quit (one confirmation when there are unsaved edits; `Q`
//!   discards them)
//! - while editing: type the reason, `Enter` saves, `Esc` cancels
//!
//! This is the one view that *writes*. It is still not an analysis: it
//! reads the engine's findings and records human decisions about them
//! (§3's rule is that the TUI never re-implements analysis, and §8 puts
//! baseline updates in triage's job description). Every transition in
//! [`state`] stays pure — writing happens in the binary's event loop,
//! which is the only place that touches the filesystem.

pub mod data;
pub mod state;
pub mod view;

pub use data::{TriageData, TriageItem};
pub use state::{KeyAction, Status, TriageState};
