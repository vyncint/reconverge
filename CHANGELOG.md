# Changelog

All notable changes to this project are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); the artifact
schemas in [`schemas/`](schemas/) are versioned independently of the crates.

Every release reports **both** sets of numbers, deliberately: the corpus
figures this project generates for itself, and the findings that came from
real code. Self-made numbers are a proxy and can be gamed by whoever writes
the corpus; found-in-the-wild is the true north.

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

[0.1.3]: https://github.com/vyncint/reconverge/releases/tag/v0.1.3
[0.1.2]: https://github.com/vyncint/reconverge/releases/tag/v0.1.2
[0.1.1]: https://github.com/vyncint/reconverge/releases/tag/v0.1.1
[0.1.0]: https://github.com/vyncint/reconverge/releases/tag/v0.1.0
