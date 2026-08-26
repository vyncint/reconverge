# Security Policy

## Supported versions

The latest published version of reconverge is supported. Security fixes are released in a new patch or minor version, since crates.io releases are immutable. Users should upgrade to the latest published version to receive security fixes.

## Reporting a vulnerability

Report privately — please do not open a public issue:

- GitHub → **Security** → **Report a vulnerability** (preferred), or
- directly to the maintainer, [@vyncint](https://github.com/vyncint).

Please include a minimal reproducer and what you expected instead. Expect an
acknowledgement within a few days; fixes land with a regression test before
the advisory is published, per the project's ratchet rule.

## Threat model

Two properties are worth stating plainly, because they shape what counts as a
vulnerability here.

**Analyzing a crate compiles that crate.** `cargo reconverge check` runs a
real `cargo check` with the analyzer wrapped around rustc, so build scripts
and proc macros in the target crate and its dependencies execute exactly as
they would under `cargo check`. reconverge inherits cargo's trust model and
does not add a sandbox: **do not point it at code you would not be willing to
build.** This is expected behavior, not a vulnerability.

**Nothing here requires a GPU or a vendor SDK.** The analysis reads your own
Rust through the compiler's Stable MIR; it does not link, invoke, or parse
any GPU vendor SDK component, and no shipped code path attempts to talk to a
driver. The only scripts that touch GPU tooling live in `scripts/hardware-*`,
are never invoked by CI, and refuse to run without a GPU driver present.

## In scope

- The analyzer writing outside its own artifacts directory, or the triage
  view writing anywhere other than the single baseline path it was given.
- Path traversal or unsafe deserialization when loading artifacts, baselines,
  or fixtures — including artifacts crafted by a third party.
- Anything in the GitHub Action that could exfiltrate secrets or execute
  attacker-controlled input beyond the ordinary compilation described above.
- Escaping the "no vendor SDK, no GPU" property, or any dependency on one
  appearing in the build graph.

## Out of scope

- **Wrong findings.** A false positive or a missed bug is a correctness issue,
  not a security one: please use the
  [false-positive issue form](.github/ISSUE_TEMPLATE/false_positive.yml) —
  every confirmed report becomes a permanent regression test.
- Vulnerabilities in upstream cuda-oxide, cargo, or rustc; report those to
  their maintainers (we are happy to help route them).
- The fact that analyzing a crate compiles it, as described above.
