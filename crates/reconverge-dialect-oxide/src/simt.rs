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

        // Warp collectives (RC002's subject, wired in M3).
        "shfl_sync" | "shfl_up_sync" | "shfl_down_sync" | "shfl_xor_sync" | "ballot_sync"
        | "any_sync" | "all_sync" | "activemask" => CallKind::WarpCollective,

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
