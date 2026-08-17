//! `learn` — four SIMT lessons: divergence,
//! barriers, masks, reconvergence. Everything is embedded — prose from
//! `docs/learn/`, kernel excerpts, and the shipped fixture witnesses — so
//! the lessons run with no network, no analysis step, and no files on
//! disk. The interactive pages drive the witness debugger's own replay
//! machinery, so a lesson animates exactly what `cargo reconverge
//! witness` shows on real findings.
//!
//! Keyboard model:
//!
//! - list: `j`/`k` select, `Enter`/`l` open, `q`/`Esc` quit
//! - page: `n`/`p` turn pages, `h`/`l` step the replay, `d`/`v` jump to
//!   the divergence/verdict, `Esc`/`b` back to the list, `q` quit
//!
//! State is a pure function of (embedded lessons, key sequence); every
//! screen is reproducible from a scripted key list alone.

pub mod lessons;
pub mod state;
pub mod view;

pub use lessons::{Lesson, lessons};
pub use state::{KeyAction, LearnState, Screen};
