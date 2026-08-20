//! The dialect hedge (docs/ARCHITECTURE.md): everything the engine needs to know
//! about a concrete SIMT surface, expressed as one trait over definition
//! paths. cuda-oxide is the first implementation
//! (`reconverge-dialect-oxide`); rust-gpu or CubeCL would be new impls,
//! with zero engine changes.

/// A concrete SIMT dialect: classifies call targets by definition path.
pub trait SimtDialect {
    /// Classify a callee. `def_path` is the fully qualified definition
    /// path as the compiler reports it (e.g.
    /// `cuda_device::thread::__internal::index_1d`).
    fn classify_call(&self, def_path: &str) -> CallKind;
}

/// What a call means to the uniformity engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CallKind {
    /// Mints a per-thread index witness (`index_1d`, `threadIdx_x`, lane
    /// id, …): the result is divergent by definition.
    ThreadIndexWitness,
    /// Atomic read-modify-write returning the old value: divergent.
    AtomicRmw,
    /// Uniform within a block (`block_idx`, block/grid dimensions):
    /// the result is uniform.
    BlockUniform,
    /// Dialect plumbing with uniform, side-effect-free results
    /// (launch-scope constructors, config markers).
    UniformMarker,
    /// A block-wide execution barrier (`sync_threads`): RC001's subject.
    Barrier,
    /// A warp collective (`shuffle_sync`, `ballot_sync`, …): RC002's
    /// subject. Results are treated as divergent (they are at most
    /// warp-uniform, and the lattice does not distinguish warp- from
    /// block-uniformity).
    WarpCollective,
    /// Reads the per-lane execution environment (`active_mask`): the
    /// result depends on which lanes are active, so it is divergent for
    /// the lattice — but the call takes no participation mask and
    /// synchronizes nothing, so it is never RC002's subject and never a
    /// synchronization point for the witness interpreter.
    DivergentEnvRead,
    /// Reads a thread-index witness back out (`ThreadIndex::get`): for the
    /// dataflow it joins its arguments like any call; for the witness
    /// interpreter it is the identity on its first argument.
    WitnessRead,
    /// Bit population count on a primitive integer (`count_ones`), with
    /// the operand's width in bits. For the dataflow it joins its
    /// arguments like any call; for the witness interpreter it is the
    /// population count of its first argument.
    ///
    /// The width is part of the classification because a popcount is
    /// meaningless without it: the interpreter's store is an untyped
    /// `u128`, and unchecked arithmetic wraps at 128 bits rather than at
    /// the source type's width, so a value carrying bits above `bits` is
    /// not one the program ever held. Counting those bits would be a
    /// confident wrong answer, so the interpreter declines instead —
    /// the same discipline as [`crate::model::Eval::CheckedBinary`].
    CountOnes { bits: u32 },
    /// Anything else: the result joins the arguments' uniformities.
    ///
    /// This is deliberately optimistic about *value* flow — a callee could
    /// in principle manufacture divergence from uniform arguments (e.g. by
    /// reading shared memory written by other threads). Barrier and
    /// warp-op *effects* of local callees are covered separately by the
    /// interprocedural summary bits; exotic value-flow through callees is
    /// a documented v1 recall gap, never a precision gap.
    Other,
}

impl CallKind {
    /// The uniformity contributed by the call itself, before joining
    /// argument uniformities. `None` means "just join the arguments".
    #[must_use]
    pub fn result_base(self) -> Option<crate::Uniformity> {
        match self {
            CallKind::ThreadIndexWitness
            | CallKind::AtomicRmw
            | CallKind::WarpCollective
            | CallKind::DivergentEnvRead => Some(crate::Uniformity::Divergent),
            CallKind::BlockUniform | CallKind::UniformMarker => Some(crate::Uniformity::Uniform),
            CallKind::Barrier
            | CallKind::WitnessRead
            | CallKind::CountOnes { .. }
            | CallKind::Other => None,
        }
    }
}
