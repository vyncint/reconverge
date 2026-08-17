# Changelog

All notable changes to this project are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); the artifact
schemas in [`schemas/`](schemas/) are versioned independently of the crates.

Every release reports **both** sets of numbers, deliberately: the corpus
figures this project generates for itself, and the findings that came from
real code. Self-made numbers are a proxy and can be gamed by whoever writes
the corpus; found-in-the-wild is the true north.

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

[0.1.0]: https://github.com/vyncint/reconverge/releases/tag/v0.1.0
