# Hardware results

One TSV per session and device, recorded by the procedure in
[../session-1.md](../session-1.md): the confirmed true positives from
`lint-samples`, built and launched with upstream's own tooling under a
watchdog, and the outcome per compute capability. The analysis never runs
on a GPU; these runs are what "undefined behavior on hardware" turned out
to mean on a particular part, and the verdict wording is calibrated to them.

| File | Device | What it says |
| --- | --- | --- |
| [t4-cc75-2026-09-09.tsv](t4-cc75-2026-09-09.tsv) | Tesla T4 (CC 7.5) | Both RC001 probes **passed** — the divergent barrier did not hang on this part with this compiler; the RC002 probe returned the **wrong value** (`0x55555555` for a full-mask ballot half the warp never reached). |
| [a10g-cc86-2026-09-22-synccheck.tsv](a10g-cc86-2026-09-22-synccheck.tsv) | NVIDIA A10G (CC 8.6) | Session #2: 36 labeled mutants under `compute-sanitizer --tool synccheck`. Read below. |

## Session #2 — the sanitizer cross-check, 2026-09-22

The same mutation operators that build the static corpus, applied to whole
upstream examples so the kernels launch, each mutant run under NVIDIA's own
dynamic checker. Static recall is from `conformance/MUTATION.md` at the same
pin; the hardware column is this file.

| class | expected | reconverge, static | on an A10G under synccheck (n) |
| --- | --- | --- | --- |
| `wrapbar` | RC001 | 76% default, **98%** `--strict` | 4 hang, 2 flagged, **3 passed** (9) |
| `wrapcol` | RC002 | 65% default, **100%** `--strict` | 3 hang, 1 build error (4) |
| `shrinkmask` | RC002 | **0%**, published as such | **7 passed, 0 flagged** (7) |
| `mutslice` | RC003 | **100%** | **7 passed, 0 flagged** (7) |
| `delbar` | — (a race; out of the decidable slice) | **0%**, by design | 2 flagged, 1 hang, 6 passed (9) |

Four things in that table are worth saying out loud.

**The dynamic checker did not see RC003 at all.** Seven `mutslice` mutants —
`DisjointSlice<T>` swapped for `&mut [T]`, one exclusive reference handed to
every thread — ran to completion with nothing reported. reconverge denies
every one of them from syntax alone. That is the sharpest static/dynamic
split here: 100% against 0% on the same labeled bugs.

**Three real barrier-divergence bugs passed on hardware.** Of nine `wrapbar`
mutants, four hung, two were flagged, and three completed silently. This is
the accidental pass the README describes, measured: a dynamic tool only sees
the launch you happened to run, and on this part a third of the injected
barrier bugs resolved benignly. reconverge reports all of them under
`--strict` regardless of what the hardware felt like doing.

**`shrinkmask` is invisible to both.** Zero static detection, published as
expected recall 0 — and zero synccheck reports. A full mask shrunk to
`0x0000_ffff` at a convergent call site names no lane that is absent, so
neither tool has anything to catch. The class stays in the corpus because
the boundary is worth publishing, and this is the first evidence it is a real
boundary rather than a reconverge one.

**`delbar` is the honest direction of the trade.** synccheck flagged two
deleted-barrier mutants, and reconverge cannot see any of them: a deleted
barrier is a data race, which is outside the decidable slice by design and
documented as expected recall 0. The dynamic checker earns those two. That is
the gap a user should know about, and it is why the README says the two
approaches are complementary rather than ranked.

One operational note: `atomics` was not probed. Upstream rewrote it at this
pin to take `*mut u32` through `DeviceAtomicU32::from_ptr` rather than
transmuting a shared slice, which is the shape the earlier likely-real
finding keyed on.

What a *pass* here means: not that the code is correct, but that the
undefined behavior happened to resolve benignly on this compiler, driver
and part. The same source is a hang on another — which is why the verdict
says "undefined behavior" first and names the observed outcomes after it,
never "always hangs".
