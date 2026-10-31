# Releasing reconverge

One page, copy-pasteable. Maintainers only. The same shape as the sibling
projects' release docs — [termlens], [mossaic], [launchbound] — so a
maintainer moving between them is not relearning the process.

**Releases here are cut by dispatch, not by pushing a tag.** That is
deliberate: publishing is irreversible, so the tag and the publish are two
decisions rather than one, and the workflow offers a dry run between them.

## Prerequisites

- **crates.io Trusted Publishing**, linked to this repository and
  `release.yml`. No token is stored anywhere.
- Seven crates publish **in dependency order** — artifacts → core → dialect →
  witness → driver → tui → cargo-reconverge, the order of the loop in
  `release.yml` — and `cargo publish` waits for each to appear on the index
  before the next.

## Cutting vX.Y.Z

```sh
# 0. Green main, and no flakes. ci.yml is the gate; the hunt is not.
gh workflow run stress.yml -f iterations=100
gh run watch                    # ten shards, both OSes

# 1. Bump the version — the workspace manifest AND the five crates that pin
#    siblings by version.
$EDITOR Cargo.toml crates/*/Cargo.toml
cargo check --workspace         # refreshes Cargo.lock

# 2. The conformance extractor is outside the workspace and has its own
#    lockfile. run-conformance.sh passes --locked, and so does its clippy
#    gate in `just ci`, so a stale lock fails the build — with "cannot
#    update the lock file ... because --locked was passed", which names
#    neither the version nor the crate.
#
#    `--workspace` rather than naming crates: the old form listed
#    reconverge-core and reconverge-dialect-oxide and missed
#    reconverge-artifacts, which is a path dependency too, so following this
#    step literally still left the lock stale (0.6.1 hit exactly that).
(cd conformance/extractor && cargo update --workspace)

# 3. Move the CHANGELOG section: [Unreleased] -> [X.Y.Z] — YYYY-MM-DD,
#    leaving an empty [Unreleased] above it. At the foot of the file, add
#    the [X.Y.Z] link definition and move the [Unreleased] compare link's
#    base to vX.Y.Z.

# 3b. Every witness fixture is stamped with the tool version that wrote it,
#     and a test holds the stamp to the workspace version. Re-record the
#     three recorded ones, restamp the synthetic one, and refresh the
#     lessons' byte-identical copies — or the `test` and `schemas` gates
#     fail on the release PR (they did, at 0.6.0).
./scripts/record-fixtures.sh
#     `-i.bak`, not `-i`: BSD sed on macOS takes the next argument as the
#     backup suffix, so the GNU spelling fails there.
sed -i.bak 's/"version": "OLD"/"version": "X.Y.Z"/' fixtures/witness/reconverged-clean.json
rm fixtures/witness/reconverged-clean.json.bak
cp fixtures/witness/{rc001-divergent-barrier,rc002-partial-mask,reconverged-clean}.json crates/reconverge-tui/lessons/

# 4. Land it.
git switch -c release/vX.Y.Z
git commit -sam "release: X.Y.Z"
gh pr create --fill

# 5. Tag the squash-merged commit, then publish deliberately. The first job
#    fails unless the tag is v<the version every crate's manifest declares>,
#    dry run included, so a tag on the wrong commit stops there.
git switch main && git pull
git tag vX.Y.Z && git push origin vX.Y.Z
gh workflow run release.yml -f tag=vX.Y.Z -f dry_run=true    # validate
gh workflow run release.yml -f tag=vX.Y.Z -f dry_run=false   # publish
```

## After the publish

- **Point the semver gate at the release you just cut.** `baseline-version`
  in `ci.yml`'s `semver` job is a literal; move it to X.Y.Z in its own PR
  once the crates are on the index (the job downloads the baseline from
  crates.io, so bumping it *before* the publish fails every check). Until
  then every PR is measured against the previous release — including this
  release's own intended breaks, which is why the release PR carries the
  `breaking` label.
- **The GitHub Release cuts itself.** `release.yml`'s `github-release` job
  creates it from this version's `CHANGELOG.md` section, appends the
  `## Install` block, and attaches one `.tar.gz` per platform — the three
  binaries the `build` job compiles, with a `.sha256` beside each. Before
  this it built them and left them in workflow artifacts, so `v0.6.1`
  shipped with zero assets. Give the release a title if you want one
  (`gh release edit vX.Y.Z --title 'vX.Y.Z — short theme'`); the job names it
  after the tag, since a theme is not something a workflow can invent.
- **`v0` moves itself too.** `float-major-tag` repoints it at the release
  commit through the API, after the publish, so the tag documented in
  `README.md` and `action/README.md` can never name a release that did not
  reach crates.io. It only moves forward: a patch on an older line (0.6.2
  after 0.7.0) leaves `v0` where it is and says so in the job log.
- **Read the finished release run's job list, and explain every job that is
  not green.** A `skipped` job is something to account for, not something to
  scroll past: `notify-testing-repo` carried a guard on an event this
  workflow cannot receive, so it had never run once — and a skipped job
  beside three green ones looks exactly like a job that worked.
  ```sh
  gh run view --json jobs --jq '.jobs[] | "\(.conclusion)\t\(.name)"'
  ```
- **Verify what was published, not what was built.** `release.yml`'s
  `verify-install` job dispatches `install.yml` for the new version; read
  that run. It installs from crates.io on stable, checks that `cargo
  reconverge` resolves through PATH — the shape that matters for a cargo
  subcommand — and then runs the first run a new user would: `setup` into
  the CLI's own install root, and `check` through cargo on a kernel crate
  with no toolchain file, which must exit 1 on its RC001. To repeat it by
  hand:
  ```sh
  gh workflow run install.yml -f version=X.Y.Z
  ```
- **The Action is checked by the next pull request.** CI's `action` job
  runs `./action` against the published release the tree names, so it
  reports "not on crates.io yet" on the release PR itself. The
  semver-baseline PR above is the first to run it against X.Y.Z; read that
  job, not just the green tick.

## What a version number means here

- **Breaking** (minor pre-1.0): a removed or renamed public item, a changed
  CLI flag, a schema version, or a change to which findings are reported that
  a user would have to relearn.
- **Not breaking**: a new finding class behind an existing code, a new view,
  a better diagnostic.
- **Toolchain bumps are minor**, never patch. The pinned nightly moves only in
  lockstep with the upstream cuda-oxide pin, and never in a change that also
  alters analysis behaviour — one at a time.
- **A break is declared twice, and both are load-bearing.** On the pull
  request it is the `breaking` label. On `main` it is a `### Breaking`
  heading under `## [Unreleased]` in the CHANGELOG — `github.event.pull_request`
  does not exist on a push, so the label is invisible there, and before
  #137 the first labelled break merged turned every later push to `main`
  red until the release moved the baseline. Merging a break without
  recording it in the CHANGELOG now fails the gate, which is the better
  rule: the CHANGELOG is the document this release is cut from.
- **A breaking PR carries the `breaking` label.** The `semver` CI job runs
  `cargo-semver-checks` with the release type forced to `patch` — left to
  infer it from the version number, the checker treats a 0.x minor bump as a
  major release and skips every check. The label runs the gate as `major`
  — the checker's word for a 0.x break — so the break is visible in the
  diagnostics rather than waved through by a version bump.

## If something fails mid-release

- **Before publish**: fix, delete the tag (`git push --delete origin vX.Y.Z`),
  re-tag. The dry run exists so this is the usual outcome of a mistake.
- **Part-way through the seven crates**: re-run the dispatch; it skips what is
  already on the registry.
- **After publish**: crates.io is immutable. Ship `X.Y.Z+1`. Yank only if the
  release is actively harmful.

[termlens]: https://github.com/vyncint/termlens/blob/main/docs/RELEASING.md
[mossaic]: https://github.com/vyncint/mossaic/blob/main/docs/RELEASING.md
[launchbound]: https://github.com/vyncint/launchbound/blob/main/docs/RELEASING.md
