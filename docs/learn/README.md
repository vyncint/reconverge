# Learn-mode lessons

Four lessons, in teaching order:

1. [divergence](../../crates/reconverge-tui/lessons/divergence.md) — how a warp splits
2. [barriers](../../crates/reconverge-tui/lessons/barriers.md) — why a divergent sync hangs
3. [masks](../../crates/reconverge-tui/lessons/masks.md) — who joins a warp collective
4. [reconvergence](../../crates/reconverge-tui/lessons/reconvergence.md) — the fix

Run them with `cargo reconverge learn`. Everything is embedded in the
binary, so the lessons work with no network, no analysis step, and no files
on disk — the flow tests prove it by running them from an empty directory.

The files live inside `reconverge-tui` because it embeds them at build time,
alongside the recorded replays each interactive page steps through. Those
replay JSONs are copies of `fixtures/witness/`, and a unit test fails if the
two ever drift apart.

Format: each file is the lesson's prose, with pages separated by `---`
lines. The kernel excerpt and the replay for each interactive page are
declared in `crates/reconverge-tui/src/learn/lessons.rs`, whose tests lock
the page count, the 80×24 layout budget, and the no-"lockstep" rule.
