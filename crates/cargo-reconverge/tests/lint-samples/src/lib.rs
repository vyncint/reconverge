//! Lint-sample kernels: minimal true positives and known-tricky true
//! negatives for RC003, RC004, and RC005 (CONTRIBUTING.md testing policy).
//!
//! Sizing notes for the RC004 cases: `SharedArray<T, N>` is a host-side
//! marker whose device footprint is `N * size_of::<T>()`; a `Barrier` is 8
//! bytes of shared memory; the static cap is 49152 bytes (48 KiB).

use cuda_device::barrier::{Barrier, mbarrier_arrive, mbarrier_init, mbarrier_wait};
use cuda_device::{DisjointSlice, SharedArray, kernel, launch_contract, thread, warp};

// ---------------------------------------------------------------- RC003

/// True positive: `&mut [T]` kernel parameter.
#[kernel]
pub fn rc003_mut_slice(data: &mut [f32]) {
    if let Some(x) = data.get_mut(0) {
        *x = 1.0;
    }
}

/// True negative: a shared reference is not a parallel-write hazard.
#[kernel]
pub fn rc003_ok_shared_ref(data: &[f32], mut out: DisjointSlice<f32>) {
    let i = thread::index_1d();
    let v = data.first().copied().unwrap_or_default();
    if let Some(e) = out.get_mut(i) {
        *e = v;
    }
}

/// True negative: `&mut [T]` in a plain host function is not a kernel
/// parameter.
pub fn rc003_ok_not_a_kernel(data: &mut [f32]) {
    if let Some(x) = data.first_mut() {
        *x = 1.0;
    }
}

// ---------------------------------------------------------------- RC004

/// True positive: 65536 bytes, with the length given as a **named const**.
///
/// The same footprint as `rc004_over_budget`, spelled the way a tunable
/// kernel spells it. Until this was fixed the const arrived as an
/// unevaluated anonymous body, `eval_target_usize` refused it, and the
/// static vanished from the budget with no finding and no diagnostic — so
/// this kernel came back clean. An autotuner that rewrites named consts per
/// candidate takes this path for every configuration it tries.
pub const OVER_TILE: usize = 16384;

#[kernel]
pub fn rc004_named_const_over_budget(mut out: DisjointSlice<f32>) {
    static mut STAGE: SharedArray<f32, OVER_TILE> = SharedArray::UNINIT;
    let i = thread::index_1d();
    unsafe {
        STAGE[0] = 0.0;
    }
    if let Some(e) = out.get_mut(i) {
        *e = unsafe { STAGE[0] };
    }
}

/// True negative: 4096 bytes through the same named-const path.
///
/// The pair matters more than either half. Resolving the const only counts
/// if it resolves to the *right* number: a fix that reported every named
/// size as over-budget would satisfy the test above on its own.
pub const SMALL_TILE: usize = 1024;

#[kernel]
pub fn rc004_ok_named_const_under(mut out: DisjointSlice<f32>) {
    static mut SMALL: SharedArray<f32, SMALL_TILE> = SharedArray::UNINIT;
    let i = thread::index_1d();
    unsafe {
        SMALL[0] = 0.0;
    }
    if let Some(e) = out.get_mut(i) {
        *e = unsafe { SMALL[0] };
    }
}

/// True positive: 65536 bytes of static shared memory.
#[kernel]
pub fn rc004_over_budget(mut out: DisjointSlice<f32>) {
    static mut TILE: SharedArray<f32, 16384> = SharedArray::UNINIT;
    let i = thread::index_1d();
    unsafe {
        TILE[0] = 0.0;
    }
    if let Some(e) = out.get_mut(i) {
        *e = unsafe { TILE[0] };
    }
}

/// True positive: 49148 array bytes plus an 8-byte barrier = 49156.
#[kernel]
pub fn rc004_barrier_pushes_over(mut out: DisjointSlice<u32>) {
    static mut TILE: SharedArray<f32, 12287> = SharedArray::UNINIT;
    static mut BAR: Barrier = Barrier::UNINIT;
    let tid = thread::threadIdx_x();
    let block = thread::blockDim_x();
    let i = thread::index_1d();
    if tid == 0 {
        unsafe {
            mbarrier_init(&raw mut BAR, block);
        }
    }
    thread::sync_threads();
    unsafe {
        TILE[0] = 1.0;
    }
    let token = unsafe { mbarrier_arrive(&raw const BAR) };
    unsafe { mbarrier_wait(&raw const BAR, token) }
    if let Some(e) = out.get_mut(i) {
        *e = 1;
    }
}

/// True negative: exactly at the 49152-byte static cap.
#[kernel]
pub fn rc004_ok_at_limit(mut out: DisjointSlice<f32>) {
    static mut TILE: SharedArray<f32, 12288> = SharedArray::UNINIT;
    let i = thread::index_1d();
    unsafe {
        TILE[0] = 2.0;
    }
    if let Some(e) = out.get_mut(i) {
        *e = unsafe { TILE[0] };
    }
}

// ---------------------------------------------------------------- RC005

/// True positive (warning): `domain = 2` contract with the 1D-only formula.
#[kernel]
#[launch_contract(domain = 2, coordinates = u32, block = (16, 16, 1))]
pub fn rc005_mismatch(mut out: DisjointSlice<f32>) {
    let i = thread::index_1d();
    if let Some(e) = out.get_mut(i) {
        *e = 3.0;
    }
}

/// True positive (warning): shape-dependent formula, no contract at all.
#[kernel]
pub fn rc005_missing_contract(mut out: DisjointSlice<f32>) {
    let i = thread::index_1d();
    if let Some(e) = out.get_mut(i) {
        *e = 4.0;
    }
}

/// True negative: matching 1D contract.
#[kernel]
#[launch_contract(domain = 1, coordinates = u32, block = (128, 1, 1))]
pub fn rc005_ok_contracted(mut out: DisjointSlice<f32>) {
    let i = thread::index_1d();
    if let Some(e) = out.get_mut(i) {
        *e = 5.0;
    }
}

// ---------------------------------------------------------------- RC001

/// True positive (warning): the canonical divergent barrier.
#[kernel]
pub fn rc001_divergent_barrier(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 {
        thread::sync_threads();
    }
    if let Some(e) = out.get_mut(i) {
        *e = 1;
    }
}

/// True negative: the canonical block-uniform barrier.
#[kernel]
pub fn rc001_ok_block_uniform(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if thread::blockIdx_x() > 3 {
        thread::sync_threads();
    }
    if let Some(e) = out.get_mut(i) {
        *e = 2;
    }
}

/// True negative: the thread-0-initializes pattern — the barrier sits
/// after the divergent `if` reconverges, so every thread reaches it.
#[kernel]
pub fn rc001_ok_reconverged(mut out: DisjointSlice<u32>) {
    let tid = thread::threadIdx_x();
    let mut x = 0u32;
    if tid == 0 {
        x = 1;
    }
    thread::sync_threads();
    let i = thread::index_1d();
    if let Some(e) = out.get_mut(i) {
        *e = x;
    }
}

/// Plain helper containing a barrier, for the interprocedural case.
fn barrier_helper() {
    thread::sync_threads();
}

// ---------------------------------------------------------------- RC002

/// True positive (warning): a full-mask collective under a
/// thread-divergent branch — the named lanes may never arrive.
#[kernel]
pub fn rc002_divergent_collective(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    let mut vote = 0u32;
    if i.get() % 2 == 0 {
        vote = warp::ballot_sync(0xffff_ffff, true);
    }
    if let Some(e) = out.get_mut(i) {
        *e = vote;
    }
}

/// True negative: the same collective at a convergent point.
#[kernel]
pub fn rc002_ok_convergent(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    let vote = warp::ballot_sync(0xffff_ffff, i.get() % 2 == 0);
    if let Some(e) = out.get_mut(i) {
        *e = vote;
    }
}

/// Plain helper containing a collective, for the interprocedural case.
fn collective_helper() -> bool {
    warp::all_sync(0xffff_ffff, true)
}

/// True positive (warning): a call that may execute a warp collective,
/// made under thread-divergent control.
#[kernel]
pub fn rc002_divergent_call(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    let mut agreed = false;
    if i.get() % 2 == 0 {
        agreed = collective_helper();
    }
    if let Some(e) = out.get_mut(i) {
        *e = u32::from(agreed);
    }
}

/// True positive (warning): a call that may execute a barrier, made under
/// thread-divergent control (interprocedural summary bits).
#[kernel]
pub fn rc001_divergent_call(mut out: DisjointSlice<u32>) {
    let i = thread::index_1d();
    if i.get() % 2 == 0 {
        barrier_helper();
    }
    if let Some(e) = out.get_mut(i) {
        *e = 3;
    }
}
