# reconverge-core

The dialect-agnostic divergence engine behind [reconverge](https://github.com/vyncint/reconverge):
uniformity as dataflow over a compiler-free model IR.

A value is *uniform* when every active lane of a warp holds the same value
at the same program point, and *divergent* otherwise. The engine computes
that per value (lattice `Uniform ⊑ Divergent`, optimistic fixpoint),
marks the region between a divergent branch and its post-dominator as
divergent control, records a provenance chain from every divergent value
back to its source, and carries per-function summary bits for
interprocedural barrier and collective reporting. Degrades declare
themselves: irreducible control flow becomes all-divergent *and says so*,
and opaque statements are counted as coverage.

It knows nothing about any concrete GPU dialect — divergence sources,
barriers, and collectives arrive through the `SimtDialect` trait
(implemented for cuda-oxide by
[`reconverge-dialect-oxide`](https://crates.io/crates/reconverge-dialect-oxide)) —
and nothing about the compiler, which is why the whole dataflow is
unit-tested on hand-built control-flow graphs.

End users want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge),
the CLI built on top.
