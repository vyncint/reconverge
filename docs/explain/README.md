# Explain pages

One page per diagnostic code: a minimal failing kernel, the hardware reason
it fails, and the idiomatic fix. Read them in the terminal with
`cargo reconverge --explain RC001`, or here:

| Code | Page |
|---|---|
| `RC001` | [`sync_threads()` under divergent control](../../crates/cargo-reconverge/explain/RC001.md) |
| `RC002` | [warp collective at a non-convergent point](../../crates/cargo-reconverge/explain/RC002.md) |
| `RC003` | [`&mut [T]` as a kernel parameter](../../crates/cargo-reconverge/explain/RC003.md) |
| `RC004` | [static shared memory over the limit](../../crates/cargo-reconverge/explain/RC004.md) |
| `RC005` | [launch-contract inconsistency](../../crates/cargo-reconverge/explain/RC005.md) |

The files live inside `cargo-reconverge` because the binary embeds them at
build time: `--explain` then works offline, and the pages travel with the
crate when it is published. Editing one changes the binary's output on the
next build, and unit tests hold every page to the same structural bar — its
own heading, a minimal kernel, and no claim that warps run in lockstep.

`RC006`/`RC007` are reserved for the planned performance lints and have no
pages yet.
