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
- Seven crates publish **in dependency order** — core → artifacts → dialect →
  witness → driver → tui → cargo-reconverge — and `cargo publish` waits for
  each to appear on the index before the next.

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
#    lockfile. run-conformance.sh passes --locked, so it fails the build if
#    you forget this.
(cd conformance/extractor && cargo update -p reconverge-core -p reconverge-dialect-oxide)

# 3. Move the CHANGELOG section: [Unreleased] -> [X.Y.Z] — YYYY-MM-DD,
#    leaving an empty [Unreleased] above it, and add its link definition
#    at the foot of the file.

# 3b. Every witness fixture is stamped with the tool version that wrote it,
#     and a test holds the stamp to the workspace version. Re-record the
#     three recorded ones, restamp the synthetic one, and refresh the
#     lessons' byte-identical copies — or the `test` and `schemas` gates
#     fail on the release PR (they did, at 0.6.0).
./scripts/record-fixtures.sh
sed -i 's/"version": "OLD"/"version": "X.Y.Z"/' fixtures/witness/reconverged-clean.json
cp fixtures/witness/{rc001-divergent-barrier,rc002-partial-mask,reconverged-clean}.json crates/reconverge-tui/lessons/

# 4. Land it.
git switch -c release/vX.Y.Z
git commit -sam "release: X.Y.Z"
gh pr create --fill

# 5. Tag the squash-merged commit, then publish deliberately.
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
- **Cut the GitHub Release** from the CHANGELOG section, titled
  `vX.Y.Z — short theme` to match the existing ones, with an `## Install`
  block carrying both commands (`cargo install cargo-reconverge` and
  `cargo reconverge setup`).
- **Read the finished release run's job list, and explain every job that is
  not green.** A `skipped` job is something to account for, not something to
  scroll past: `notify-testing-repo` carried a guard on an event this
  workflow cannot receive, so it had never run once — and a skipped job
  beside three green ones looks exactly like a job that worked.
  ```sh
  gh run view --json jobs --jq '.jobs[] | "\(.conclusion)\t\(.name)"'
  ```
- **Verify what was published, not what was built:**
  ```sh
  gh workflow run install.yml
  ```
  It installs from crates.io on stable and checks that `cargo reconverge`
  resolves through PATH — the shape that matters for a cargo subcommand.

## What a version number means here

- **Breaking** (minor pre-1.0): a removed or renamed public item, a changed
  CLI flag, a schema version, or a change to which findings are reported that
  a user would have to relearn.
- **Not breaking**: a new finding class behind an existing code, a new view,
  a better diagnostic.
- **Toolchain bumps are minor**, never patch. The pinned nightly moves only in
  lockstep with the upstream cuda-oxide pin, and never in a change that also
  alters analysis behaviour — one at a time.
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
