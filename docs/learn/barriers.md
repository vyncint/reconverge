`sync_threads()` is a block-wide rendezvous: it compiles to a barrier
that waits until every thread of the block arrives, and only then lets
anyone continue. Its contract is per thread — all of them, no exceptions.

Put that barrier on a path only some threads take, and the threads that
skipped it never arrive. The arrival count stays short forever. There is
no error, no message, no timeout of its own: the block simply stops.
---
Watch it happen: the barrier sits inside `if i.get() % 2 == 0`, so only
the even lanes can ever reach it. Step with l — the even lanes park at
the barrier (W), the odd lanes walk past the branch and leave the kernel
(.), and the verdict lands: sixteen waiting for sixteen that no longer
exist. Undefined behavior; on hardware it usually hangs until a watchdog
kills the kernel.
---
The rule that keeps you safe: a barrier may only execute under
block-uniform control — conditions every thread of the block agrees on
(kernel parameters, `block_idx`, constants). `if block_idx() > 3 {
sync_threads() }` is fine; anything derived from a thread index is not.

This is reconverge's RC001. Statically proven cases and witness-replayed
cases are shown by default; `cargo reconverge witness` opens the replay
you just watched, for your own kernels. The fix is the subject of the
reconvergence lesson.
