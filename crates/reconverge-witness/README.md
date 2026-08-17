# reconverge-witness

The witness interpreter behind [reconverge](https://github.com/vyncint/reconverge): a 32-lane replay of
one finding — never a general-purpose kernel runtime.

Given the same function model the engine analyzed, a finding site, and a
launch shape, it runs each lane through the kernel-subset semantics the
driver captured and watches what happens at the site: who arrives at the
barrier, who never can, which lanes a collective's mask names that are not
there. A successful replay is a concrete thread configuration plus a lane
timeline, and it promotes the finding to `confirmed`.

Its defining property is refusal: anything genuinely unknown on the way — a
branch on a parameter, a loop past the step budget, an unmodeled operation —
aborts the replay. *No witness, the static result stands.* Verdict wording
is calibrated the same way: hardware behavior is "usually" a hang, never
"always".

End users want [`cargo-reconverge`](https://crates.io/crates/cargo-reconverge),
the CLI built on top.
