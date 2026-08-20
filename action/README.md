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
      - uses: vyncint/reconverge/action@main
        with:
          cc: "8.6" # optional: capacity context for RC004
```

Inputs: `working-directory` (default `.`), `cc`, `strict`
(`"true"` shows warning-tier findings; they never gate), and `sarif` — a
path to also write a SARIF 2.1.0 report, for upload with
`github/codeql-action/upload-sarif`:

```yaml
      - uses: vyncint/reconverge/action@main
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
- The action is built from this repository, so it tracks whichever ref you
  pin (`@main`, a tag, or a SHA). Pin a tag or SHA if you want the
  analyzer to change only when you say so.

Verified end to end on a separate repository
(`vyncint/reconverge-action-smoke`): a clean kernel crate passes, and an
injected RC003 fails the job, exactly as the exit-code contract promises.
