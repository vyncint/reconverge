//! `witness view` — the 32-lane replay debugger.
//!
//! A pure reader of `witness.v1`: one warp, one timeline, stepped by hand.
//! Keyboard model:
//!
//! - `l`/`h` (or arrows): step forward/back through the MIR events
//! - `g`/`G`: jump to the launch instant / the end
//! - `d`: jump to the divergence (the first event that splits the warp)
//! - `v`: jump to the verdict step
//! - `n`/`N`: next/previous witness
//! - `q`/`Esc`: quit
//!
//! State is a pure function of (artifacts, key sequence): all transitions
//! live in [`state`] with no I/O, and every screen is reproducible from a
//! fixture plus a scripted key list. The lane strip speaks the same
//! language as the text diagnostics' warp diagram (`W.W.W.W. …`), so what
//! CI logs summarize, the debugger animates.

pub mod data;
pub mod state;
pub mod view;

pub use data::WitnessData;
pub use state::{KeyAction, WitnessState};
