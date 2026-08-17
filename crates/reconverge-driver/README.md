# reconverge-driver

The compiler driver behind [`cargo-reconverge`](https://github.com/vyncint/reconverge): a rustc wrapper
that obtains Stable MIR for `#[kernel]` functions, runs the uniformity
analysis, replays confirmed findings through the witness interpreter, and
writes the versioned artifacts every other tool reads.

**You probably want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge)**,
which drives this binary for you — `cargo reconverge setup` installs the
matching version of it.

Two constraints follow from being a rustc driver:

- It must be **built by the exact nightly it wraps**
  (`nightly-2026-04-03`, matching upstream cuda-oxide's own pin), with the
  `rustc-dev` and `llvm-tools` components installed.
- docs.rs cannot build it (the `rustc_private` crates are not available
  there); the [repository](https://github.com/vyncint/reconverge) is the documentation.

Nothing in it links, invokes, or parses a GPU vendor SDK component: the
analysis reads your Rust through the compiler's own Stable MIR.
