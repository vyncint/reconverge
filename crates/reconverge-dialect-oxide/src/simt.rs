//! The `SimtDialect` implementation for cuda-oxide: call classification by
//! definition path, verified against cuda-device at the pinned rev.
//! Path matching only — no upstream code is vendored.

use reconverge_core::dialect::{CallKind, SimtDialect};

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

        // The block-wide execution barrier (RC001's subject).
        "sync_threads" => CallKind::Barrier,

        // Warp collectives (RC002's subject): cuda-device's masked `*_sync`
        // surface, every one taking the participation mask as its first
        // argument, plus `sync_mask` — the warp barrier, whose mask carries
        // the same contract. The unmasked convenience wrappers (`shuffle`,
        // `ballot`, `all`, `any`, `reduce_*`, …) hide the collective — and
        // an implicit full mask — inside cuda-device, where the analysis
        // cannot see the mask argument; they are a documented v1 recall gap
        // (explain/RC002.md), never misread as mask-first calls.
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
        | "sync_mask" => CallKind::WarpCollective,

        // Which lanes are currently active: divergent by definition, but
        // not a collective — it takes no mask, synchronizes nothing, and is
        // legal (indeed designed) to call under divergence.
        "active_mask" => CallKind::DivergentEnvRead,

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
            CallKind::WarpCollective
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
                CallKind::WarpCollective,
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
    fn unmasked_wrappers_are_the_documented_v1_gap() {
        // `shuffle`/`ballot`/`all`/`any` (and the typed non-`_sync`
        // variants) pass an implicit full mask inside cuda-device. Their
        // first argument is NOT a mask, so classifying them as collectives
        // would corrupt mask reasoning; they stay Other until the dialect
        // can carry an implicit-mask convention.
        for name in [
            "shuffle",
            "shuffle_down",
            "shuffle_down_f32",
            "ballot",
            "all",
            "any",
        ] {
            assert_eq!(
                classify_call(&format!("cuda_device::warp::{name}")),
                CallKind::Other,
                "{name} is outside the v1 surface"
            );
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
