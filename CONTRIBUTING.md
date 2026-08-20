# Contributing

Thanks for helping build reconverge. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
explains how the pieces fit together; this page is the practical summary of
how to work on them.

## Dev setup

1. Install [rustup](https://rustup.rs) and
   [`just`](https://github.com/casey/just) (plus
   [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) for the full
   local CI run).
2. `just setup` — materializes the pinned nightly from
   `rust-toolchain.toml` and wires the repo-local git hooks.
3. `just ci` runs everything CI gates on. Keep it green before every push —
   never push red.

## Layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the crate map, the
artifact flow, the isolation invariants, and the test strategy.

## Testing policy

- The conformance suite runs in CI. **Any new false positive on conformance
  fails CI** — zero false positives at default confidence is a release
  requirement, not a goal. Found one in the wild? File it with the
  "False positive" issue form.
- Every diagnostic code keeps minimal true-positive fixtures, known-tricky
  true-negatives, and a mutation-corpus slice.
- TUI tests are **never skipped-but-green**: a flaky TUI test is
  quarantined the same day and root-caused (either a determinism bug here,
  fixed here, or a termlens issue filed upstream with a reduction).

## Reviewing findings (the baseline)

A project silences a reviewed finding by accepting it in
`reconverge-baseline.json` — write it with `cargo reconverge triage`, which
requires a reason for every entry. Points that matter in review:

- **An accepted finding is a decision, not a disappearance.** It never
  gates the exit code, its count is always reported, `--show-suppressed`
  prints it with its reason, and SARIF carries it as a suppression with
  that reason as the justification.
- **The reason is the review.** "Accepted because the host half validates
  the launch shape" is a reason; "false positive" is not — if it really is
  one, file it with the "False positive" issue form instead, so the engine
  gets fixed for everyone.
- Entries match on `(crate, kernel, code)`, never on line numbers, so an
  edit above a finding cannot silently unsuppress it. `check` reports
  entries that stopped matching anything: delete them.
- This repository's own conformance gate is *not* a baseline — no
  suppression file can quiet a false positive on the corpus.

## Commit conventions

- [Conventional Commits](https://www.conventionalcommits.org), imperative
  subject ≤ 72 chars. Scopes: `core:` `dialect:` `driver:` `witness:`
  `artifacts:` `schemas:` `tui:` `cli:` `ci:` `docs:` `repo:`.
- **DCO**: sign off every commit (`git commit -s`); the `Signed-off-by:`
  trailer must match the author identity. Squash-merging rewrites the landed
  commit's author to your account's commit email, so the trailer has to use
  *that* address rather than whatever your local `git config` holds — if you
  keep your address private, `ID+username@users.noreply.github.com`. Set it
  per-repo and the gate stays green after the merge, not just on the PR.
- **AI assistance is welcome; AI attribution is not. Remove the trailer and
  recommit — you are the author of record.**

## PR process

- PRs are **squash-merged only**; head branches auto-delete.
- `required-green` (fmt, clippy, test, docs, deny, isolation) must pass,
  and the commit-policy gate checks DCO + attribution hygiene on every
  commit in the range.
- Keep the PR checklist honest — especially "conformance untouched or
  intentionally updated".
