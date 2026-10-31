# reconverge GitHub Action

Runs `cargo reconverge check` on a repository: exit 0 when clean, and the
job fails when any deny/confirmed finding exists — divergent barriers
(RC001), non-convergent warp collectives (RC002), `&mut [T]` kernel
parameters (RC003), shared-memory over-budget (RC004). No GPU and no vendor
SDK anywhere in the run.

```yaml
jobs:
  reconverge:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: vyncint/reconverge/action@v0
        with:
          cc: "8.6" # optional: capacity context for RC004
```

Inputs: `working-directory` (default `.`), `cc`, `strict`
(`"true"` shows warning-tier findings; they never gate), and `sarif` — a
path to also write a SARIF 2.1.0 report, for upload with
`github/codeql-action/upload-sarif`:

```yaml
      - uses: vyncint/reconverge/action@v0
        with:
          sarif: reconverge.sarif
      - uses: github/codeql-action/upload-sarif@v3
        if: always()
        with:
          sarif_file: reconverge.sarif
```

Notes:

- The project is compiled with reconverge's pinned nightly (the action
  exports `RUSTUP_TOOLCHAIN`, overriding any toolchain file in your repo) —
  a rustc-driver tool must match the rustc it wraps, and cuda-oxide already
  requires the same pin.
- The analyzer is installed from crates.io with `cargo install`, at the
  version the pinned ref declares. Nothing is cached: every run installs the
  toolchain and that version from scratch, so a run cannot inherit a stale
  binary and what CI checks is always a published release. Expect the install
  to cost a few minutes on every run.
- The action is built from this repository, so the ref you pin decides which
  analyzer version is installed — the install step reads the version out of
  the workspace manifest at that ref. `@v0` is a floating tag moved to each
  release by `release.yml`, so it follows releases rather than the default
  branch; `@v0.7.0` pins one release exactly, and a 40-hex SHA pins one
  commit. **Do not use `@main`**: it re-decides the analyzer version on every
  run of your CI, from whatever merged here since.
- rustup is asked not to update itself (`--no-self-update`): replacing the
  runner's rustup is not this action's business, and it can fail on its
  own terms.

How it is checked: the `action` job in this repository's CI runs it from
the working tree on every pull request — against the lint samples, where
it must fail the job with an RC001 in its SARIF report, and against a clean
kernel crate, where it must pass. It installs the published release that
the tree's version names, so it checks this file against a real release.
While a release pull request names a version crates.io does not have yet,
the job says so and runs nothing; the pull request that moves the semver
baseline after the publish runs it again. (`vyncint/reconverge-action-smoke`
exercised it on a separate repository until 2026-08-17, before `v0`
existed, and has not run since.)
