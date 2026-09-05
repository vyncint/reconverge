# cargo-reconverge

Static reconvergence analysis for Rust GPU kernels — catches divergent
barriers and non-convergent warp operations at compile time, and shows you
why, lane by lane, in your terminal. **No GPU required.**

Works with kernels written using
[cuda-oxide](https://github.com/NVlabs/cuda-oxide).

## Install

Three binaries cooperate: this CLI, the analysis driver, and the terminal
views. Install the CLI, then let it fetch its own matching pieces:

```console
$ cargo install cargo-reconverge
$ cargo reconverge setup
```

`setup` installs the pinned nightly toolchain (a rustc-driver tool must be
built by the exact rustc it wraps) and `reconverge-driver` +
`reconverge-tui` at this CLI's own version. It prints every command before
running it; the manual equivalent is (the `@VERSION` pins matter — all
three binaries must be the same version, so pin both companions to the
version of `cargo-reconverge` you installed):

```console
$ rustup toolchain install nightly-2026-04-03 --profile minimal --component rustc-dev --component llvm-tools
$ rustup run nightly-2026-04-03 cargo install --locked reconverge-driver@VERSION reconverge-tui@VERSION
```

## Use

```console
$ cargo reconverge check              # analyze; exit 1 on deny/confirmed findings
$ cargo reconverge check --strict     # include warning-tier findings
$ cargo reconverge --explain RC001    # why a finding is a bug, and the fix
$ cargo reconverge witness            # step a confirmed hang, 32 lanes at a time
$ cargo reconverge learn              # four interactive SIMT lessons, offline
$ cargo reconverge triage             # review findings into a baseline
$ cargo reconverge watch              # re-run the check on every save
```

A divergent barrier does not crash — the kernel hangs, silently, forever.
`check` finds it statically, a 32-lane interpreter replays the hang under a
concrete launch configuration, and the diagnostic walks the branch condition
back to the thread index that made it divergent:

```text
error[RC001]: kernel `reduce` may execute `sync_threads()` under thread-divergent control
   = note: witness: replayed with grid (1,1,1) x block (32,1,1), warp 0 — 16 of 32 lanes
           wait at `sync_threads()` while 16 never arrive
   = note: lanes 0..31 at the failure point: W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.
```

Zero false positives at default confidence is a requirement, not a goal:
every CI run pushes all of upstream's example kernels through the tool, and
precision against a corpus of mechanically injected bugs is published in the
repository.

## More

The [repository](https://github.com/vyncint/reconverge) has the full README, the architecture notes, the
diagnostic explain pages, and the GitHub Action for running this in CI.
