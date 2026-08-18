# Changelog

All notable changes to this project are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); the artifact
schemas in [`schemas/`](schemas/) are versioned independently of the crates.

Every release reports **both** sets of numbers, deliberately: the corpus
figures this project generates for itself, and the findings that came from
real code. Self-made numbers are a proxy and can be gamed by whoever writes
the corpus; found-in-the-wild is the true north.

## [0.1.10] — 2026-08-18

Fixes [#12](https://github.com/vyncint/reconverge/issues/12):
documentation only — a report about a stated *reason*, not a behavior,
and the reason is what changes.

### Changed

- The Limitations entry for the lane-environment gap named "truncating
  casts" as missing machinery, but casts on the thread index are
  evaluated today and such guards promote to `confirmed` (the issue's
  reproduction). The entry now says the precise thing: casts are
  evaluated *as the identity* — exact for the small thread-index values
  replays traffic in, wrong for full-width masks — and integer `!` is
  modeled for booleans only; width-typed evaluation of those unchecked
  operations is what `lanemask_*` promotion actually needs.
  Overflow-checked arithmetic has been width-typed since 0.1.8.

## [0.1.9] — 2026-08-18

Fixes [#11](https://github.com/vyncint/reconverge/issues/11):
documentation only — the behavior was measured to be better than the
claim, and the claim is what changes.

### Changed

- `conformance/MUTATION.md` said RC002 v1 "does not do mask arithmetic
  against launch shapes", which read literally predicts a gating finding
  for the correct guarded partial-warp idiom. The replay *does* compare
  the mask against the lanes it finds present — promotion happens exactly
  when a named lane is absent, and a mask naming exactly the arrivals is
  never promoted. The shrinkmask row's explanation now says the true,
  narrower thing: a shrunk mask at a *convergent* site names no absent
  lane, so there is nothing to witness; recall numbers are unchanged.
- The README Limitations now state positively what the replay checks
  (mask versus lanes present, under the one-warp launch it runs), instead
  of leaving the stronger property undocumented.

## [0.1.8] — 2026-08-18

Fixes [#10](https://github.com/vyncint/reconverge/issues/10): a divergent
guard *inside* a loop is witness-promoted like the same guard outside one.

### Fixed

- **Overflow-checked arithmetic evaluates in replays.** Debug builds lower
  `n += 1` to a checked pair (`CheckedBinaryOp` + assert + field read),
  which the interpreter did not model — the counter went unknown after one
  iteration, the loop condition became unknowable, and any site inside the
  loop's cyclic region was abandoned. The adapter now recognizes the whole
  idiom (the pair local, function-wide, excluded if anything else ever
  writes it; the `.0` read; the width from the operand's unsigned type)
  and the interpreter evaluates it **exactly within the type's width,
  yielding unknown past it** — the checked form panics the thread on
  overflow, so a wrapped value never exists in the real program and is
  never fabricated in a replay. Signed and 128-bit operands stay
  unmodeled.

## [0.1.7] — 2026-08-18

Fixes [#9](https://github.com/vyncint/reconverge/issues/9): witness
promotion no longer stops at the first barrier it cannot see past, so a
barrier added *above* a confirmable finding can no longer take it out of
the CI gate silently.

### Fixed

- **Lanes split between the site and an upstream barrier are a mutual
  deadlock, not an abort.** The site's arrived lanes wait forever (a
  barrier site waits for the whole block; a collective site is only
  emitted when its mask names an absent lane), so no upstream barrier can
  ever be satisfied either — the parked lanes provably never arrive, and
  the replay now concludes exactly that instead of declining. A divergent
  barrier below another divergent barrier is witness-confirmed again.
- **`warp_id()` and `live_lanes_1d()` evaluate in replays.** The replay
  always runs one full warp (`block [32,1,1]`, the same shape under which
  `blockDim_x` is already hardcoded), where those two are exactly 0 and
  32\. A `warp_id()`-guarded barrier upstream now releases uniformly
  instead of aborting the replay of everything below it — the issue's
  headline case. Findings *under* such guards still never promote (the
  guard is uniform across the replayed warp, so there is no divergence to
  witness), which keeps the documented tier for lane-environment guards
  intact.
- The per-lane registers (`lanemask_*`, `active_mask`) remain deliberately
  unevaluable — their 32-bit mask values would flow into evaluation that
  is not width-typed, and a wrong value could fabricate a confirmation.
  The Limitations section now also states the upstream-guard consequence,
  as the issue requested for any residual ordering effect.

## [0.1.6] — 2026-08-18

Two more coverage bugs from a second independent end-to-end review, plus
hygiene.

### Fixed

- **RC001 now covers every all-threads barrier, not just the block one.**
  `cluster::cluster_sync()` and `grid::sync()` deadlock exactly like
  `sync_threads()` when reached divergently — upstream's own safety note on
  the cluster barrier says so — but only `sync_threads` was classified, so
  a divergent cluster or grid barrier reported nothing, interprocedurally
  included. All three now classify as barriers (a divergent `cluster_sync`
  is witness-confirmed like any other). The mbarrier arrive/wait family
  stays out *deliberately*: it is a phase-counted split barrier where
  partial participation is the designed use, and the boundary is now
  written down in `--explain RC001` and the README.
- **The lane-environment registers are no longer read as uniform.** The
  `lanemask_*` registers (per-lane by definition — upstream documents
  `lanemask_eq()` as `1 << lane_id()`), `warp_id()`, and `live_lanes_1d()`
  took no arguments, so the lattice defaulted their results to uniform:
  guards built on them marked no divergence, silencing RC001 and RC002
  entirely, and the Inspector labeled per-lane hardware registers uniform.
  All seven now classify as divergent environment reads — findings under
  such guards fire at warning tier. They are not witness-promoted yet:
  giving the replay their exact values needs width-typed evaluation
  (integer `!`, truncating casts), which the interpreter does not have —
  and approximating would risk false confirmations, the one thing this
  tool must never produce. The README's Limitations section states the
  tier honestly.
- The mutation corpus's barrier operators now ask the dialect which calls
  are barriers (as the collectives already did), so cluster and grid
  barrier sites join the wrapbar/delbar classes and can never drift from
  the analyzer.

### Changed

- `reconverge-tui` on a non-TTY now explains that an interactive terminal
  is required and points at `--message-format json` / `--sarif`, instead
  of dying with a bare `os error 6`.
- The README's manual-install path now pins `reconverge-driver` and
  `reconverge-tui` to the CLI's version, matching the guarantee
  `cargo reconverge setup` provides.
- The conformance scripts build the extractor with `--locked`, and the
  extractor's lockfile is refreshed as part of a release bump — previously
  it drifted silently and every conformance run dirtied the tree.

## [0.1.5] — 2026-08-18

Dependency housekeeping; no behavior changes.

- The termlens PTY test harness is now a crates.io dependency (0.3.0)
  instead of a rev-pinned git dependency — the pinned rev was exactly the
  v0.3.0 release commit, so the bits are identical. Dev-dependency only;
  it never ships in the binaries.

## [0.1.4] — 2026-08-18

Release-pipeline change only; the shipped code is identical to 0.1.3.

- Publishing now authenticates to crates.io with [Trusted
  Publishing](https://crates.io/docs/trusted-publishing) (GitHub OIDC): the
  release workflow exchanges a per-run identity token for a ~30-minute
  crates.io token at publish time. No long-lived registry token exists
  anywhere anymore — this release is the end-to-end proof.

## [0.1.3] — 2026-08-18

Three bug fixes, from an independent end-to-end review of 0.1.1.

### Fixed

- **`--cc` changes now actually re-lint.** The `--cc` invalidation (and the
  missing-artifact self-heal) deleted `<build>/.fingerprint`, but cargo
  keeps freshness fingerprints under the *profile* directory
  (`<build>/debug/.fingerprint`), so the deletion hit a path that never
  existed and stale RC004 findings were re-rendered verbatim — reporting
  the first capability ever seen as fact, even when `--cc` was dropped
  entirely. Both sites now sweep the profile directories.
- **Workspaces with a proc-macro member no longer re-drive every run.**
  Findings artifacts are named `findings-<crate>-<crate types>.json` and
  the crate name was split off at the *last* hyphen — but `proc-macro` is a
  crate type with a hyphen in it, so those artifacts never matched a
  member, and the self-heal re-ran the whole wrapped `cargo check` on every
  warm invocation. The name now splits at the first hyphen (crate names
  cannot contain one).
- **RC002 now recognizes the collectives cuda-device actually exports.**
  The dialect matched CUDA C spellings (`shfl_*_sync`, `activemask`) that
  do not exist in the Rust API, so every real shuffle fell through
  unclassified. The classifier now covers the full masked `*_sync` surface
  at the pinned rev — `shuffle_*_sync` in every width, `match_*_sync`,
  `redux_sync_*`, `elect_sync`, `is_elected_sync` — plus `sync_mask`, the
  warp barrier, whose mask carries the same contract. `active_mask()` is
  classified as a divergent environment read: its result is divergent for
  the lattice, but it is never flagged (no mask, no synchronization, legal
  under divergence). The conformance extractor now asks the dialect itself
  which collectives it classifies, so the mutation corpus can never drift
  from the analyzer again; the unmasked convenience wrappers
  (`warp::shuffle`, `warp::ballot`, the `reduce_*` helpers) remain outside
  v1 and are now documented as such in `--explain RC002`, the masks lesson,
  and the README.
- **`check` works from any directory, on any default toolchain.** The
  wrapped `cargo check` now exports the pinned toolchain (as the CI action
  always did) and resolves the driver's dylib path from that toolchain
  instead of the ambient one, so a kernel crate no longer needs a copy of
  reconverge's `rust-toolchain.toml` just to keep the driver from dying in
  the dynamic linker. When the wrapped build still fails, the error no
  longer claims "build errors" unconditionally — it distinguishes a driver
  that failed to start and points at `cargo reconverge setup`.

## [0.1.2] — 2026-08-17

The crates.io pages, and a one-stop install.

- `cargo reconverge setup`: after `cargo install cargo-reconverge`, one
  command installs the pinned toolchain with the components the driver
  needs, then `reconverge-driver` and `reconverge-tui` at the CLI's own
  version — the three binaries cannot drift apart. Every command is printed
  before it runs, and failures end with the manual steps.
- Every crate now ships a README (the pages on crates.io were blank: the
  repository README sits at the workspace root, which is never packaged),
  plus keywords and categories. The driver's documentation link points at
  the repository, since docs.rs cannot build `rustc_private` crates.
- The bin crates no longer package their integration tests, which need
  sibling binaries and repository fixtures a package cannot carry.
- The driver/TUI not-found errors now tell installed users about `setup`
  instead of suggesting a `cargo build` that only works in a checkout.

## [0.1.1] — 2026-08-17

Packaging only; no behavior changes.

- The explain pages and the learn-mode lessons now live inside the crates
  that embed them (`cargo-reconverge/explain/`, `reconverge-tui/lessons/`)
  rather than in `docs/`. `include_str!` cannot reach outside a package
  directory, so the published crates would not have compiled without this.
  `docs/explain/` and `docs/learn/` remain as indexes.
- The recorded replays the lessons step through are copies of
  `fixtures/witness/`, with a test that fails if the two drift apart.
- Workspace crates carry version requirements on their path dependencies,
  and publishing is enabled.

## [0.1.0] — 2026-08-17

First public release. The analysis, the four terminal views, and the CI
integration are complete and tested; the version is `0.1.x` because nothing
here has met a real user yet, and the verdict wording still awaits
calibration against hardware.

### Analysis

- Uniformity dataflow over Stable MIR behind a dialect trait, with mandatory
  provenance chains from every divergent value back to its source, and
  declared degrades (irreducible CFGs, opaque statements, coverage reported
  next to findings).
- `RC001` divergent barriers and `RC002` non-convergent warp collectives,
  each promoted to `confirmed` when a 32-lane witness interpreter replays a
  concrete hang under a concrete launch configuration — and left at
  `warning` whenever anything the replay needed was unknowable.
- `RC003` (`&mut [T]` kernel parameters), `RC004` (static shared memory over
  the target's limit, with `--cc`), `RC005` (launch-contract inconsistency).

### Interfaces

- `cargo reconverge check` with `--strict`, `--cc`, `--message-format`,
  `--sarif`, `--baseline`, `--show-suppressed`; exit codes 0/1/2.
- `cargo reconverge inspect | witness | learn | triage | watch`, and
  `--explain RCxxx` for every code.
- Four terminal views (uniformity inspector, 32-lane witness debugger,
  SIMT lessons, findings triage) — pure readers of versioned
  artifacts, deterministic frames, `NO_COLOR` and `--ascii` honored.
- `findings.v1`, `unimap.v1`, `witness.v1`, and `baseline.v1` schemas, with
  fixtures acting as their API tests.
- A GitHub Action wrapper, verified on a separate repository in both
  directions: a clean crate passes, injected findings fail the job.

### Numbers

- **Conformance:** zero false positives at default confidence across the
  extracted upstream corpus (143 kernel crates at the pinned commit), gated
  on every CI run.
- **Mutation corpus:** precision **1.000** at default confidence over 513
  compiling mutants; recall published per bug class, including the honest
  zeros, in [`conformance/MUTATION.md`](conformance/MUTATION.md).
- **Found in the wild:** one candidate — a barrier upstream keeps under
  divergent control — reported at `warning` and *not* claimed as confirmed:
  its guard depends on values the interpreter cannot know, so hardware
  evidence comes first.

[0.1.10]: https://github.com/vyncint/reconverge/releases/tag/v0.1.10
[0.1.9]: https://github.com/vyncint/reconverge/releases/tag/v0.1.9
[0.1.8]: https://github.com/vyncint/reconverge/releases/tag/v0.1.8
[0.1.7]: https://github.com/vyncint/reconverge/releases/tag/v0.1.7
[0.1.6]: https://github.com/vyncint/reconverge/releases/tag/v0.1.6
[0.1.5]: https://github.com/vyncint/reconverge/releases/tag/v0.1.5
[0.1.4]: https://github.com/vyncint/reconverge/releases/tag/v0.1.4
[0.1.3]: https://github.com/vyncint/reconverge/releases/tag/v0.1.3
[0.1.2]: https://github.com/vyncint/reconverge/releases/tag/v0.1.2
[0.1.1]: https://github.com/vyncint/reconverge/releases/tag/v0.1.1
[0.1.0]: https://github.com/vyncint/reconverge/releases/tag/v0.1.0
