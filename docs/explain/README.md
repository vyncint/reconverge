# Explain pages

One page per diagnostic code (`cargo reconverge --explain RCxxx`): a minimal
failing kernel, the hardware reason it fails, and the idiomatic fix. Each
page links to its learn-mode lesson.

The pages are embedded into `cargo-reconverge` at build time, so
`--explain` works offline exactly as shipped: editing a page here changes
the binary's output on the next build. Unit tests hold every page to the
same structural bar — its own heading, a minimal kernel, and no claim that
warps run in lockstep.

Shipped: RC001–RC005. RC006/RC007 are reserved for the v1.1 performance
lints and intentionally have no pages yet.
