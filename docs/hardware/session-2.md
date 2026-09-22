# Hardware session #2 — sanitizer cross-check (human-provisioned)

Status: **run once** — 2026-09-22 on a rented g5.xlarge (NVIDIA A10G,
CC 8.6, driver 595.71.05, CUDA 13.2), cuda-oxide `b0f961df` on
nightly-2026-08-28. 36 mutants across three examples; results in
[`results/a10g-cc86-2026-09-22-synccheck.tsv`](results/a10g-cc86-2026-09-22-synccheck.tsv)
and read in [`results/README.md`](results/README.md). Nothing in this repo's
CI touches the CUDA SDK, and this session still never runs there.

## Goal

Cross-check `compute-sanitizer synccheck` against reconverge's static
verdicts on the *same labeled bugs*: the mutation operators that built the
static corpus (`conformance/MUTATION.md`) are applied to the full upstream
examples — host side included, so the kernels actually launch — and each
mutant runs under NVIDIA's own dynamic checker. Misses in both directions
are data, and the comparison gets published:

- a mutant reconverge confirms that synccheck also flags → agreement;
- a mutant reconverge confirms that synccheck passes → dynamic checking
  needed the failing input and didn't get it (the static tool's whole
  pitch);
- a mutant synccheck flags that reconverge only warns about (or misses) →
  a named engine improvement for the flywheel (CONTRIBUTING.md).

## Procedure

1. Provision a host with a CUDA-capable GPU, the CUDA toolkit
   (compute-sanitizer on PATH), and the pinned Rust nightly. Note the GPU
   model and compute capability.
2. Clone this repo and the upstream pin (`conformance/PIN`).
3. For each example worth probing, run:
   `scripts/hardware-mutation-probe.sh <upstream-dir> <example> <out.tsv>`
   — it generates the labeled mutants of that example's `main.rs` with the
   exact same operators as the static corpus (single-file mode of
   `conformance-extractor mutate`), splices each one in, runs it under
   `compute-sanitizer --tool synccheck` with a watchdog, restores the
   original, and records
   `example<TAB>class<TAB>expected<TAB>kernel<TAB>cc<TAB>outcome<TAB>seconds`.
4. Start with the barrier-heavy examples and `lanemask_scan` for the
   collective classes; add more as time allows. **The example directory is
   `barrier`, not `barrier_sync_test`** — the latter is a kernel inside it,
   and this page named it as an example until the first run found otherwise.
   `atomics` was not probed on 2026-09-22: upstream rewrote it at this pin to
   take `*mut u32` through `DeviceAtomicU32::from_ptr` instead of transmuting
   a shared slice, which is the shape the old finding keyed on.
5. Commit the TSVs under `docs/hardware/results/` — the comparison against
   the static table and its publication happen in a normal session
   afterwards. Session #1's probes (the confirmed lint-sample kernels and
   the likely-real `atomics` finding) can share the same host booking.

## Asks for the maintainer

- Provision the host and run step 3 for the step-4 examples (approximate
  cost: one spot GPU instance for an hour or two, shared with session #1).
- Bring back the TSVs and the interesting logs; everything else happens in
  a normal session.
