# Learn-mode lessons

Four lessons, in teaching order: **divergence** (how a warp splits),
**barriers** (why a divergent sync hangs), **masks** (who joins a warp
collective), **reconvergence** (the fix). Run them with
`cargo reconverge learn` — everything is embedded in the binary, so the
lessons work with no network, no analysis step, and no files on disk (the
flow tests literally run them from an empty directory).

Format: each file is the lesson's prose, with pages separated by `---`
lines. The kernel excerpt and the recorded witness for each interactive
page live in `reconverge-tui/src/learn/lessons.rs`, which `include_str!`s
these files. Unit tests there lock the page count, the 80×24 layout budget,
and the no-"lockstep" rule — so an edit here that would break the lesson
player fails the build, not the reader. The interactive pages drive the witness debugger's own replay
machinery over the shipped `fixtures/witness/` artifacts (including
`reconverged-clean.json`, the fixed kernel whose verdict is *completed*).

Each explain page links to its lesson (`--explain RC001` → barriers and
reconvergence, `--explain RC002` → masks).
