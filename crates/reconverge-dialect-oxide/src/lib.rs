//! The cuda-oxide dialect surface: item paths and intrinsic semantics.
//!
//! Recognizes upstream APIs by path matching (the way Clippy recognizes
//! `Option::unwrap`) — no upstream code is vendored, and nothing here
//! links, invokes, or parses any GPU vendor SDK component. Warp semantics
//! come from public documentation.
//!
//! Layout: [`paths`] recognizes items (kernels, index intrinsics, shared
//! memory, launch contracts), [`cc`] holds the compute-capability capacity
//! table, and [`simt`] is the [`reconverge_core::dialect::SimtDialect`]
//! implementation that classifies every call the engine sees.

#![forbid(unsafe_code)]

pub mod cc;
pub mod paths;
pub mod simt;

/// Name of the dialect as reported in diagnostics and artifacts.
pub const DIALECT_NAME: &str = "cuda-oxide";

/// Item-name prefix the `#[kernel]` macro gives the generated kernel
/// function (current naming contract).
///
/// The embedded `246e25db` component is upstream's collision hash
/// (`sha256("cuda_oxide_" + "rust")` truncated), which makes an accidental
/// user-written match effectively impossible — and upstream's macro rejects
/// user identifiers under its reserved `cuda_oxide_` root outright.
pub const KERNEL_SYMBOL_PREFIX: &str = "cuda_oxide_codegen_v1_cuda_oxide_kernel_246e25db_";

/// Kernel item-name prefix used by earlier cuda-oxide versions.
pub const LEGACY_KERNEL_SYMBOL_PREFIX: &str = "cuda_oxide_kernel_246e25db_";

/// Recognize a cuda-oxide kernel item and return the user-facing kernel name.
///
/// The `#[kernel]` proc macro consumes the attribute and re-emits the user's
/// function renamed to `<prefix><name>` with `#[unsafe(no_mangle)]`, so the
/// renamed item is simultaneously the marker the macro leaves behind and the
/// linker symbol. Recognition is by path matching against upstream's naming
/// contract (its `reserved-oxide-symbols` crate), the way Clippy recognizes
/// `Option::unwrap` — no upstream code is vendored.
///
/// Accepts fully qualified item paths (`krate::mod::cuda_oxide_…_foo`) as
/// returned by Stable MIR, and bare item names. The current prefix is tried
/// before the legacy one.
pub fn kernel_base_name(item_path: &str) -> Option<&str> {
    let last = item_path.rsplit("::").next().unwrap_or(item_path);
    let base = last
        .strip_prefix(KERNEL_SYMBOL_PREFIX)
        .or_else(|| last.strip_prefix(LEGACY_KERNEL_SYMBOL_PREFIX))?;
    (!base.is_empty()).then_some(base)
}

#[cfg(test)]
mod tests {
    use reconverge_core::Uniformity;

    use super::kernel_base_name;

    #[test]
    fn dialect_links_against_the_engine() {
        assert_eq!(super::DIALECT_NAME, "cuda-oxide");
        assert_eq!(
            Uniformity::Uniform.join(Uniformity::Divergent),
            Uniformity::Divergent
        );
    }

    #[test]
    fn recognizes_current_kernel_names() {
        assert_eq!(
            kernel_base_name(
                "sample::kernels::cuda_oxide_codegen_v1_cuda_oxide_kernel_246e25db_scale"
            ),
            Some("scale")
        );
        assert_eq!(
            kernel_base_name("cuda_oxide_codegen_v1_cuda_oxide_kernel_246e25db_scale"),
            Some("scale")
        );
    }

    #[test]
    fn recognizes_legacy_kernel_names() {
        assert_eq!(
            kernel_base_name("krate::cuda_oxide_kernel_246e25db_add"),
            Some("add")
        );
    }

    #[test]
    fn current_prefix_is_not_misread_as_legacy() {
        // The current prefix embeds the legacy prefix as a substring; the
        // base name must come from stripping the full current prefix, never
        // the embedded legacy one.
        let name = "cuda_oxide_codegen_v1_cuda_oxide_kernel_246e25db_foo";
        assert_eq!(super::kernel_base_name(name), Some("foo"));
        assert!(name.contains(super::LEGACY_KERNEL_SYMBOL_PREFIX));
    }

    #[test]
    fn rejects_non_kernel_names() {
        assert_eq!(kernel_base_name("sample::ordinary_function"), None);
        // Prefix must start the final path segment, not merely occur in it.
        assert_eq!(
            kernel_base_name("sample::x_cuda_oxide_kernel_246e25db_y"),
            None
        );
        // A bare prefix with no base name is not a kernel.
        assert_eq!(
            kernel_base_name("cuda_oxide_codegen_v1_cuda_oxide_kernel_246e25db_"),
            None
        );
    }
}
