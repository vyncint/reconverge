# Hardware session #1 — witness calibration (human-provisioned)

Status: **prepared, not run.** Per CONTRIBUTING.md, this session never
runs unattended: the maintainer provisions a GPU host (rented is fine)
and runs it there. Nothing in this repo's CI touches the CUDA SDK.

## Goal

Calibrate the witness interpreter's verdicts against real hardware: for
each true-positive kernel, record what actually happens — hang, wrong
result, or accidental pass — per compute capability. Verdict wording
stays calibrated ("undefined behavior, usually hangs" — never "always
hangs"); this session turns "usually" into recorded per-CC data.

## Procedure

1. Provision a host with a CUDA-capable GPU, the CUDA toolkit, and the
   pinned Rust nightly. Note the GPU model and compute capability.
2. Clone this repo and the upstream pin:
   `git -C upstream fetch --depth 1 https://github.com/NVlabs/cuda-oxide <PIN>`.
3. Run `scripts/hardware-probe.sh <upstream-dir> <output.tsv>` — it builds
   and launches each probe kernel via upstream's `cargo oxide run`
   equivalent harness with a watchdog timeout, recording
   `kernel<TAB>cc<TAB>outcome<TAB>seconds` per row. Outcomes: `hang`
   (watchdog fired), `wrong-result`, `pass`, `error`.
4. Probe kernels: the confirmed true positives from `lint-samples`
   (`rc001_divergent_barrier`, `rc002_divergent_collective`) today; the
   full mutation corpus. The likely-real upstream finding
   (`atomics::atomic_i32_test`) is also worth a run before reporting it.
5. Commit the TSV under `docs/hardware/results/` and update the witness
   verdict wording if the data disagrees with it.

## Asks for the maintainer

- Provision the host and run step 3 (approximate cost: one spot GPU
  instance for under an hour).
- Bring back the TSV; the analysis and any wording changes happen in a
  normal session afterwards.
