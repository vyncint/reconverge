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
