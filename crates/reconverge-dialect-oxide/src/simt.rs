//! The `SimtDialect` implementation for cuda-oxide: call classification by
//! definition path, verified against cuda-device at the pinned rev.
//! Path matching only — no upstream code is vendored.

use reconverge_core::LaunchScope;
use reconverge_core::dialect::SimtDialect;
// Re-exported: `classify_call` is public, so the vocabulary of its return
// type has to be reachable from the same path. A caller could not name what
// it was handed.
pub use reconverge_core::dialect::{CallKind, MaskSource};

/// The cuda-oxide dialect.
#[derive(Debug, Clone, Copy, Default)]
pub struct CudaOxide;

impl SimtDialect for CudaOxide {
    fn classify_call(&self, def_path: &str) -> CallKind {
        classify_call(def_path)
    }

    fn classify_method_call(&self, def_path: &str, receiver: Option<&str>) -> CallKind {
        classify_method_call(def_path, receiver)
    }

    fn barrier_scope(&self, def_path: &str, receiver: Option<&str>) -> LaunchScope {
        barrier_scope(def_path, receiver)
    }
}

/// How far a barrier's participant set reaches (free function form).
///
/// Asked only of callees [`classify_method_call`] already called
/// [`CallKind::Barrier`], so anything unrecognized here is a block barrier
/// — the narrow answer, which reports nothing extra.
///
/// `ThreadGroup::sync` is one path for three scopes and is told apart by
/// receiver, exactly as its classification is.
#[must_use]
pub fn barrier_scope(def_path: &str, receiver: Option<&str>) -> LaunchScope {
    if def_path.ends_with("::cooperative_groups::ThreadGroup::sync")
        && let Some(receiver) = receiver
    {
        return match receiver.rsplit("::").next().unwrap_or(receiver) {
            "Grid" => LaunchScope::Grid,
            "Cluster" => LaunchScope::Cluster,
            _ => LaunchScope::Block,
        };
    }
    let last = def_path.rsplit("::").next().unwrap_or(def_path);
    match last {
        // `grid::sync()` — every block of the launch must arrive.
        "sync" if def_path.contains("::grid::") => LaunchScope::Grid,
        // The cluster barrier, safe wrapper and raw waiting half alike.
        "cluster_sync" | "barrier_cluster_wait" | "barrier_cluster_wait_aligned"
            if def_path.contains("::cluster::") =>
        {
            LaunchScope::Cluster
        }
        _ => LaunchScope::Block,
    }
}

/// Classify a callee whose meaning depends on its receiver.
///
/// `cooperative_groups::ThreadGroup::sync` is one definition path for five
/// different barriers: on a `ThreadBlock`, `Grid` or `Cluster` it is the
/// scope-wide barrier RC001 is about — every thread of the scope must
/// arrive, so a divergent call hangs exactly like `sync_threads` — while on a
/// `WarpTile<N>` or `CoalescedThreads` the participants are the tile or the
/// lanes that happen to be active, a partial-participation contract this
/// analysis does not model (conformance/SURFACE_ALLOW says so). Without a
/// receiver the path alone cannot tell them apart and stays `Other`.
#[must_use]
pub fn classify_method_call(def_path: &str, receiver: Option<&str>) -> CallKind {
    if def_path.ends_with("::cooperative_groups::ThreadGroup::sync")
        && let Some(receiver) = receiver
        && receiver.starts_with("cuda_device::cooperative_groups::")
    {
        let group = receiver.rsplit("::").next().unwrap_or(receiver);
        return match group {
            "ThreadBlock" | "Grid" | "Cluster" => CallKind::Barrier,
            _ => CallKind::Other,
        };
    }
    classify_call(def_path)
}

/// Classify a cuda-device callee (free function form; see [`CudaOxide`]).
#[must_use]
pub fn classify_call(def_path: &str) -> CallKind {
    if let Some(kind) = classify_int_intrinsic(def_path) {
        return kind;
    }
    if !def_path.starts_with("cuda_device::") {
        return CallKind::Other;
    }
    let last = def_path.rsplit("::").next().unwrap_or(def_path);
    let in_internal = def_path.contains("::__internal::");

    match last {
        // Thread-index witnesses: the rewritten intrinsics the #[kernel]
        // macro emits, plus the raw per-thread built-ins.
        "index_1d" | "index_2d" | "index_2d_runtime" | "warp_index" | "index_1d_u32"
        | "coord_2d_u32" | "index_2d_row" | "index_2d_col"
            if in_internal =>
        {
            CallKind::ThreadIndexWitness
        }
        "threadIdx_x" | "threadIdx_y" | "threadIdx_z" | "lane_id" => CallKind::ThreadIndexWitness,

        // Uniform within a block and different in the next one: which
        // block this is. Enough to decide `sync_threads`, not enough to
        // decide a cluster- or grid-wide barrier (#133).
        "blockIdx_x" | "blockIdx_y" | "blockIdx_z" => CallKind::BlockUniform,
        // The launch geometry: the same number on every thread of every
        // block, so it decides a barrier at any scope.
        "blockDim_x" | "blockDim_y" | "blockDim_z" | "gridDim_x" | "gridDim_y" | "gridDim_z" => {
            CallKind::GridUniform
        }
        // Special registers that read the same on every thread of a block.
        // `smid` is which SM this block landed on — a block fact, and one
        // that differs between blocks. The rest are launch-wide: how many
        // SMs the device has, the grid's id, the warp-slot count, and the
        // two launch-environment registers.
        "smid" if def_path.contains("::thread::") => CallKind::BlockUniform,
        "nsmid" | "gridid" if def_path.contains("::thread::") => CallKind::GridUniform,
        "nwarpid" if def_path.contains("::warp::") => CallKind::GridUniform,
        "envreg1" | "envreg2" if def_path.contains("::grid::") => CallKind::GridUniform,
        // The raw cluster barrier is a *split* barrier: `barrier.cluster.arrive`
        // signals and returns, `barrier.cluster.wait` blocks until the cluster
        // has arrived. Only the waiting half can hang, so only it is RC001's
        // subject.
        //
        // 0.6.0 classified both halves as `Barrier`, on the reasoning that
        // "every thread of the cluster must reach both". That is true of the
        // pair and false of each instruction: a warp may arrive through
        // `arrive_aligned` while its sibling arrives through `arrive`, which
        // is a legal split arrival and was reported as two confirmed
        // deadlocks (#132).
        //
        // The arrival half joins the mbarrier family in
        // `conformance/SURFACE_ALLOW` for the same reason those are there:
        // partial participation at one arrival instruction is the designed
        // use, and deciding it needs the phase counting explain/RC001.md
        // declines. The boundary that buys is written down there too —
        // `if c { arrive() } wait()` is a real hang this does not report.
        "barrier_cluster_wait" | "barrier_cluster_wait_aligned"
            if def_path.contains("::cluster::") =>
        {
            CallKind::Barrier
        }

        // Execution barriers (RC001's subject): every primitive whose
        // contract is "all threads of the scope must reach this call".
        // Divergence *within a block* breaks the block, cluster, and grid
        // scopes alike, so one CallKind covers all three. The mbarrier
        // arrive/wait family (`barrier::Barrier`) is deliberately absent:
        // it is a phase-counted split barrier where partial participation
        // is the designed use, so "some threads never reach the wait" is
        // not by itself a bug — a documented v1 boundary (explain/RC001.md).
        "sync_threads" | "cluster_sync" => CallKind::Barrier,
        "sync" if def_path.contains("::grid::") => CallKind::Barrier,
        // Cooperative-groups block helpers carry a block-wide barrier inside
        // (`block_reduce` and `block_scan` both `sync_threads` between their
        // warp and block phases), so a divergent call is the divergent
        // barrier one helper deeper. The tile-scoped siblings (`warp_reduce`,
        // `warp_scan` over a `WarpTile<N>`) are not here: their participant
        // set is the tile, which is not modelled — conformance/SURFACE_ALLOW.
        "block_reduce" | "block_scan" if def_path.contains("::cooperative_groups::") => {
            CallKind::Barrier
        }
        // Cluster geometry, split by how far each one is actually
        // constant. The block's rank within its cluster and its cluster
        // coordinates are block facts that differ across the cluster —
        // exactly the values that decide a `cluster_sync` for some blocks
        // and not others (#133).
        "block_rank" | "cluster_ctaidX" | "cluster_ctaidY" | "cluster_ctaidZ"
            if def_path.contains("::cluster::") =>
        {
            CallKind::BlockUniform
        }
        // Which cluster this is: the same on every block of the cluster,
        // so it decides a cluster-wide barrier, and different in the next
        // cluster, so it does not decide a grid-wide one.
        "cluster_idx" if def_path.contains("::cluster::") => CallKind::ClusterUniform,
        // The cluster's shape is launch geometry, like `blockDim`.
        "cluster_size" | "num_clusters" | "cluster_nctaidX" | "cluster_nctaidY"
        | "cluster_nctaidZ"
            if def_path.contains("::cluster::") =>
        {
            CallKind::GridUniform
        }

        // Warp collectives (RC002's subject): cuda-device's masked `*_sync`
        // surface, every one taking the participation mask as its first
        // argument, plus `sync_mask` — the warp barrier, whose mask carries
        // the same contract. The unmasked convenience wrappers (`shuffle`,
        // `ballot`, `all`, `any`, …) hide the collective — and an implicit
        // full mask — inside cuda-device; they are classified below as
        // `ImplicitFull` rather than misread as mask-first calls. They have
        // been covered since #21, and explain/RC002.md now says so.
        // The partial-warp reducers build their mask from a runtime
        // `live_lanes` argument, so it is neither full nor the first
        // argument. Classified anyway: a warning naming an unevaluable
        // mask is worth more than silence, and calling it full would be
        // a confident wrong answer.
        "reduce_sum_f32_partial"
        | "reduce_sum_f64_partial"
        | "reduce_max_f32_partial"
        | "reduce_max_f64_partial"
        | "reduce_min_f32_partial"
        | "reduce_min_f64_partial"
            if def_path.contains("::warp::") =>
        {
            CallKind::WarpCollective {
                mask: MaskSource::Unknown,
            }
        }

        // The unmasked convenience wrappers. Each one delegates to its
        // `*_sync` counterpart with `u32::MAX`, verified against
        // cuda-device at the pinned rev, so the participation mask is
        // known from the call: the wrapper supplies it. Treating these
        // as ordinary calls made a kernel written entirely against the
        // ergonomic API analyze as though it held no collectives —
        // silence rather than a warning, which is the worse failure.
        "all" | "any" | "ballot" | "popc" | "shuffle" | "shuffle_xor" | "shuffle_down"
        | "shuffle_up" | "shuffle_f32" | "shuffle_xor_f32" | "shuffle_down_f32"
        | "shuffle_up_f32" | "shuffle_u64" | "shuffle_xor_u64" | "shuffle_down_u64"
        | "shuffle_up_u64" | "shuffle_f64" | "shuffle_xor_f64" | "shuffle_down_f64"
        | "shuffle_up_f64" | "reduce_sum_f32" | "reduce_max_f32" | "reduce_min_f32"
        | "reduce_sum_f64" | "reduce_max_f64" | "reduce_min_f64" | "warp_reduce_sum"
            if def_path.contains("::warp::") =>
        {
            CallKind::WarpCollective {
                mask: MaskSource::ImplicitFull,
            }
        }

        "ballot_sync"
        | "any_sync"
        | "all_sync"
        | "shuffle_sync"
        | "shuffle_up_sync"
        | "shuffle_down_sync"
        | "shuffle_xor_sync"
        | "shuffle_f32_sync"
        | "shuffle_up_f32_sync"
        | "shuffle_down_f32_sync"
        | "shuffle_xor_f32_sync"
        | "shuffle_u64_sync"
        | "shuffle_up_u64_sync"
        | "shuffle_down_u64_sync"
        | "shuffle_xor_u64_sync"
        | "shuffle_f64_sync"
        | "shuffle_up_f64_sync"
        | "shuffle_down_f64_sync"
        | "shuffle_xor_f64_sync"
        | "match_any_sync"
        | "match_any_i64_sync"
        | "match_all_sync"
        | "match_all_i64_sync"
        | "redux_sync_add"
        | "redux_sync_and"
        | "redux_sync_or"
        | "redux_sync_xor"
        | "redux_sync_min_u32"
        | "redux_sync_min_i32"
        | "redux_sync_max_u32"
        | "redux_sync_max_i32"
        | "redux_sync_min_f32"
        | "redux_sync_max_f32"
        | "redux_sync_min_abs_f32"
        | "redux_sync_max_abs_f32"
        | "redux_sync_min_nan_f32"
        | "redux_sync_max_nan_f32"
        | "redux_sync_min_abs_nan_f32"
        | "redux_sync_max_abs_nan_f32"
        | "elect_sync"
        | "is_elected_sync"
        | "sync_mask" => CallKind::WarpCollective {
            mask: MaskSource::FirstArgument,
        },

        // Per-lane and per-warp environment reads: divergent by definition
        // (the lanemask registers differ on every lane; `warp_id` and
        // `live_lanes_1d` are warp-uniform, and the lattice does not
        // distinguish warp- from block-uniformity — same rule as collective
        // results), but none is a collective: no mask, no synchronization,
        // legal under divergence. Not replay-evaluable yet: giving the
        // interpreter their values needs width-typed evaluation (integer
        // `!`, truncating casts), so guards on them stay warning-tier.
        "active_mask" | "lanemask_lt" | "lanemask_le" | "lanemask_eq" | "lanemask_ge"
        | "lanemask_gt" | "warp_id" | "live_lanes_1d" => CallKind::DivergentEnvRead,
        // `%warpid`, the hardware warp slot: warp-uniform and, like
        // `warp_id`, not something the lattice separates from thread-level
        // divergence. Not the logical warp index (`warp_id` is), and never
        // a collective.
        "warpid" if def_path.contains("::warp::") => CallKind::DivergentEnvRead,

        // Dialect plumbing with uniform, effect-free results.
        "make_kernel_scope"
        | "__launch_contract_config"
        | "__launch_contract_block_config"
        | "__launch_bounds_config"
        | "__cluster_config"
        | "__unchecked_indexing_config"
        | "__unroll_config" => CallKind::UniformMarker,

        // Reading a witness back out is the identity on the witness value
        // for the interpreter (`ThreadIndex::get`).
        "get" if def_path.contains("ThreadIndex") => CallKind::WitnessRead,

        _ => {
            // Atomic read-modify-writes return the previous value, which
            // differs per thread by construction.
            if def_path.contains("::atomic::") {
                CallKind::AtomicRmw
            } else {
                CallKind::Other
            }
        }
    }
}

/// Primitive-integer intrinsics the witness interpreter can evaluate,
/// recognized by their inherent-impl definition path and *only* there.
///
/// The shape is `core::num::<impl {int}>::{method}` — the width lives in
/// the path, which is what makes the popcount evaluable at all. Matching
/// on the bare final segment instead would claim every `count_ones` in
/// the dependency graph (`bitvec`'s `BitSlice`, `roaring`'s bitmap, any
/// inherent method a user writes), whose first argument is a receiver
/// rather than the bits — a popcount of a pointer, reported as fact.
///
/// `usize`/`isize` are deliberately absent: their width is target-defined
/// and not recoverable from the path, and an assumed width is exactly the
/// confident wrong answer this function exists to avoid.
fn classify_int_intrinsic(def_path: &str) -> Option<CallKind> {
    let rest = def_path.strip_prefix("core::num::<impl ")?;
    let (ty, method) = rest.split_once(">::")?;
    if method != "count_ones" {
        return None;
    }
    let bits = match ty {
        "u8" | "i8" => 8,
        "u16" | "i16" => 16,
        "u32" | "i32" => 32,
        "u64" | "i64" => 64,
        "u128" | "i128" => 128,
        _ => return None,
    };
    Some(CallKind::CountOnes { bits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_index_witnesses() {
        assert_eq!(
            classify_call("cuda_device::thread::__internal::index_1d"),
            CallKind::ThreadIndexWitness
        );
        assert_eq!(
            classify_call("cuda_device::thread::threadIdx_x"),
            CallKind::ThreadIndexWitness
        );
        // The public unreachable!-stub is NOT the rewritten intrinsic; the
        // macro rewrites calls, so treating the stub as Other is safe and
        // avoids misclassifying same-named user helpers re-exported paths.
        assert_eq!(
            classify_call("cuda_device::thread::index_1d"),
            CallKind::Other
        );
    }

    #[test]
    fn classifies_count_ones_with_its_operand_width() {
        for (path, bits) in [
            ("core::num::<impl u8>::count_ones", 8),
            ("core::num::<impl u32>::count_ones", 32),
            ("core::num::<impl i32>::count_ones", 32),
            ("core::num::<impl u64>::count_ones", 64),
            ("core::num::<impl u128>::count_ones", 128),
        ] {
            assert_eq!(classify_call(path), CallKind::CountOnes { bits }, "{path}");
        }
        assert_eq!(
            classify_call("cuda_device::warp::lanemask_lt"),
            CallKind::DivergentEnvRead
        );
    }

    /// `count_ones` is a method name, not a reserved word: only the
    /// primitive-integer inherent impls are the intrinsic. Anything else
    /// takes a receiver as its first argument, so evaluating it as a
    /// popcount would count the bits of a pointer.
    #[test]
    fn count_ones_elsewhere_is_not_the_integer_intrinsic() {
        for path in [
            "bitvec::slice::BitSlice::<T, O>::count_ones",
            "roaring::RoaringBitmap::count_ones",
            "my_app::occupancy::Histogram::count_ones",
            "core::num::<impl usize>::count_ones",
            "core::num::<impl isize>::count_ones",
            "core::num::<impl u32>::count_zeros",
        ] {
            assert_eq!(classify_call(path), CallKind::Other, "{path}");
        }
    }

    /// A barrier's participant set, which is what decides whether a
    /// block-uniform guard is enough (#133).
    #[test]
    fn barrier_scope_names_the_participant_set() {
        use reconverge_core::LaunchScope;

        // Block-wide: the default, and what an unrecognized path gets.
        for path in [
            "cuda_device::thread::sync_threads",
            "cuda_device::cooperative_groups::block_reduce",
            "cuda_device::something::unheard_of",
        ] {
            assert_eq!(barrier_scope(path, None), LaunchScope::Block, "{path}");
        }

        // Cluster-wide: the safe wrapper and the raw waiting half. The
        // arrival half is not a barrier at all (#132), so it is never asked.
        for path in [
            "cuda_device::cluster::cluster_sync",
            "cuda_device::cluster::barrier_cluster_wait",
            "cuda_device::cluster::barrier_cluster_wait_aligned",
        ] {
            assert_eq!(barrier_scope(path, None), LaunchScope::Cluster, "{path}");
        }

        assert_eq!(
            barrier_scope("cuda_device::grid::sync", None),
            LaunchScope::Grid
        );

        // `ThreadGroup::sync` is one path for three scopes, told apart by
        // receiver exactly as its classification is.
        let sync = "cuda_device::cooperative_groups::ThreadGroup::sync";
        for (receiver, want) in [
            (
                "cuda_device::cooperative_groups::ThreadBlock",
                LaunchScope::Block,
            ),
            (
                "cuda_device::cooperative_groups::Cluster",
                LaunchScope::Cluster,
            ),
            ("cuda_device::cooperative_groups::Grid", LaunchScope::Grid),
        ] {
            assert_eq!(barrier_scope(sync, Some(receiver)), want, "{receiver}");
        }
        // Without a receiver the path cannot say, and the narrow answer
        // reports nothing extra.
        assert_eq!(barrier_scope(sync, None), LaunchScope::Block);
    }

    #[test]
    fn classifies_uniform_sources_and_barrier() {
        assert_eq!(
            classify_call("cuda_device::thread::blockIdx_x"),
            CallKind::BlockUniform
        );
        // Launch geometry is the same on every block, so it decides a
        // barrier at any scope; `blockIdx` above is not.
        assert_eq!(
            classify_call("cuda_device::thread::blockDim_x"),
            CallKind::GridUniform
        );
        assert_eq!(
            classify_call("cuda_device::sync_threads"),
            CallKind::Barrier
        );
        assert_eq!(
            classify_call("cuda_device::thread::sync_threads"),
            CallKind::Barrier
        );
    }

    #[test]
    fn classifies_warp_collectives_and_atomics() {
        assert_eq!(
            classify_call("cuda_device::warp::ballot_sync"),
            CallKind::WarpCollective {
                mask: MaskSource::FirstArgument
            }
        );
        assert_eq!(
            classify_call("cuda_device::atomic::atomic_add"),
            CallKind::AtomicRmw
        );
    }

    #[test]
    fn classifies_the_full_masked_sync_surface() {
        // The names cuda-device actually exports at the pinned rev (its
        // `warp` module): `shuffle_*_sync` in every width, the match and
        // redux families, election, and the warp barrier. The historical
        // CUDA C spellings (`shfl_sync`, `activemask`) do not exist in the
        // Rust API and must NOT be matched — a name that matches nothing
        // is a silent recall hole.
        for name in [
            "shuffle_sync",
            "shuffle_up_sync",
            "shuffle_down_sync",
            "shuffle_xor_sync",
            "shuffle_f32_sync",
            "shuffle_up_f32_sync",
            "shuffle_down_f32_sync",
            "shuffle_xor_f32_sync",
            "shuffle_u64_sync",
            "shuffle_up_u64_sync",
            "shuffle_down_u64_sync",
            "shuffle_xor_u64_sync",
            "shuffle_f64_sync",
            "shuffle_up_f64_sync",
            "shuffle_down_f64_sync",
            "shuffle_xor_f64_sync",
            "match_any_sync",
            "match_any_i64_sync",
            "match_all_sync",
            "match_all_i64_sync",
            "redux_sync_add",
            "redux_sync_and",
            "redux_sync_or",
            "redux_sync_xor",
            "redux_sync_min_u32",
            "redux_sync_min_i32",
            "redux_sync_max_u32",
            "redux_sync_max_i32",
            "redux_sync_min_f32",
            "redux_sync_max_f32",
            "redux_sync_min_abs_f32",
            "redux_sync_max_abs_f32",
            "redux_sync_min_nan_f32",
            "redux_sync_max_nan_f32",
            "redux_sync_min_abs_nan_f32",
            "redux_sync_max_abs_nan_f32",
            "elect_sync",
            "is_elected_sync",
            "sync_mask",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::warp::{name}")),
                CallKind::WarpCollective {
                    mask: MaskSource::FirstArgument
                },
                "{name} must be a warp collective"
            );
        }
        for dead in ["shfl_sync", "shfl_down_sync", "activemask"] {
            assert_eq!(
                classify_call(&format!("cuda_device::warp::{dead}")),
                CallKind::Other,
                "{dead} does not exist in cuda-device"
            );
        }
    }

    #[test]
    fn active_mask_is_divergent_but_never_a_collective() {
        assert_eq!(
            classify_call("cuda_device::warp::active_mask"),
            CallKind::DivergentEnvRead
        );
    }

    #[test]
    fn classifies_every_all_threads_barrier() {
        // Block, cluster, and grid scope: all three deadlock when reached
        // divergently, and all three must be RC001's subject.
        assert_eq!(
            classify_call("cuda_device::cluster::cluster_sync"),
            CallKind::Barrier
        );
        assert_eq!(classify_call("cuda_device::grid::sync"), CallKind::Barrier);
        // `sync` is a barrier only in the grid module — the bare name is
        // too generic to match anywhere else.
        assert_eq!(classify_call("cuda_device::foo::sync"), CallKind::Other);
        // The mbarrier arrive/wait family is a phase-counted split barrier
        // where partial participation is the designed use: deliberately
        // outside RC001 (explain/RC001.md documents the boundary).
        assert_eq!(
            classify_call("cuda_device::barrier::Barrier::wait"),
            CallKind::Other
        );
    }

    #[test]
    fn lane_environment_reads_are_divergent_sources() {
        // The lanemask registers differ on every lane; warp_id and
        // live_lanes_1d are warp-uniform, which the lattice must treat as
        // divergent (it does not distinguish warp- from block-uniformity).
        // None is a collective and none is replay-evaluable yet.
        for name in [
            "lanemask_lt",
            "lanemask_le",
            "lanemask_eq",
            "lanemask_ge",
            "lanemask_gt",
            "warp_id",
            "live_lanes_1d",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::warp::{name}")),
                CallKind::DivergentEnvRead,
                "{name} must be a divergent environment read"
            );
        }
    }

    /// The unmasked wrappers are collectives whose mask is known from the
    /// call. Each delegates to its `*_sync` counterpart with `u32::MAX`,
    /// verified against cuda-device at the pinned rev, so the wrapper — not
    /// a callee the analysis cannot see — is what supplies the mask.
    ///
    /// This replaces a test asserting they were `Other`. Its reasoning was
    /// that their first argument is not a mask, so classifying them would
    /// corrupt mask reasoning, and they should stay out "until the dialect
    /// can carry an implicit-mask convention". `implicit_full_mask` is that
    /// convention.
    #[test]
    fn unmasked_wrappers_carry_an_implicit_full_mask() {
        for name in [
            "shuffle",
            "shuffle_down",
            "shuffle_down_f32",
            "ballot",
            "all",
            "any",
            "popc",
            "reduce_sum_f32",
            "warp_reduce_sum",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::warp::{name}")),
                CallKind::WarpCollective {
                    mask: MaskSource::ImplicitFull
                },
                "{name} is a collective with a full mask"
            );
        }
    }

    /// The wrapper names are only collectives under `::warp::`. `all` and
    /// `any` are ordinary words, and a false positive here would invent a
    /// collective where the program has none.
    /// `ThreadGroup::sync` is one path for five barriers; the receiver
    /// decides. Block, grid and cluster groups are RC001's subject; a tile
    /// or the coalesced set is a partial-participation contract left as
    /// `Other` (conformance/SURFACE_ALLOW), and no receiver is no decision.
    #[test]
    fn thread_group_sync_is_a_barrier_by_receiver() {
        let sync = "cuda_device::cooperative_groups::ThreadGroup::sync";
        for group in ["ThreadBlock", "Grid", "Cluster"] {
            let receiver = format!("cuda_device::cooperative_groups::{group}");
            assert_eq!(
                classify_method_call(sync, Some(&receiver)),
                CallKind::Barrier,
                "{group}::sync is a scope-wide barrier"
            );
        }
        for group in ["WarpTile<16>", "WarpTile<32>", "CoalescedThreads"] {
            let receiver = format!("cuda_device::cooperative_groups::{group}");
            assert_eq!(
                classify_method_call(sync, Some(&receiver)),
                CallKind::Other,
                "{group}::sync is tile-scoped and deliberately unmodelled"
            );
        }
        assert_eq!(classify_method_call(sync, None), CallKind::Other);
        // A foreign trait with the same method name is nobody's barrier.
        assert_eq!(
            classify_method_call(
                "my_crate::ThreadGroup::sync",
                Some("cuda_device::cooperative_groups::ThreadBlock")
            ),
            CallKind::Other
        );
        // The receiver never changes a free function's classification.
        assert_eq!(
            classify_method_call(
                "cuda_device::thread::sync_threads",
                Some("cuda_device::cooperative_groups::WarpTile<32>")
            ),
            CallKind::Barrier
        );
    }

    /// The generated special-register readers and the raw cluster barrier:
    /// scanned by scripts/check-surface.sh through the modules' `include!`s.
    #[test]
    fn generated_registers_and_the_raw_cluster_barrier_are_classified() {
        // Which SM this block landed on is a block fact, and it differs
        // between blocks.
        assert_eq!(
            classify_call("cuda_device::thread::smid"),
            CallKind::BlockUniform,
            "smid is the same on every thread of the block, and differs on the next block"
        );
        // The rest are launch-wide: how many SMs the device has, the grid's
        // id, the warp-slot count, and the launch environment.
        for (module, name) in [
            ("thread", "nsmid"),
            ("thread", "gridid"),
            ("warp", "nwarpid"),
            ("grid", "envreg1"),
            ("grid", "envreg2"),
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::{module}::{name}")),
                CallKind::GridUniform,
                "{name} reads the same on every thread of the launch"
            );
        }
        assert_eq!(
            classify_call("cuda_device::warp::warpid"),
            CallKind::DivergentEnvRead,
            "the hardware warp slot differs per warp"
        );
        // The raw cluster barrier is split: only the waiting half blocks.
        for name in ["barrier_cluster_wait", "barrier_cluster_wait_aligned"] {
            assert_eq!(
                classify_call(&format!("cuda_device::cluster::{name}")),
                CallKind::Barrier,
                "{name} blocks until the cluster has arrived"
            );
        }
        // The arrival half signals and returns. 0.6.0 called it a barrier and
        // reported a legal split arrival -- one warp through the aligned
        // form, its sibling through the plain one -- as two confirmed
        // deadlocks (#132). Allowlisted with the mbarrier family, and
        // explain/RC001.md says what that costs.
        for name in [
            "barrier_cluster_arrive",
            "barrier_cluster_arrive_aligned",
            "barrier_cluster_arrive_relaxed",
            "barrier_cluster_arrive_relaxed_aligned",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::cluster::{name}")),
                CallKind::Other,
                "{name} is a non-blocking arrival, not a barrier"
            );
        }
        // The counted CTA barrier names its participants, so partial
        // participation is its design: allowlisted, not a barrier here.
        assert_eq!(
            classify_call("cuda_device::barrier::barrier_cta_sync"),
            CallKind::Other
        );
        // Foreign lookalikes are nobody's register.
        assert_eq!(classify_call("my_crate::thread::smid"), CallKind::Other);
    }

    /// The block helpers carry a `sync_threads` inside; the cluster reads
    /// are block-uniform; the cluster marker is a marker.
    #[test]
    fn cooperative_block_helpers_and_cluster_reads_are_classified() {
        for name in ["block_reduce", "block_scan"] {
            assert_eq!(
                classify_call(&format!("cuda_device::cooperative_groups::{name}")),
                CallKind::Barrier,
                "{name} is a block-wide barrier one helper deeper"
            );
            assert_eq!(
                classify_call(&format!("my_crate::{name}")),
                CallKind::Other,
                "{name} outside cuda_device is nobody's barrier"
            );
        }
        for name in [
            "warp_reduce",
            "warp_scan",
            "coalesced_threads",
            "this_thread_block",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::cooperative_groups::{name}")),
                CallKind::Other,
                "{name} is allowlisted, not classified"
            );
        }
        // Split by how far each is actually constant (#133). The block's
        // rank and its cluster coordinates are block facts that differ
        // across the cluster -- exactly the values that decide a
        // `cluster_sync` for some blocks and not others.
        for name in [
            "block_rank",
            "cluster_ctaidX",
            "cluster_ctaidY",
            "cluster_ctaidZ",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::cluster::{name}")),
                CallKind::BlockUniform,
                "{name} is a block fact that differs across the cluster"
            );
        }
        assert_eq!(
            classify_call("cuda_device::cluster::cluster_idx"),
            CallKind::ClusterUniform,
            "cluster_idx is the same on every block of the cluster"
        );
        for name in [
            "cluster_size",
            "num_clusters",
            "cluster_nctaidX",
            "cluster_nctaidY",
            "cluster_nctaidZ",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::cluster::{name}")),
                CallKind::GridUniform,
                "{name} is launch geometry, the same everywhere"
            );
        }
        assert_eq!(
            classify_call("cuda_device::cluster::__cluster_config"),
            CallKind::UniformMarker
        );
    }

    #[test]
    fn wrapper_names_outside_warp_are_not_collectives() {
        for path in [
            "cuda_device::cooperative::all",
            "my_app::iter::any",
            "core::slice::<impl [T]>::all",
        ] {
            assert_eq!(classify_call(path), CallKind::Other, "{path}");
        }
    }

    #[test]
    fn foreign_lookalikes_are_other() {
        assert_eq!(classify_call("my_crate::sync_threads"), CallKind::Other);
        assert_eq!(
            classify_call("my_crate::thread::threadIdx_x"),
            CallKind::Other
        );
        assert_eq!(
            classify_call("cuda_device::__internal::make_kernel_scope"),
            CallKind::UniformMarker
        );
    }
}

#[cfg(test)]
mod partial_reducer_tests {
    use super::*;

    /// The partial-warp reducers are collectives, but their mask is built
    /// from a runtime `live_lanes` argument — neither full nor the first
    /// argument. Calling them full would claim every lane participates in
    /// a reduction deliberately scoped to fewer, so the mask is unknown
    /// and RC002 reports it as such instead of guessing.
    #[test]
    fn partial_reducers_are_collectives_with_an_unknown_mask() {
        for name in [
            "reduce_sum_f32_partial",
            "reduce_sum_f64_partial",
            "reduce_max_f32_partial",
            "reduce_min_f64_partial",
        ] {
            let kind = classify_call(&format!("cuda_device::warp::{name}"));
            assert_eq!(
                kind,
                CallKind::WarpCollective {
                    mask: MaskSource::Unknown
                },
                "{name}"
            );
            assert!(kind.mask_is_unknown(), "{name} must not claim a mask");
            assert_eq!(kind.implicit_mask(), None, "{name}");
        }
    }
}
