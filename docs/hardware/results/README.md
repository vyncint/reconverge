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

What a *pass* here means: not that the code is correct, but that the
undefined behavior happened to resolve benignly on this compiler, driver
and part. The same source is a hang on another — which is why the verdict
says "undefined behavior" first and names the observed outcomes after it,
never "always hangs".
