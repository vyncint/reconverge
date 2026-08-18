Warp collectives — `ballot_sync`, `any_sync`, `all_sync`, the
`shuffle_*_sync` family — make the lanes of one warp cooperate: vote,
exchange, gather.
Their first argument is the participation mask, one bit per lane, and it
is a contract: the collective synchronizes exactly the lanes the mask
names, every named lane must eventually execute the same call, and a
lane must always be named in its own mask.

Break the contract and nothing crashes. The result is undefined — a hang,
or a silently wrong value that flows onward into your data.
---
Watch the mismatch. The ballot below sits under `i.get() % 2 == 0` with
the full mask `0xffffffff`: the mask promises all 32 lanes, the branch
delivers 16.

Step with l to the collective: the mask strip names every lane (#), the
active strip shows who actually arrived, and the named-but-absent lanes
are computed for you. Sixteen promised voters never show up.
---
Two honest ways out. Make the call convergent — move it out of the branch
and put the condition inside: `ballot_sync(FULL_MASK, i.get() % 2 == 0)`.
Or, when partial participation is the point, make the mask tell the same
story as the branch: `if lane < 16` pairs with mask `0x0000ffff`, exactly
the lanes that arrive.

This is reconverge's RC002. The mask and the control flow must agree;
either one alone is not enough.
