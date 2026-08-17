//! Path recognizers for the cuda-oxide device API.
//!
//! Everything here works on definition paths as returned by Stable MIR
//! (`CrateDef::name()`), matching the way Clippy recognizes
//! `Option::unwrap`. Verified against cuda-device at the pinned rev; no
//! upstream code is vendored.

/// Thread-index witness functions the `#[kernel]` macro rewrites calls to.
///
/// Only the launch-shape-dependent, `LaunchContext`-generic ones are listed:
/// the proof-carrying `index_1d_u32` / `coord_2d_u32` take a *typed* context
/// (`Domain1`/`Domain2` + `U32Coordinates`), so a contract mismatch there is
/// already a compile error and never a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexFn {
    /// `__internal::index_1d` — X-only formula; thread-unique only for an
    /// effectively 1D launch.
    Index1d,
    /// `__internal::index_2d::<ROW_STRIDE>` — const-stride row-major
    /// formula; thread-unique for launches of at most two axes.
    Index2d,
    /// `__internal::index_2d_runtime` — stores raw `(row, col)` coordinates,
    /// resolved per-slice at the access site; shape-independent.
    Index2dRuntime,
    /// `__internal::warp_index` — flat warp slot; minted for lane 0 only.
    WarpIndex,
}

impl IndexFn {
    /// Human-facing name as users write it (`thread::…()`).
    #[must_use]
    pub fn user_name(self) -> &'static str {
        match self {
            IndexFn::Index1d => "index_1d",
            IndexFn::Index2d => "index_2d",
            IndexFn::Index2dRuntime => "index_2d_runtime",
            IndexFn::WarpIndex => "warp_index",
        }
    }

    /// The largest launch-domain dimensionality under which this formula
    /// still mints thread-unique witnesses without runtime shape checks,
    /// or `None` when the formula is shape-independent.
    #[must_use]
    pub fn max_proven_dimensions(self) -> Option<u8> {
        match self {
            IndexFn::Index1d => Some(1),
            IndexFn::Index2d => Some(2),
            // Coordinates are resolved against the addressed slice, and the
            // warp slot is computed from the full launch geometry: neither
            // depends on a declared launch shape.
            IndexFn::Index2dRuntime | IndexFn::WarpIndex => None,
        }
    }
}

/// Recognize a call target as one of the index witness functions.
pub fn index_fn(def_path: &str) -> Option<IndexFn> {
    if !def_path.starts_with("cuda_device::") {
        return None;
    }
    match def_path.rsplit("::").next()? {
        "index_1d" if def_path.ends_with("::__internal::index_1d") => Some(IndexFn::Index1d),
        "index_2d" if def_path.ends_with("::__internal::index_2d") => Some(IndexFn::Index2d),
        "index_2d_runtime" if def_path.ends_with("::__internal::index_2d_runtime") => {
            Some(IndexFn::Index2dRuntime)
        }
        "warp_index" if def_path.ends_with("::__internal::warp_index") => Some(IndexFn::WarpIndex),
        _ => None,
    }
}

/// Launch domains a kernel can declare via `#[launch_contract(domain = N)]`.
///
/// `Unknown` is what the macro chooses when no contract is declared: index
/// witnesses are then validated at runtime and silently invalidated on a
/// mismatched launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LaunchDomain {
    Unknown,
    D1,
    D2,
    D3,
}

impl LaunchDomain {
    /// Number of launch axes the contract proves, if any.
    #[must_use]
    pub fn dimensions(self) -> Option<u8> {
        match self {
            LaunchDomain::Unknown => None,
            LaunchDomain::D1 => Some(1),
            LaunchDomain::D2 => Some(2),
            LaunchDomain::D3 => Some(3),
        }
    }

    /// How the domain is written in a `#[launch_contract]`.
    #[must_use]
    pub fn contract_syntax(self) -> &'static str {
        match self {
            LaunchDomain::Unknown => "(none)",
            LaunchDomain::D1 => "domain = 1",
            LaunchDomain::D2 => "domain = 2",
            LaunchDomain::D3 => "domain = 3",
        }
    }
}

/// Recognize a launch-domain marker type from its definition path.
pub fn launch_domain(type_path: &str) -> Option<LaunchDomain> {
    if !type_path.starts_with("cuda_device::") {
        return None;
    }
    match type_path.rsplit("::").next()? {
        "UnknownDomain" if type_path.ends_with("::__internal::UnknownDomain") => {
            Some(LaunchDomain::Unknown)
        }
        "Domain1" if type_path.ends_with("::__internal::Domain1") => Some(LaunchDomain::D1),
        "Domain2" if type_path.ends_with("::__internal::Domain2") => Some(LaunchDomain::D2),
        "Domain3" if type_path.ends_with("::__internal::Domain3") => Some(LaunchDomain::D3),
        _ => None,
    }
}

/// Whether an ADT path is `cuda_device`'s block-scoped shared-memory array.
#[must_use]
pub fn is_shared_array(type_path: &str) -> bool {
    type_path.starts_with("cuda_device::") && type_path.ends_with("::SharedArray")
}

/// Whether an ADT path is `cuda_device`'s shared-memory mbarrier.
///
/// Matched on the final segment under the `cuda_device` crate: Stable MIR
/// reports the type by its re-export path (`cuda_device::Barrier`), and the
/// crate defines exactly one `pub struct Barrier` (verified at the pin).
#[must_use]
pub fn is_barrier(type_path: &str) -> bool {
    type_path.starts_with("cuda_device::") && type_path.ends_with("::Barrier")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_index_fns_at_their_internal_paths() {
        assert_eq!(
            index_fn("cuda_device::thread::__internal::index_1d"),
            Some(IndexFn::Index1d)
        );
        assert_eq!(
            index_fn("cuda_device::__internal::index_1d"),
            Some(IndexFn::Index1d)
        );
        assert_eq!(
            index_fn("cuda_device::thread::__internal::index_2d"),
            Some(IndexFn::Index2d)
        );
        assert_eq!(
            index_fn("cuda_device::thread::__internal::index_2d_runtime"),
            Some(IndexFn::Index2dRuntime)
        );
        assert_eq!(
            index_fn("cuda_device::thread::__internal::warp_index"),
            Some(IndexFn::WarpIndex)
        );
    }

    #[test]
    fn rejects_lookalike_index_paths() {
        // The public unreachable!-stub, not the rewritten intrinsic.
        assert_eq!(index_fn("cuda_device::thread::index_1d"), None);
        // Same tail in a foreign crate.
        assert_eq!(index_fn("my_crate::__internal::index_1d"), None);
        // Proof-carrying variants are type-enforced; not classified.
        assert_eq!(
            index_fn("cuda_device::thread::__internal::index_1d_u32"),
            None
        );
    }

    #[test]
    fn recognizes_launch_domains() {
        assert_eq!(
            launch_domain("cuda_device::thread::__internal::UnknownDomain"),
            Some(LaunchDomain::Unknown)
        );
        assert_eq!(
            launch_domain("cuda_device::thread::__internal::Domain2"),
            Some(LaunchDomain::D2)
        );
        assert_eq!(launch_domain("cuda_device::thread::Domain2"), None);
        assert_eq!(LaunchDomain::D2.dimensions(), Some(2));
        assert_eq!(LaunchDomain::Unknown.dimensions(), None);
    }

    #[test]
    fn recognizes_shared_memory_types() {
        assert!(is_shared_array("cuda_device::shared::SharedArray"));
        assert!(is_shared_array("cuda_device::SharedArray"));
        assert!(is_barrier("cuda_device::barrier::Barrier"));
        assert!(is_barrier("cuda_device::Barrier"));
        assert!(!is_shared_array("my_crate::shared::SharedArray"));
        assert!(!is_barrier("my_crate::Barrier"));
        assert!(!is_barrier("cuda_device::BarrierToken"));
    }
}
