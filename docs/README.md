# Documentation

Start with the [README](../README.md); this directory is what sits below it.

## For people using reconverge

| | |
|---|---|
| [`explain/`](explain/) | one page per diagnostic code (`RC001`–`RC005`): a minimal failing kernel, the hardware reason it fails, the idiomatic fix. Also available offline as `cargo reconverge --explain RC001`. |
| [`learn/`](learn/) | the four SIMT lessons — divergence, barriers, masks, reconvergence. Embedded in `cargo reconverge learn`, which replays them against recorded 32-lane witnesses with no network and no analysis step. |
| [`demo/`](demo/) | an asciinema recording of the witness debugger walking the canonical hang and the mask mismatch. |
| [`../action/README.md`](../action/README.md) | the GitHub Action: inputs, SARIF upload, and what it costs on a cold run. |

## For people working on reconverge

| | |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | the design: crate graph and isolation invariants, what happens during one `check`, the engine's lattice and degrades, the witness interpreter's honesty rails, the artifact contract, and how the whole thing is tested. |
| [`../schemas/README.md`](../schemas/README.md) | the versioned schemas and the rules that govern changing them. |
| [`../conformance/README.md`](../conformance/README.md) | the zero-false-positive gate, why upstream examples are extracted rather than built, and the mutation corpus behind the published [precision/recall table](../conformance/MUTATION.md). |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | dev setup, testing policy, commit conventions, and the review rules for accepting a finding. |
| [`../CHANGELOG.md`](../CHANGELOG.md) | what has landed, with both the self-made corpus numbers and the found-in-the-wild ones. |

## Human-run procedures

Two things this project cannot do on its own, prepared here so that when a
GPU host is available the session is mechanical rather than exploratory:

| | |
|---|---|
| [`hardware/session-1.md`](hardware/session-1.md) | calibrate the witness interpreter's verdict wording against real hardware, per compute capability. |
| [`hardware/session-2.md`](hardware/session-2.md) | cross-check the vendor's dynamic checker against reconverge's static verdicts on identical mechanically injected bugs — misses in either direction are data, and get published. |
