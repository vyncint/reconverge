# Changelog

All notable changes to this project are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); the artifact
schemas in [`schemas/`](schemas/) are versioned independently of the crates.

Every release reports **both** sets of numbers, deliberately: the corpus
figures this project generates for itself, and the findings that came from
real code. Self-made numbers are a proxy and can be gamed by whoever writes
the corpus; found-in-the-wild is the true north.

## [Unreleased]

### Changed

- **termlens 0.3 → 0.6** for the TUI test harness, across `reconverge-tui`
  and `cargo-reconverge`. Three releases of breaking change, of which one
  reached this suite: since 0.5 `send` returns `Result`, so the sixty
  keystrokes the flow tests type are now checked rather than discarded. A
  send that fails means the application stopped reading — precisely the
  failure a TUI test exists to catch, and until now it was dropped on the
  floor and the test failed later, somewhere else, as a timeout.

  Each site names its key, because knowing *which* keystroke was lost is
  most of the diagnosis.

  The upgrade also brings the `openpty` retry from 0.6: macOS recycles PTY
  devices through `revoke()` and refuses a suite that asks faster than the
  kernel returns them, which is what `cargo test` does by default on a
  many-core machine. This suite spawns a PTY per flow test.

## [0.2.0] — 2026-08-20

The milestone that made the witness interpreter tell the truth about full-width
values, and then used that to reach the constructs it had been declining: the
lane-ordinal idiom, the ergonomic collective API, and a barrier behind a helper.

Thirteen issues, and the three-part chain in the middle of them had to land in
order. [#22](https://github.com/vyncint/reconverge/issues/22) made evaluation
width-typed, [#23](https://github.com/vyncint/reconverge/issues/23) added an
exact population count, and
[#24](https://github.com/vyncint/reconverge/issues/24) gave the positional lane
masks their values. Taken out of order the last of those routes exact 32-bit
masks through arithmetic that was wrong at full width — measured, not assumed:
before #22, `(!lane).count_ones() > 0` is true for all 32 lanes and the replay
called it a 1-of-32 hang.

### Numbers

Self-made, on this project's own corpus: conformance holds at zero false
positives with gating findings matching the baseline exactly; the mutation
corpus reports **precision 1.000 across 466 gating findings**, up from 443, with
the `wrapcol` family going from 14 mutants and none detected at the gating tier
to 42 with 23 detected.

Independent, from [simt-diff](https://github.com/vyncint/simt-diff) — 147
generated kernels whose convergence property is known *by construction*, with
oracles computed rather than inherited:

| | |
|---|---|
| safe-by-construction cases | 34 |
| of those gated (false positives) | **0** |
| unsafe-by-construction cases | 113 |
| of those gated | **107** |
| precision at the gating tier | **1.000** |
| recall at the gating tier | **0.947** |
| cases classified as worth a human's attention | **0** |

The six remaining recall gaps are all in the mask family and all documented:
three are the named-`const` mask boundary re-tested in
[#32](https://github.com/vyncint/reconverge/issues/32), and four of the six are
reported at `warning` rather than silent.

Still no findings from real code — the true-north number remains unearned.

### Added

- **The unmasked warp wrappers are analyzed**
  ([#21](https://github.com/vyncint/reconverge/issues/21)). A kernel written
  entirely against the ergonomic API used to be analyzed as though it held no
  collectives at all — silence, not a warning. `MaskSource` records where a
  collective's mask comes from: the first argument for the `*_sync` surface, an
  implicit `u32::MAX` for the 27 wrappers that delegate with one, and unknown
  for the `reduce_*_partial` helpers, which build theirs from a runtime
  `live_lanes` argument and would be a confident wrong answer called full.
- **Bounded inlining** ([#29](https://github.com/vyncint/reconverge/issues/29)).
  An interprocedural finding is witness-promoted when the callee can be spliced
  into the caller — non-recursive, at most two frames — which replaces "the
  summary says this may reach a barrier" with an actual path. Nothing is
  promoted on a summary bit; the bit raises the finding and a trace confirms it.
- **The replay says why it produced no witness**
  ([#27](https://github.com/vyncint/reconverge/issues/27),
  [#28](https://github.com/vyncint/reconverge/issues/28)). "Unreachable under
  the declared launch" and "a mask naming exactly the arriving lanes" are
  results, not failures to evaluate, and each now carries a matchable `replay:`
  note instead of being indistinguishable from an absence of knowledge.
- **The driver names the missing `rustup` component**
  ([#33](https://github.com/vyncint/reconverge/issues/33)) instead of failing
  with four `E0463`s.

### Changed

- **Unchecked operations evaluate at their operand's width**
  ([#22](https://github.com/vyncint/reconverge/issues/22)). Integer `!` is the
  complement at the operand's own width — which is boolean negation at width 1,
  so conditions fall out of the general rule — and casts truncate to their
  target width rather than being the identity. Where a width is unavailable the
  interpreter yields unknown: exact or unknown, never approximate.
- **`count_ones` is modeled with its operand's width**
  ([#23](https://github.com/vyncint/reconverge/issues/23)), recognized only on
  the primitive-integer impls, and declining an operand carrying bits its type
  cannot hold.
- **The positional lane masks evaluate**
  ([#24](https://github.com/vyncint/reconverge/issues/24)). `lanemask_lt/le/eq/
  ge/gt` are closed forms of the lane's own ordinal, so
  `warp::lanemask_lt().count_ones()` replays. `active_mask` stays unknown: its
  value depends on which lanes are still live, a path-dependent question rather
  than a positional one.
- **Per-warp convergence in the multi-warp replay**
  ([#30](https://github.com/vyncint/reconverge/issues/30)). A collective on a
  lane's path no longer aborts the attempt; a warp whose still-running lanes are
  all at the same collective passes it whatever the other warps are doing, and a
  configuration that would need warps to interact is declined rather than
  approximated. The site itself must still be a barrier beyond one warp.
- **The GitHub Action installs from crates.io** rather than building the
  analyzer from the materialized repo, and caches nothing.

### Fixed

- **`main` did not build.** The launch-matrix helpers merged with `i128`
  constants where `Operand::Const` holds a `u128`.
- **The commit-policy gate's guidance never reached fork contributors** — a fork
  PR gets a read-only token whatever the workflow asks for, so the step that
  explained the failure failed silently. It goes to the job summary now.

### Documented

- Promotion covers every site, not a prefix
  ([#25](https://github.com/vyncint/reconverge/issues/25),
  [#26](https://github.com/vyncint/reconverge/issues/26)). The prefix rule was
  measured at 0.1.11 and the chain above dissolved it; what remains are two
  cases where no lane reaches the later site at all, both correct. The second of
  those was found by simt-diff after the first documentation of this landed.
- The named-`const` mask boundary, with the APIs actually tried
  ([#32](https://github.com/vyncint/reconverge/issues/32)). `ConstDef` exposes
  no way to read the initializer, and `MirConst::eval_target_usize()` — the one
  evaluation entry point — ICEs on a `u32` const *after* resolving the value. The
  boundary is the exposed surface, not the compiler's ability.

## [0.1.12] — 2026-08-18

Fixes [#14](https://github.com/vyncint/reconverge/issues/14): whole-warp
divergence is witnessed at the block the launch contract declares, so a
kernel that is safe at one warp and undefined at two gates exactly when
its contract says two.

### Fixed

- **The witness replays the declared block.** When the one-warp replay
  finds nothing and the kernel's `#[launch_contract]` declares a
  one-dimensional block of several whole warps (64, 96, or 128), barrier
  findings are replayed again at that size. `warp_id()` becomes the warp
  of the thread index, `lane_id()` wraps per warp, `blockDim_x` is the
  declared width, and the lane diagram prints one row per warp. The
  multi-warp replay covers barriers only — any warp collective on any
  lane's path aborts it, since a collective synchronizes within each warp
  and modeling that per-warp choreography wrongly could fabricate a
  witness.
- **Thread-index witnesses now evaluate per name, closing a latent
  false-confirmation hole.** Every `ThreadIndexWitness` used to replay as
  the lane id — but `threadIdx_y` and `threadIdx_z` are 0 under the
  replay's one-dimensional block, not the lane id, so a barrier guarded
  on them (uniform on hardware, correct code) could have been falsely
  confirmed. Each witness name now maps to its cuda-device formula under
  the replayed launch (`index_2d_row` is 0, `warp_index` is the warp,
  `lane_id` wraps), and an unrecognized name evaluates to unknown, never
  to a guess.

## [0.1.11] — 2026-08-18

Fixes [#13](https://github.com/vyncint/reconverge/issues/13):
documentation only — the behavior is deliberate and was undocumented.

### Changed

- Written down, in the README Limitations and `--explain RC001`: a
  recognized construct that the declared launch cannot reach (for
  example a barrier behind mutually exclusive guards) is reported at
  `warning` tier and never witness-promoted. The split is intentional —
  a launch contract is a declaration, not a proof, so staying silent
  would lose the diagnostic for kernels launched outside their declared
  shape, while the replay honestly has nothing to confirm under the
  declared one. Such findings never gate.

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

[0.2.0]: https://github.com/vyncint/reconverge/releases/tag/v0.2.0
[0.1.12]: https://github.com/vyncint/reconverge/releases/tag/v0.1.12
[0.1.11]: https://github.com/vyncint/reconverge/releases/tag/v0.1.11
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
