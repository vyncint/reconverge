//! Kernels whose *source lines* are the point.
//!
//! Every one of these is an ordinary divergent barrier — a confirmed RC001 —
//! so the analysis is never what is under test. What differs is the shape of
//! the line the diagnostic has to print: a real `ESC` byte in a comment, a
//! leading tab, a wide identifier before the span, and a line long enough to
//! scroll the diagnostic off an 80x24 terminal.
//!
//! Not formatted by `cargo fmt --all`: the crate declares its own
//! `[workspace]`, which is what keeps the tab indentation below intact.

use cuda_device::{DisjointSlice, kernel, thread};

/// Renders before the escaped kernel, so a test can assert it is still on
/// screen after that one has been printed.
#[kernel]
pub fn aaa_first(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 { thread::sync_threads(); }
    if let Some(e) = out.get_mut(i) { *e = 1; }
}

/// The comment holds two real `ESC` bytes — erase display, cursor home. A
/// renderer that copies the line through wipes every diagnostic above it.
#[kernel]
pub fn zzz_escaped(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 { thread::sync_threads(); } // [2J[H
    if let Some(e) = out.get_mut(i) { *e = 2; }
}

/// Tab-indented: a terminal advances to the next tab stop, so a caret
/// offset counted in characters lands short.
#[kernel]
pub fn tabbed(mut out: DisjointSlice<u32>) {
	let i = thread::index_1d();
	if i.get() % 2 == 0 { thread::sync_threads(); }
	if let Some(e) = out.get_mut(i) { *e = 3; }
}

/// A wide identifier before the span: eight CJK characters are sixteen
/// cells, not eight.
#[kernel]
pub fn wide_cjk(mut out: DisjointSlice<u32>) {
    let 幅幅幅幅幅幅幅幅 = thread::index_1d();
    if 幅幅幅幅幅幅幅幅.get() % 2 == 0 { thread::sync_threads(); }
    if let Some(e) = out.get_mut(幅幅幅幅幅幅幅幅) { *e = 4; }
}

/// The barrier sits past column 760, so the untrimmed snippet row and its
/// caret row were ten wrapped rows each on an 80-column terminal.
#[kernel]
pub fn longline(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 { let _pad: u32 = 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1; thread::sync_threads(); }
    if let Some(e) = out.get_mut(i) { *e = 5; }
}
