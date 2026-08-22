# Working on reconverge

Instructions for coding agents — and useful to humans. **reconverge** is static reconvergence analysis for Rust GPU kernels: it catches divergent barriers and non-convergent warp collectives at compile time, and shows you why, lane by lane, in your terminal.

This file is the canonical brief; `CLAUDE.md` points here. `CONTRIBUTING.md`
is the full contributor document and wins wherever the two disagree.

## Layout

- `crates/reconverge-core` — the analysis. `-dialect-oxide` recognizes
  cuda-oxide items; `-driver` is a **rustc-driver** tool; `-witness` replays 32
  lanes; `-artifacts` is the schema layer; `-tui` the four terminal views;
  `cargo-reconverge` the CLI.
- `schemas/` — the artifact schemas, versioned independently of the crates.
- `conformance/` — the corpus, with an **extractor that has its own
  `Cargo.lock`** outside the workspace. A version bump has to update it too.
- `docs/ARCHITECTURE.md` — the layers and what each may assume.

## Build and test

```sh
cargo test --workspace        # needs the pinned nightly with rustc-dev
cargo test -p reconverge-tui  # the PTY suite alone, no driver needed
./scripts/run-conformance.sh  # the corpus, with --locked
```

Pinned nightly with `rustc-dev` (`rust-toolchain.toml`): a rustc-driver tool
must be built by the same rustc it wraps. Bump it only in lockstep with the
upstream cuda-oxide pin, and never together with a change to analysis
behaviour.

## Things that will bite you here

- **`cargo-reconverge` must keep installing on stable.** It depends on the
  artifacts and dialect crates, not the driver, so a user installs it with
  whatever they have. `install.yml` pins that property; if a change drags the
  driver into the CLI's graph, that check is what will tell you.
- **Goldens:** regenerate with `RECONVERGE_BLESS=1 cargo test -p reconverge-tui`,
  then read every diff.
- **The TUI's lane glyphs are shared with the text diagnostics.** The ASCII
  warp diagram a CI log prints is literally a frame of the witness view. That
  equivalence is load-bearing; do not change the glyph language on one side.
- **The analysis declines to guess.** An unrecognized name evaluates to
  unknown, never to a plausible value. A false confirmation is worse than no
  finding.

## The rules that will fail CI

Three, and they are the same in every one of these repositories.

1. **Conventional Commits.** `feat:`, `fix:`, `docs:`, `test:`, `ci:`,
   `chore:`, `refactor:`, `perf:` — imperative mood, subject line under 72
   characters, scope optional (`fix(screen): …`).
2. **DCO sign-off.** `git commit -s`, and the `Signed-off-by:` email must
   match the commit author's. Forgot? `git commit --amend -s --no-edit`, or
   `git rebase --signoff main` for a branch.
3. **No AI attribution.** See below — this one is about you, and it is the
   rule most likely to catch an agent out.

Run them yourself before pushing; both scripts take a commit range:

```sh
.github/scripts/check-dco.sh main..HEAD
.github/scripts/check-no-ai-attribution.sh main..HEAD
```

## Using AI here

**You are welcome.** Every one of these projects was built with AI assistance
and says so in its CONTRIBUTING. Use whatever helps.

**You are not a contributor.** Do not add yourself to the history:

- no `Co-Authored-By:` trailer naming an assistant, a model, or a vendor,
- no "Generated with …" footer, no robot emoji,
- no bot account as author or committer.

The human who opens the pull request is the author of record and takes
responsibility for the change under the DCO. That is what the sign-off
certifies, and it cannot be certified by a tool. `.claude/settings.json`
turns co-author trailers off for agents that read it; the check in CI is the
boundary, and it reads every commit in the range.

If CI catches one, the fix is to rewrite the message, not to argue with it:

```sh
git commit --amend            # the last commit
git rebase -i main            # several, marking each `reword`
git push --force-with-lease
```

## What good work looks like here

These repositories share a house style, and it is stricter than most:

- **Evidence over assertion.** A bug report says what was measured against
  which released version. "Reproduced against 0.4.0" is the standard; "the
  code looks wrong" is not. Issues in these repos read *Today / Why it is
  worth fixing / Fix / Done when*, with a concrete reproduction.
- **Every change lands with a test**, and the test must be able to fail. If
  you add a guard, prove it catches the thing — break it once and watch it go
  red before you commit.
- **Comments say *why*, never *what*.** The diff shows what. A comment earns
  its place by recording the reason, the alternative rejected, or the failure
  that motivated the line.
- **Say what you did not do.** A pull request that lists what it left out and
  why is worth more than one that implies completeness. If something is
  unverified, say so — an honest gap is cheap and a false claim is expensive.
- **Documentation is checked, not maintained.** Where a README states a fact
  the code owns, there is usually a test asserting the two agree. Do not
  break that pattern by hand-editing the doc.

## Pull requests

Branch from `main` (`feat/…`, `fix/…`, `docs/…`, `ci/…`). PRs are
**squash-merged**, so the PR title becomes the commit subject on `main` —
write it as a Conventional Commit. Update `CHANGELOG.md` under
`[Unreleased]` for anything user-facing.

Direct pushes to `main` are blocked by a ruleset; everything goes through a
pull request, including releases.

## Releasing

Releases are cut by dispatch: tag `vX.Y.Z`, then run `release.yml` with that tag (it offers a dry run). See `docs/RELEASING.md`.
