//! `inspect` — the uniformity and provenance browser.
//!
//! A pure reader of `unimap.v1` (labels, edges, blocks, coverage) and
//! `findings.v1` (for jumping between findings), plus the source files the
//! artifacts point at. Keyboard model:
//!
//! - `j`/`k` (or arrows): select the next/previous listed value
//! - `p`/`Enter`: walk the selected value's provenance one hop toward its
//!   divergence source
//! - `u`: walk back
//! - `n`/`N`: jump to the next/previous finding
//! - `f`: cycle functions
//! - `q`/`Esc`: quit
//!
//! State is a pure function of (artifacts, key sequence): all transitions
//! live in [`state`] with no I/O, and the view derives scrolling from the
//! selection, so any screen is reproducible from a fixture plus a scripted
//! key list.

pub mod data;
pub mod state;
pub mod view;

pub use data::InspectorData;
pub use state::{InspectorState, KeyAction};
