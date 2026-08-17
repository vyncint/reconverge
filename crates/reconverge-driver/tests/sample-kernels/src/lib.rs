//! Sample cuda-oxide kernel crate for the MIR access test.
//!
//! Two deliberately small kernels: one clean, and one carrying the canonical
//! divergent-barrier shape (`if idx % 2 == 0 { sync_threads() }`) that RC001
//! exists to catch — the analysis finds it without a GPU, and the witness
//! interpreter replays it lane by lane.

use cuda_device::{DisjointSlice, kernel, thread};

#[kernel]
pub fn scale(mut out: DisjointSlice<f32>, factor: f32) {
    let i = thread::index_1d();
    if let Some(e) = out.get_mut(i) {
        *e *= factor;
    }
}

#[kernel]
pub fn divergent_barrier(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 {
        thread::sync_threads();
    }
    if let Some(e) = out.get_mut(i) {
        *e = 1;
    }
}
