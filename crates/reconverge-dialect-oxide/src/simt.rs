//! The `SimtDialect` implementation for cuda-oxide: call classification by
//! definition path, verified against cuda-device at the pinned rev.
//! Path matching only — no upstream code is vendored.

use reconverge_core::dialect::{CallKind, MaskSource, SimtDialect};

/// The cuda-oxide dialect.
#[derive(Debug, Clone, Copy, Default)]
pub struct CudaOxide;

impl SimtDialect for CudaOxide {
    fn classify_call(&self, def_path: &str) -> CallKind {
        classify_call(def_path)
    }
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

        // Uniform within a block (or the whole grid).
        "blockIdx_x" | "blockIdx_y" | "blockIdx_z" | "blockDim_x" | "blockDim_y" | "blockDim_z"
        | "gridDim_x" | "gridDim_y" | "gridDim_z" => CallKind::BlockUniform,

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

        // Warp collectives (RC002's subject): cuda-device's masked `*_sync`
        // surface, every one taking the participation mask as its first
        // argument, plus `sync_mask` — the warp barrier, whose mask carries
        // the same contract. The unmasked convenience wrappers (`shuffle`,
        // `ballot`, `all`, `any`, `reduce_*`, …) hide the collective — and
        // an implicit full mask — inside cuda-device, where the analysis
        // cannot see the mask argument; they are a documented v1 recall gap
        // (explain/RC002.md), never misread as mask-first calls.
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

        // Dialect plumbing with uniform, effect-free results.
        "make_kernel_scope"
        | "__launch_contract_config"
        | "__launch_contract_block_config"
        | "__launch_bounds_config"
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

    #[test]
    fn classifies_uniform_sources_and_barrier() {
        assert_eq!(
            classify_call("cuda_device::thread::blockIdx_x"),
            CallKind::BlockUniform
        );
        assert_eq!(
            classify_call("cuda_device::thread::blockDim_x"),
            CallKind::BlockUniform
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
