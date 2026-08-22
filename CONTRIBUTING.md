# Contributing

Thanks for helping build reconverge. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
explains how the pieces fit together; this page is the practical summary of
how to work on them.

> **These four projects share one contributor pattern** — the same commit
> rules, the same DCO, the same AI policy, the same CI and release shape:
> [termlens](https://github.com/vyncint/termlens),
> [mossaic](https://github.com/vyncint/mossaic),
> [launchbound](https://github.com/vyncint/launchbound),
> [reconverge](https://github.com/vyncint/reconverge). Learn it once.

## 1. Dev setup

1. Install [rustup](https://rustup.rs) and
   [`just`](https://github.com/casey/just) (plus
   [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) for the full
   local CI run).
2. `just setup` — materializes the pinned nightly from
   `rust-toolchain.toml` and wires the repo-local git hooks.
3. `just ci` runs everything CI gates on. Keep it green before every push —
   never push red.

## 2. Project layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the crate map, the
artifact flow, the isolation invariants, and the test strategy.

## 3. Testing policy

- The conformance suite runs in CI. **Any new false positive on conformance
  fails CI** — zero false positives at default confidence is a release
  requirement, not a goal. Found one in the wild? File it with the
  "False positive" issue form.
- Every diagnostic code keeps minimal true-positive fixtures, known-tricky
  true-negatives, and a mutation-corpus slice.
- TUI tests are **never skipped-but-green**: a flaky TUI test is
  quarantined the same day and root-caused (either a determinism bug here,
  fixed here, or a termlens issue filed upstream with a reduction).

## 4. Commit conventions

We use [Conventional Commits](https://www.conventionalcommits.org/):
`feat:`, `fix:`, `docs:`, `test:`, `ci:`, `chore:`, `refactor:`, `perf:` —
scope optional (`feat(witness): …`). Subject line: imperative mood,
≤ 72 characters.

## 5. Developer Certificate of Origin (DCO)

Every commit must be signed off:

```sh
git commit -s
```

This appends `Signed-off-by: Your Name <you@example.com>` and certifies you
wrote the change or otherwise have the right to submit it under the project
license — the [Developer Certificate of Origin](https://developercertificate.org),
the same lightweight model the Linux kernel uses. The sign-off email must
match the commit author email; CI enforces this on every commit in a PR.

**There is no CLA. DCO only.** You keep your copyright.

Forgot to sign off? `git commit --amend -s` for the last commit, or
`git rebase --signoff main` for a whole branch, then force-push.

One exception, and it is GitHub's rather than ours: a pull request
**squash-merged through the web UI** has its author email rewritten by GitHub
*after* the sign-off was written, so an exact match is impossible by
construction. Such a commit must carry a sign-off, but is not matched against
an author it did not choose. The commits that went into the PR were already
checked, address and all, on the branch.

## 6. AI tooling policy

**AI assistance is welcome here — use whatever helps.** Every one of these
projects was built with it. There is an [AGENTS.md](AGENTS.md) briefing coding
agents on the layout, the commands, and the house style.

**AI attribution is not welcome.** No `Co-Authored-By` trailer naming an
assistant, model or vendor; no "Generated with …" footer; no robot emoji; no
bot identity as author or committer. Whoever opens the pull request is the
author of record, takes responsibility under the DCO, and the history should
say so — a tool cannot certify the DCO, which is the whole point of it.

This is enforced, not requested: `commit-policy.yml` runs
[`check-no-ai-attribution.sh`](.github/scripts/check-no-ai-attribution.sh) and
[`check-dco.sh`](.github/scripts/check-dco.sh) over every commit in a pull
request. Run them yourself first — both take a range:

```sh
.github/scripts/check-dco.sh main..HEAD
.github/scripts/check-no-ai-attribution.sh main..HEAD
```

If a check fails, rewrite the message rather than arguing with it:

```sh
git commit --amend            # the last commit
git rebase -i main            # several, marking each `reword`
git push --force-with-lease
```

`.claude/settings.json` turns co-author trailers off for agents that read
repository settings. That is a courtesy; the check in CI is the boundary.
Contributions authored *by* an autonomous account are not accepted.

## 7. PR flow

- Branch from `main`; name branches `feat/…`, `fix/…`, `docs/…`, `ci/…`.
- PRs are **squash-merged** — keep the PR title in Conventional Commit form,
  since it becomes the commit subject on `main`. Branches are deleted on merge.
- Required checks: `required-green` (fmt, clippy, test, docs, deny, isolation, conformance), plus `commit-policy` (DCO + attribution). All
  must pass before merge; direct pushes to `main` are blocked by a ruleset.
- **Every change lands with a test, and the test must be able to fail.** If
  you add a guard, break it once and watch it go red before you commit.
- **Say what you did not do.** A PR that lists what it left out and why is
  worth more than one implying completeness. An honest gap is cheap; a false
  claim is expensive.
- **Contributing from a fork?** Two things are normal. On your first PR the
  workflows wait for a maintainer to approve them — GitHub's standard
  first-time-contributor safeguard, nothing you did wrong. And when
  `commit-policy` fails on a fork PR it cannot post its explanatory comment
  (fork PRs get a read-only token); the job log carries the full explanation,
  including the offending commit and the command that fixes it.
- Review: expect actionable review within a few days. Small, focused PRs get
  reviewed faster. Update `CHANGELOG.md` under `[Unreleased]` for any
  user-facing change.

## 8. Release process

Releases are cut by maintainers only; the checklist lives in
[docs/RELEASING.md](docs/RELEASING.md).

## 9. Reviewing findings (the baseline)

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
