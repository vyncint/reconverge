# Conformance

Upstream's examples are the test corpus: every kernel upstream ships must
come through reconverge with **zero false positives at default confidence**.
That is a requirement, not a goal.

## How it runs

`scripts/run-conformance.sh` (also the CI `conformance` job):

1. Materializes the upstream checkout pinned in [`PIN`](PIN).
2. Runs [`extractor/`](extractor/) — upstream's host half hard-requires the
   CUDA SDK at build time (`cuda-bindings` runs bindgen against `cuda.h`,
   with no SDK-free fallback), and requiring the SDK anywhere is forbidden
   (SECURITY.md). The extractor therefore splices each example's
   `#[cuda_module] mod … { … }` — the analysis surface — verbatim into a
   kernel-only crate depending on `cuda-device` alone. Nothing extracted is
   committed; the corpus regenerates from the pin on every run.
3. Prunes (and counts) crates whose kernels reach host-side items, and
   fails if the surviving corpus drops below the extraction floor.
4. Runs `cargo reconverge check` over the whole corpus and diffs the
   deny/confirmed findings against [`EXPECTED`](EXPECTED):
   - an **extra** finding is a false positive → CI fails;
   - a **missing** finding is a detection regression → CI fails.

Warning-confidence findings are reported but not baselined; they are hidden
by default in the tool and never gate the exit code.

## Updating

- Moving the pin: update `PIN`, rerun, re-review any baseline change, and
  keep the sample-crate pins in `crates/*/tests/*/Cargo.toml` in lockstep.
  If the bump changes analysis behavior, stop and ask (CONTRIBUTING.md).
- Every `EXPECTED` line must carry a review comment explaining why the
  finding is a true positive.

## The mutation corpus

`scripts/run-mutation-corpus.sh` (runs after conformance in the same CI
job) reuses the extracted corpus and the unmutated baseline:

1. [`extractor/`](extractor/)'s `mutate` subcommand mechanically injects
   the labeled bug classes — wrap a barrier in an index-derived `if`,
   delete a barrier, wrap a warp collective, shrink a full mask, swap a
   `DisjointSlice<T>` parameter to `&mut [T]` — one single-site mutant per
   crate, with every skipped site counted (never silently capped).
2. Mutants that do not compile are pruned and counted (a swap into
   slice-only API is expected to fail sometimes).
3. `cargo reconverge check` runs over every compiling mutant; the `score`
   subcommand joins findings against the labels, **measured against the
   unmutated baseline** so pre-existing upstream findings are never
   claimed as catches.
4. Precision at default confidence must be 1.0 — any unattributed
   deny/confirmed finding on a mutant fails CI — and the published
   per-class recall table must match [`MUTATION.md`](MUTATION.md) exactly,
   so every movement is a deliberate, reviewed diff.

The same operators apply to the full upstream examples in single-file
mode, which is how hardware session #2 cross-checks `compute-sanitizer
synccheck` against the static verdicts on identical labeled bugs
(`docs/hardware/session-2.md`).
