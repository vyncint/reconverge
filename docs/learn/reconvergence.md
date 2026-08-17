Split paths meet again. For a structured branch, the meeting point is its
post-dominator — the first place every path out of the branch must pass
through on its way to the end of the function. Past that point the warp
is whole again, whatever happened inside the branch.

That point is where synchronization belongs. The discipline in one line:
do the divergent work first, reconverge, then synchronize.
---
The barriers-lesson kernel, fixed: the branch does its per-lane work,
the paths rejoin, and only then comes `sync_threads()`. Step with l —
the warp splits at the switch, becomes whole at the join, all 32 lanes
arrive at the barrier together, it releases, and the verdict says
completed. This is the shape that cannot hang.
---
This is also exactly how reconverge reasons. The engine marks the region
between a divergent branch and its post-dominator; a barrier or
collective inside that region is a finding, the same operation after the
join is clean, and a block-uniform condition never opens a region at all.

You now have the full loop: `cargo reconverge check` finds the bug,
`--explain RC001` tells you why it is one, `witness` replays it lane by
lane, and this lesson is the fix. Go reconverge first — then sync.
