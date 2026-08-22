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

- **Cut the GitHub Release** from the CHANGELOG section, titled
  `vX.Y.Z — short theme` to match the existing ones, with an `## Install`
  block carrying both commands (`cargo install cargo-reconverge` and
  `cargo reconverge setup`).
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
