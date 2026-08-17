A GPU runs your kernel once per thread, and the hardware drives those
threads in groups of 32 called warps. The 32 slots of a warp are its
lanes. As long as every lane wants the same next instruction, the warp
moves as one — this is SIMT: one instruction stream, many lanes.

A branch changes that. If the condition depends on a per-thread value,
some lanes want one side and some the other, and the warp splits. Since
compute capability 7.0 the hardware schedules the split groups
independently — never assume the lanes of a warp advance together.
---
Below is the canonical split, replayed from a recorded witness. The
condition `i.get() % 2 == 0` depends on the thread index, so the even
lanes take the branch and the odd lanes skip it.

Step the replay with l and watch the strip: at the switch, one warp
becomes two groups.
---
What makes a value per-thread? The thread index itself, anything derived
from it, loads from thread-dependent addresses, and the return values of
atomics — each lane sees its own answer. What stays uniform across the
block: kernel parameters, `block_idx`-derived expressions, and constants.

reconverge tracks exactly this (`cargo reconverge inspect` shows every
value's label and walks the provenance back to its source). Divergence is
not a bug by itself — it is how GPUs branch. The next lessons are about
the two operations that cannot tolerate it.
