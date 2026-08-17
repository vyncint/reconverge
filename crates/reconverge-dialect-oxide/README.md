# reconverge-dialect-oxide

The [cuda-oxide](https://github.com/NVlabs/cuda-oxide) dialect surface for
[reconverge](https://github.com/vyncint/reconverge): everything the analysis needs to know about upstream's
APIs, expressed as path recognition.

- Kernel detection under upstream's reserved naming contract.
- Call classification for the engine's `SimtDialect` trait: thread-index
  witnesses, barriers, warp collectives, block-uniform built-ins, atomics.
- The compute-capability table behind shared-memory capacity checks.

Recognition works the way Clippy recognizes `Option::unwrap` — by item
path, against public documentation. No upstream code is vendored, and
nothing links, invokes, or parses a GPU vendor SDK component.

End users want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge),
the CLI built on top.
