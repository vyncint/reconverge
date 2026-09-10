//! Dialect-agnostic divergence analysis engine (MIR uniformity dataflow).
//!
//! The engine consumes a [`dialect::SimtDialect`] (divergence sources,
//! barriers, warp collectives, uniform sources) and computes per-value
//! uniformity with provenance over the [`model`] a driver adapter builds
//! from MIR. It knows nothing about any concrete GPU dialect; CI enforces
//! that it never depends on `reconverge-dialect-oxide`
//! (`scripts/check-isolation.sh`), and it is free of compiler types so it
//! stays unit-testable with hand-built models.
//!
//! Layout: [`model`] is the input IR, [`graph`] the CFG algorithms
//! (dominators, post-dominators, reducibility, divergence regions), and
//! [`analysis`] the fixpoint dataflow (docs/ARCHITECTURE.md) with mandatory
//! provenance and the interprocedural barrier/warp summary bits.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analysis;
pub mod dialect;
pub mod graph;
pub mod inline;
pub mod model;

/// The uniformity lattice: `Uniform ⊑ Divergent`, per SSA value.
///
/// A value is *uniform* when every active lane of a warp holds the same
/// value at the same program point, and *divergent* otherwise. This is a
/// dataflow fact about values, not a timing claim: since Volta (2017),
/// warps do not execute in guaranteed lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Uniformity {
    /// Every active lane observes the same value.
    Uniform,
    /// Lanes may observe different values.
    Divergent,
}

/// How wide a scope a value is constant over, or a barrier's participant
/// set reaches.
///
/// [`Uniformity`] answers a *lane* question — do the threads of one warp
/// agree? — and stops at the block. That is the right question for
/// `sync_threads`, and not the whole question for a barrier whose
/// participants span more than one block: `cluster::block_rank()` is the
/// same on every thread of a block and different on the next block over, so
/// a `cluster_sync()` under a `block_rank()` guard is entered by some blocks
/// of the cluster and skipped by others (#133).
///
/// Ordered narrow to wide. A guard admits a barrier when the guard's scope
/// is at least the barrier's: a `Grid`-scoped value is constant everywhere
/// and guards anything, a `Block`-scoped one guards only a block barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaunchScope {
    /// Constant within one block; may differ between blocks.
    Block,
    /// Constant within one cluster; may differ between clusters.
    Cluster,
    /// Constant across the whole launch — kernel arguments, `blockDim`,
    /// `gridDim`, and anything derived only from those.
    Grid,
}

impl LaunchScope {
    /// Lattice meet: the narrower of two scopes, since a value derived from
    /// both is constant only where both are.
    #[must_use]
    pub fn meet(self, other: Self) -> Self {
        if self <= other { self } else { other }
    }

    /// Does a guard constant over `self` decide a barrier whose
    /// participants span `barrier`?
    #[must_use]
    pub fn admits(self, barrier: Self) -> bool {
        self >= barrier
    }

    /// The word to use in a diagnostic: "block", "cluster", "grid".
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Cluster => "cluster",
            Self::Grid => "grid",
        }
    }
}

impl Uniformity {
    /// Lattice join: divergence is absorbing.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Uniform, Self::Uniform) => Self::Uniform,
            _ => Self::Divergent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Uniformity;

    #[test]
    fn join_is_absorbing_toward_divergent() {
        assert_eq!(
            Uniformity::Uniform.join(Uniformity::Uniform),
            Uniformity::Uniform
        );
        assert_eq!(
            Uniformity::Uniform.join(Uniformity::Divergent),
            Uniformity::Divergent
        );
        assert_eq!(
            Uniformity::Divergent.join(Uniformity::Divergent),
            Uniformity::Divergent
        );
    }
}
