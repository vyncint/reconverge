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

    /// Classify a callee whose meaning depends on the receiver type — a
    /// trait method such as `cooperative_groups::ThreadGroup::sync`, whose
    /// definition path names the trait and not the group it is called on.
    /// `receiver` is the rendered `Self` type when the compiler exposes one.
    ///
    /// The default ignores the receiver and defers to
    /// [`classify_call`](Self::classify_call), so a dialect that has no
    /// receiver-dependent primitives need not implement it.
    fn classify_method_call(&self, def_path: &str, receiver: Option<&str>) -> CallKind {
        let _ = receiver;
        self.classify_call(def_path)
    }

    /// How far a barrier's participant set reaches.
    ///
    /// Only meaningful for a callee this dialect classified as
    /// [`CallKind::Barrier`]; the engine asks for nothing else. The default
    /// is [`LaunchScope::Block`](crate::LaunchScope::Block), which is what
    /// `sync_threads` is and what a dialect with no wider barrier should
    /// keep — reporting a block barrier as cluster-wide would invent a
    /// hazard, and a dialect that says nothing gets the narrow answer.
    fn barrier_scope(&self, def_path: &str, receiver: Option<&str>) -> crate::LaunchScope {
        let _ = (def_path, receiver);
        crate::LaunchScope::Block
    }
}

/// Where a warp collective's participation mask comes from.
///
/// cuda-device offers the same collective three ways, and they are not
/// interchangeable for RC002: reading the first argument of a call that
/// has no mask argument would check the wrong value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaskSource {
    /// The mask is the call's first argument — the `*_sync` surface.
    FirstArgument,
    /// The call supplies `u32::MAX` itself: the unmasked convenience
    /// wrappers (`ballot`, `shuffle*`, `all`, `any`, `popc`, `reduce_*`),
    /// each of which delegates to its `*_sync` counterpart with a full
    /// mask. Known from the call rather than inferred from the callee.
    ImplicitFull,
    /// A collective whose mask the call does not reveal — the
    /// `reduce_*_partial` helpers, which build one from a runtime
    /// `live_lanes` argument. Still worth classifying: a warning that says
    /// "found, mask not evaluable" beats silence, and assuming a full mask
    /// here would be a confident wrong answer.
    Unknown,
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
    /// Uniform within a block, and **not** guaranteed beyond it
    /// (`block_idx`, `cluster::block_rank`): every thread of one block
    /// agrees, the next block over may not. Enough for `sync_threads`,
    /// not enough to decide a cluster- or grid-wide barrier — see
    /// [`crate::LaunchScope`].
    BlockUniform,
    /// Uniform within a cluster (`cluster::cluster_idx`): every block of
    /// one cluster agrees, the next cluster may not.
    ClusterUniform,
    /// Uniform across the whole launch (`blockDim`, `gridDim`,
    /// `cluster_size`, the launch environment registers): the same value
    /// on every thread of every block, so it decides a barrier at any
    /// scope.
    GridUniform,
    /// Dialect plumbing with uniform, side-effect-free results
    /// (launch-scope constructors, config markers).
    UniformMarker,
    /// A block-wide execution barrier (`sync_threads`): RC001's subject.
    Barrier,
    /// A warp collective (`shuffle_sync`, `ballot_sync`, …): RC002's
    /// subject. Results are treated as divergent (they are at most
    /// warp-uniform, and the lattice does not distinguish warp- from
    /// block-uniformity).
    WarpCollective {
        /// Where the participation mask comes from. It is a property of
        /// the *call*, not of a callee the analysis can see inside.
        mask: MaskSource,
    },
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
    CountOnes {
        /// Width in bits of the argument's integer type.
        bits: u32,
    },
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
    /// Whether this call is a warp collective, whichever way its
    /// participation mask is supplied.
    #[must_use]
    pub fn is_warp_collective(self) -> bool {
        matches!(self, CallKind::WarpCollective { .. })
    }

    /// The mask this call supplies by construction, when it supplies one.
    #[must_use]
    pub fn implicit_mask(self) -> Option<u64> {
        match self {
            CallKind::WarpCollective {
                mask: MaskSource::ImplicitFull,
            } => Some(u64::from(u32::MAX)),
            _ => None,
        }
    }

    /// True when the mask cannot be read off the call at all.
    #[must_use]
    pub fn mask_is_unknown(self) -> bool {
        matches!(
            self,
            CallKind::WarpCollective {
                mask: MaskSource::Unknown
            }
        )
    }

    /// The uniformity contributed by the call itself, before joining
    /// argument uniformities. `None` means "just join the arguments".
    #[must_use]
    pub fn result_base(self) -> Option<crate::Uniformity> {
        match self {
            CallKind::ThreadIndexWitness
            | CallKind::AtomicRmw
            | CallKind::WarpCollective { .. }
            | CallKind::DivergentEnvRead => Some(crate::Uniformity::Divergent),
            CallKind::BlockUniform
            | CallKind::ClusterUniform
            | CallKind::GridUniform
            | CallKind::UniformMarker => Some(crate::Uniformity::Uniform),
            CallKind::Barrier
            | CallKind::WitnessRead
            | CallKind::CountOnes { .. }
            | CallKind::Other => None,
        }
    }

    /// The widest scope this call's own result is constant over, before
    /// meeting the arguments'. `None` means "just meet the arguments".
    ///
    /// Only the reads that *narrow* the scope answer here. Everything else
    /// — a kernel argument, a constant, an ordinary computation — starts at
    /// [`LaunchScope::Grid`](crate::LaunchScope::Grid) and is narrowed only
    /// by what it reads, which is what keeps `if n > 0 { grid::sync() }` on
    /// a launch argument from being reported.
    #[must_use]
    pub fn result_scope(self) -> Option<crate::LaunchScope> {
        match self {
            // Per-lane reads are narrower than a block, but the lane
            // lattice has already called them divergent; `Block` is the
            // narrowest this axis expresses and the divergence check fires
            // first either way.
            CallKind::ThreadIndexWitness
            | CallKind::AtomicRmw
            | CallKind::WarpCollective { .. }
            | CallKind::DivergentEnvRead
            | CallKind::BlockUniform => Some(crate::LaunchScope::Block),
            CallKind::ClusterUniform => Some(crate::LaunchScope::Cluster),
            CallKind::GridUniform | CallKind::UniformMarker => Some(crate::LaunchScope::Grid),
            CallKind::Barrier
            | CallKind::WitnessRead
            | CallKind::CountOnes { .. }
            | CallKind::Other => None,
        }
    }
}
