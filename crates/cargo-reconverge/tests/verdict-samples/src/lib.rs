//! Verdict samples: kernels whose finding must reach exactly one confidence
//! tier, for one stated reason. `tests/witness_verdicts.rs` holds the
//! inventory.
//!
//! No kernel here writes its output. Each one is about the replay, and the
//! index formula a write needs would add an RC005 beside every finding that
//! says nothing about it.

use cuda_device::{DisjointSlice, kernel, launch_contract, thread, warp};

// ------------------------------------------- #190: the declared block

/// Not confirmed: under the only block the contract allows, `threadIdx.x`
/// is 0..15, so every thread arrives. The replay used to run `(32, 1, 1)`
/// regardless, put lanes 16..31 on the other side of the guard, and confirm
/// a hang no allowed launch has. 256 threads is past what the replay runs,
/// so it declines and says so.
#[kernel]
#[launch_contract(domain = 2, coordinates = u32, block = (16, 16, 1))]
pub fn block_2d_x_guard_all_arrive(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() < 16 {
        thread::sync_threads();
    }
}

/// Not confirmed: a 16-thread block is half a warp, and every thread passes
/// the guard. Not a whole number of warps, so not replayed.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (16, 1, 1))]
pub fn block_half_warp_all_arrive(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() < 16 {
        thread::sync_threads();
    }
}

/// Not confirmed: the same guard under a 2-D block the replay *can* run.
/// `(16, 2, 1)` is one warp, `x` is 0..15 on both rows, and the replay —
/// now in the declared shape — finds every lane at the barrier.
#[kernel]
#[launch_contract(domain = 2, coordinates = u32, block = (16, 2, 1))]
pub fn block_2d_one_warp_all_arrive(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() < 16 {
        thread::sync_threads();
    }
}

/// Confirmed: the converse, and the false negative the fixed shape hid.
/// Lanes 16..31 of the one warp are row 1, so `threadIdx.y == 0` admits
/// half of them. Replayed as `(32, 1, 1)`, every lane had `y = 0`.
#[kernel]
#[launch_contract(domain = 2, coordinates = u32, block = (16, 2, 1))]
pub fn block_2d_row_guard_hangs(_out: DisjointSlice<u32>) {
    if thread::threadIdx_y() == 0 {
        thread::sync_threads();
    }
}

// --------------------------------------- #191: calls the inliner left in

/// Recursion: every thread reaches the one `sync_threads()` in
/// `countdown`, at a different depth.
fn countdown(n: u32) {
    if n == 0 {
        thread::sync_threads();
    } else {
        countdown(n - 1);
    }
}

/// Not confirmed: the inliner splices the first frame and leaves the
/// recursive call in place. The replay stepped over that call as if it
/// held no barrier, found one lane at the site and 31 "never arriving",
/// and confirmed it.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn recursion_all_arrive(_out: DisjointSlice<u32>) {
    let t = thread::threadIdx_x();
    if t < 32 {
        countdown(t);
    }
}

fn level3() {
    thread::sync_threads();
}
fn level2() {
    level3();
}
fn level1() {
    level2();
}
fn split_depth(t: u32) {
    if t == 0 {
        thread::sync_threads();
    } else {
        level1();
    }
}

/// Not confirmed: deeper than the inliner's two frames, so `level2()` is
/// still a call in the replayed model, and a barrier is behind it.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn depth_past_the_inliner(_out: DisjointSlice<u32>) {
    let t = thread::threadIdx_x();
    if t < 32 {
        split_depth(t);
    }
}

fn one_level() {
    thread::sync_threads();
}

/// Confirmed: the control. One frame inlines whole, nothing is left to
/// step over, and the hang is real.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn inlined_helper_hangs(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() < 4 {
        one_level();
    }
}

fn no_barrier(t: u32) -> u32 {
    t + 1
}

/// Confirmed: a local call the summaries clear of any barrier is still
/// stepped over. Refusing every call would have bought #191 with recall.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn sync_free_call_on_the_path(_out: DisjointSlice<u32>) {
    let t = thread::threadIdx_x();
    no_barrier(t);
    if t < 8 {
        thread::sync_threads();
    }
}

// --------------------------------------------- #192: signed arithmetic

/// Not confirmed: `s` is 0 on lane 0 and negative elsewhere, so `s < 1` on
/// every lane. Compared unsigned, -1 was `0xffffffff` and 31 lanes "never
/// arrived".
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn signed_all_lanes_arrive(_out: DisjointSlice<u32>) {
    let s = -(warp::lane_id() as i32);
    if s < 1 {
        thread::sync_threads();
    }
}

/// Not confirmed, and not "unreachable" either: `x > -1` holds on every
/// lane. Unsigned, `-1` was the largest value and no lane passed, which the
/// replay reported as a result.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn signed_negative_literal(_out: DisjointSlice<u32>) {
    let x = warp::lane_id() as i32;
    if x > -1 {
        thread::sync_threads();
    }
}

/// Confirmed: a real split written in `i32` — the control.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn signed_real_split(_out: DisjointSlice<u32>) {
    let x = warp::lane_id() as i32;
    if x < 16 {
        thread::sync_threads();
    }
}

/// Confirmed, on the *upper* half: signed division truncates toward zero,
/// so `-(lane) / 2 < -7` exactly when `lane >= 16`. Unsigned, every
/// negative quotient was huge and no lane passed.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn signed_division_split(_out: DisjointSlice<u32>) {
    let s = -(warp::lane_id() as i32);
    if s / 2 < -7 {
        thread::sync_threads();
    }
}

// ------------------------------------------ #194: warp-uniform guards

/// Not reported: under a 1-D block `warp_id()` is a warp number, the same
/// on every lane of a warp, so each warp takes the branch whole and the
/// full mask names only lanes that are there.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (64, 1, 1))]
pub fn warp_uniform_guard(_out: DisjointSlice<u32>) {
    if warp::warp_id() == 0 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}

/// Not reported: the same guard spelled as arithmetic.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (64, 1, 1))]
pub fn warp_uniform_division(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() / 32 == 0 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}

/// Not reported: and as a shift, by more than a warp.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (128, 1, 1))]
pub fn warp_uniform_shift(_out: DisjointSlice<u32>) {
    if thread::threadIdx_x() >> 6 == 1 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}

/// Warning: no contract, so no block shape to rely on. Under `(48, 2, 1)`
/// warp 1 holds `x` 32..47 and 0..15, and `warp_id()` (`threadIdx_x() /
/// 32`) splits it. The finding stays, and a note names the contract as
/// what settles it.
#[kernel]
pub fn warp_guard_without_contract(_out: DisjointSlice<u32>) {
    if warp::warp_id() == 0 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}

/// Warning: that block, declared. Warp 1 is split by the guard, so the
/// finding is real; it is not promoted only because a collective site is
/// replayed within one warp, and three are declared.
#[kernel]
#[launch_contract(domain = 2, coordinates = u32, block = (48, 2, 1))]
pub fn warp_guard_rows_split_a_warp(_out: DisjointSlice<u32>) {
    if warp::warp_id() == 1 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}

/// Confirmed: a value that *starts* as a warp number but is overwritten on
/// half of each warp is not warp-uniform. Scope follows control the way
/// divergence does, or `w` would have kept `warp_id()`'s and quieted this.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (32, 1, 1))]
pub fn warp_number_overwritten_per_lane(_out: DisjointSlice<u32>) {
    let mut w = warp::warp_id();
    if warp::lane_id() < 16 {
        w = 5;
    }
    if w == 0 {
        warp::ballot_sync(0xffff_ffff, true);
    }
}
