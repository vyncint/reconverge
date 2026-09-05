# Changelog

All notable changes to this project are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); the artifact
schemas in [`schemas/`](schemas/) are versioned independently of the crates.

Every release reports **both** sets of numbers, deliberately: the corpus
figures this project generates for itself, and the findings that came from
real code. Self-made numbers are a proxy and can be gamed by whoever writes
the corpus; found-in-the-wild is the true north.

## [Unreleased]

## [0.5.0] — 2026-09-05

The theme is **reading what the tool wrote**. Nine of these findings reduce
to the same shape: an artifact, a document or a page was produced correctly
and then consumed without ever being questioned — so the gate could pass on
a crate it never built, a baseline could be suppressed by a document that
said it was something else, and the debugger's evidence panel could argue
against a true finding.

### Added

- **`reconverge-driver --reconverge-version`.** The driver is a
  rustc-driver and forwards argv by design, so `--version` prints rustc's —
  which meant the half that does the analysis and writes the artifact could
  not be identified at all, while both shipped consumers stamped their
  corpora with the *CLI's* version. Every other invocation still hands rustc
  an untouched argv, cargo's `-vV` and `--print` probes included.

- **`findings.v1` gains `target` and `coverage`, additively.** `target` is
  the compiled target's crate types, so a package with a lib and a bin no
  longer emits two documents that nothing can tell apart. `coverage` is the
  run's own tally of what the engine could not read.

- **`witness.v1` documents a multi-warp replay.** `lanes` is now whole warps
  up to 128 rather than `const: 32`, `initial_lane_states` has no upper
  bound, and `lane_changes[].lane` reaches 127. This is recording reality:
  the declared-block replay has written 64, 96 and 128 since 0.1.12, so the
  schema had been unsatisfiable for exactly the artifacts that gate a build.
  It also states, for the first time, the invariant its lane strip is drawn
  from — at any step carrying a `warp_op`, the lanes present are exactly the
  set bits of `warp_op.active`.

- **Three new gates.** `scripts/check-schemas.sh` validates `fixtures/` and
  what an end-to-end `check` emits against `schemas/` (round-tripping
  through serde is not validating: it tolerates anything additive and never
  sees a `const`). `scripts/record-fixtures.sh` regenerates the witness
  fixtures from a real run and diffs them, so the API tests test the
  producer. `scripts/check-plurals.sh` fails on a count printed with a
  parenthesized plural. `conformance/extractor` — its own workspace, and
  therefore outside every gate in the repository — is now covered by fmt and
  clippy in CI and in `just ci`.

### Changed

- **JSON mode's help names the filters it ignores.** `--strict` and
  `--show-suppressed` affect text output only; JSON remains the unfiltered
  analysis record. The behavior was deliberate, but the help text previously
  implied that both flags changed JSON output, and a run given either flag
  in JSON mode now says so on stderr. Reported in #74.

- **`witness.v1`'s `statement` is documented as prose, which is what it has
  always carried.** It was described as a MIR statement or terminator; no
  released driver has ever emitted one — the replay is built from the site
  rather than by walking statements, and the model drops the printable form
  at extraction. All three witness fixtures showed MIR, and all three were
  stamped `tool.version 0.0.0`, so a front-end author who wrote a MIR parser
  had it confirmed by the project's own API tests. The fixtures are now
  recorded from a real `check`; `reconverged-clean.json` is the one that
  cannot be (a witness is written only for a *confirmed* finding) and
  `fixtures/README.md` says so. Reported in #92.

- **Coverage is a property of the run, not a note on one rule.** It was
  built inline in the RC001 site and nowhere else, so four of the five codes
  never carried it — and the case it exists for could not be reached from a
  note at all: a kernel whose divergent barrier is spelled in `asm!` has no
  finding to hang one on, and `--strict` exited 0 over it with a clean bill
  of health. The tally now rides in `findings.v1`, reaches every code as a
  note, and closes the summary line whenever anything was left unread.
  Reported in #96.


- **JSON mode's help names the filters it ignores.** `--strict` and
  `--show-suppressed` affect text output only; JSON remains the unfiltered
  analysis record. The behavior was deliberate, but the help text previously
  implied that both flags changed JSON output.

### Fixed

- **The gate compiles every member it gates on.** The wrapped build was a
  bare `cargo check`, so cargo's own package selection applied while the
  report and the exit code came from every member `cargo metadata` lists.
  Run the gate from inside a member directory — which the action's own
  `working-directory` input exists to do — or add a `default-members` line,
  and the sibling was never re-linted while its previous, clean artifact was
  printed as this run's answer, with the right paths, in the right order,
  exit 0. A deny-tier RC003 and a confirmed RC001 rendered as
  `0 deny, 0 confirmed`. It is `--workspace` now, and a member that still
  produces nothing is named and exits 2 rather than passing by omission: the
  self-heal already computed that set, forced a full rebuild with it, and
  discarded it unread — which also cost two wrapped builds on every run
  forever. Reported in #107.

- **A findings artifact that is not one is refused.** `check` read a
  document out of `target/reconverge/`, rendered it, gated on it and
  re-published it on stdout without once asking whether it was one of its
  own; the only validation was the filename, under an error message that
  already promised the check. A `findings.v99` written by a driver this
  build has never met was merged and printed while the SARIF from the same
  invocation stamped a different producer. Reported in #109.

- **A baseline is checked against its own schema tag.** `schema` exists to
  be checked and nothing checked it, so a document declaring itself
  `findings.v1`, from a tool that is not this one, suppressed a deny-tier
  finding as happily as a real baseline — and a future `baseline.v2` whose
  entries mean something else would be read with v1 semantics, silently, in
  CI. All four readers now go through one `deserialize_checked`, so a fifth
  cannot reintroduce the gap. Reported in #68.

- **A driver replaced in place forces a re-lint.** The driver goes in as
  `RUSTC_WORKSPACE_WRAPPER` and cargo does not notice a same-path wrapper
  whose contents changed, so `cargo install reconverge-driver` over a warm
  tree re-analyzed nothing and CI kept gating on whatever the old driver
  concluded — including a clean verdict on a crate that now has a finding.
  Its path, size and mtime live in the `cc-marker` beside `--cc`. Reported
  in #109.

- **A package with a lib and a bin emits two documents that name their
  targets.** Both carried the same `crate`, and the driver's own comment
  told consumers to key on it — so a dictionary keyed by `crate` keeps
  whichever `read_dir` handed back last, which in the ordinary GPU-project
  shape (kernels in the lib, a thin host binary beside them) can be the
  empty one. The sort key is total now, so two projects of identical shape
  stop emitting their documents in different orders. Reported in #93.

- **`check` into a reader that closes early is not a crash.** Rust sets
  `SIGPIPE` to `SIG_IGN`, so `println!` into a closed pipe panicked and the
  process exited 101 — outside the documented set — reporting a rustc panic
  notice as though the analyzer had crashed on the code under test.
  `check --strict | head -40` is the most ordinary thing anyone does to a
  long report. The verdict is computed before anything is rendered, so the
  run keeps its exit code. Reported in #110.

- **A comment in the analyzed source no longer erases the diagnostics above
  it.** Two real `ESC` bytes in a source line repainted the terminal from
  the analyzed file while the summary went on counting the findings it had
  wiped. C0, DEL and C1 bytes render as Unicode control pictures, tabs
  expand to a fixed stop, and nothing below `0x20` reaches stdout. Reported
  in #108.

- **The caret is measured in terminal cells.** A tab or a wide character
  before the span put it under the wrong column, and a four-character CJK
  parameter got four carets for the eight cells it occupies. Spans are
  untouched: `findings.v1`, SARIF and the baseline keep character columns.
  Reported in #105.

- **The diagnostic block is indented by its own line number's width**,
  the way rustc does it, rather than pinned at the width that suits a
  two-digit line. And a source line past 120 cells is trimmed around its
  span with an ellipsis marking the cut: an 830-column line wrapped ten
  times on an 80-column terminal and scrolled its own header away. Reported
  in #71.

- **`--help` on every subcommand prints usage and exits 0.** It was reported
  as an unrecognized argument, with exit 2 — a poor greeting for the first
  thing anyone types. `--baseline --help` is still the missing value it was.
  Reported in #74.

- **`watch` keeps stdout clean in JSON mode.** Its dashboard went to stdout,
  so half the stream was not JSON. Reported in #89.

- **The summary line stops recommending the flag the run was given.** A
  closing instruction to rerun with `--show-suppressed` in the run that
  already passed it reads as though something were still being withheld,
  which is exactly the doubt the flag exists to remove. The counts stay
  unconditional. Reported in #103.

- **SARIF carries the provenance walk, a `helpUri`, and a stable rule
  level.** The chain back to the divergence source was dropped entirely
  though SARIF has `relatedLocations` for its exact shape; the rules had
  nothing behind "Learn more"; and a rule's default severity was sampled
  from whichever result of that code came first, so swapping two kernels in
  a source file flipped the published default. `--sarif` is how the action
  delivers findings, so for a CI user this *is* the output. Reported in #70.

- **A witness does not outlive the finding it replays.** Witnesses were
  created and never removed, so fixing a kernel, re-running to
  `0 confirmed` and opening the debugger replayed `undefined behavior` on
  the barrier you had just hoisted — and downstream, where the directory is
  the interface, a stale witness turned "declined to promote" into
  "promoted". The driver prunes its own before writing, and `witness`
  filters on the findings this run still has. Reported in #94.

- **The lane strip agrees with the mask row above it.** At a warp
  collective the departures were recorded one step *after* the call, so the
  strip read all 32 lanes active two rows above an `active 0x55555555`
  saying sixteen — in the one view a user opens because they do not yet
  believe the finding. Reported in #91.

- **A damaged artifact is named a damaged artifact.** The same half-written
  file got three different diagnoses depending on which mode read it: the
  shell view named the parse error and its position, the mode loaders threw
  it away and printed `unsupported schema ``` — an empty pair of backticks
  naming nothing — and inspect said nothing at all, because its `errors`
  field was written at four sites and read at none. "Unsupported schema" is
  a version statement, and none of what it suggests helps a truncated file.
  One `sniff_schema`, three callers, and inspect renders its errors.
  Reported in #97.

- **A baseline triage cannot parse is no longer replaced by an empty one.**
  The error rendered only in the branch where there is nothing to review, so
  the screen looked entirely normal — 23 findings, `0 suppressed` — and `w`,
  which has no dirty guard, wrote an empty document and reported
  `baseline written`. Every reviewed acceptance, with its date and its
  ticket number, was gone, and so was the loud exit 2 that would have sent
  anyone to look at the file. That is the loop a maintainer walks *because*
  `check` complained about that file. The write is refused, the errors
  render above the list, `cargo reconverge triage --baseline <broken>` exits
  2 with the message `check` prints, and `write_to` renames into place so a
  failed write cannot truncate a *good* baseline either. Reported in #88.

- **The conformance and mutation gates run on macOS.** Both prune a member
  with GNU `sed -i`; BSD sed takes a mandatory operand after `-i`, so the
  expression was eaten as a backup suffix and the run died on
  `invalid command code C`. Neither prune is conditional, so no macOS
  contributor could run either gate as committed. Reported in #82 and #83.

- **`conformance/extractor` passes the repo's own clippy standard**, and is
  gated on it. It declares its own `[workspace]` and every gate here is
  workspace-scoped, so the crate that decides what the published precision
  table measures had the least scrutiny of anything in the tree — with a
  dead helper failing `-D warnings` for months while `required-green`
  reported success. Reported in #101.

- **`notify-testing-repo` can run.** It was guarded on
  `github.event_name == 'push'` in a workflow with no `push:` trigger, so
  the condition was unsatisfiable and the job had never executed once —
  skipped, not failed, beside three green jobs, which looks exactly like a
  job that worked. It now keys on `!inputs.dry_run`, takes its version from
  `inputs.tag` rather than a branch name, and names a non-2xx from the
  dispatch API. Reported in #90.

- **Counts agree with their nouns, and a gate keeps them that way.** The
  published precision table still read `466 gating finding(s)` — pinned
  there byte-for-byte by the mutation gate's own diff, because the extractor
  could not reach `reconverge_artifacts::plural` across the workspace
  boundary. No compiler catches this class, which is why #62 closed without
  a gate and it came back. Reported in #102.

- **The multi-warp limitation states itself once.** The README said a warp
  collective on a lane's path aborts the multi-warp replay, and said the
  opposite four lines later; the second is the true one and has been since
  #30. The superseded sentence is deleted rather than negated in place, and
  a lint-sample kernel pairing a 64-thread contract with a collective on the
  path sits next to it as the counterexample. Reported in #106.

- **`--explain RC002` describes the analyzer that ships.** It called the
  unmasked convenience wrappers unchecked — while `warp::ballot` under a
  divergent guard is the `confirmed` finding that just failed the reader's
  CI, which makes a baseline entry against a real, gating, correct
  diagnostic the natural next step. It also called an exactly-matching mask
  unflagged, and its fix snippet used `thread::lane_id`, which does not
  exist. A test now derives the coverage claim from the dialect rather than
  restating it. Reported in #95.

- **The crates.io install page carries the `@VERSION` pins.** It offered an
  unpinned `cargo install` as "the manual equivalent" of a command that
  pins, one sentence after saying `setup` installs the companions at the
  CLI's own version — on the page where the install actually happens. A unit
  test compares both READMEs against `setup`'s own plan. Reported in #104.


- **RC005 launch-contract mismatch help spells axis counts once.** A
  `domain = 2` contract with a 1D index formula no longer says `covers 2 two
  axes`, and wider contracts now recommend narrowing instead of a nonexistent
  three-axis index formula. Reported in #100.

- **Value-taking flags no longer swallow the next flag.** `--sarif --strict`
  used to write the SARIF report to a file named `--strict` and drop
  strict mode; `--baseline --sarif` then tried to read that leftover as a
  baseline. A following token that starts with `--` is now rejected as a
  missing value (`--sarif` requires a value (got the flag `--strict`)),
  with no usage block. Boolean flags reject an inline value the same way
  (`--strict=false` and `--show-suppressed=no` used to enable the flag).
  `--sarif=--weird` still writes to a path named `--weird`. The same two
  rules apply to `watch --max-runs`, `witness --kernel`, `triage
  --baseline`, and the `--ascii` flags on `inspect` / `learn` / `witness`
  / `triage`. Reported in #98.

- **`learn` wraps the verdict instead of truncating it.** The replay panel
  built the verdict as a single line and fit it to the terminal width, so
  the message — the line the lesson exists to deliver — was cut off with an
  ellipsis at every width; the mask lesson needed a ~226-column terminal to
  finish its own sentence, and no key revealed the rest. It now reuses the
  witness debugger's wrapping (`Paragraph::wrap` with the shared
  `wrapped_rows`), sizing the panel to the rows the verdict actually takes,
  exactly as `witness` already renders the same `witness.v1` message. The
  lanes strip — the one multi-span line, which cannot go through `fit` — is
  clipped to the width as it is built, so it too stays a single row and the
  verdict is never pushed off the panel at a narrow width. The four learn
  replay goldens are re-blessed accordingly, plus a narrow-width golden, and
  a guard test asserts no golden's final content line ends in an ellipsis.
  Reported in #69.

- **`check` and `inspect` show the source snippet from any directory in the
  workspace.** Artifact spans are workspace-root-relative, but the reader
  resolved them against the process cwd, so running from a member
  subdirectory read the wrong path: `check` silently dropped every caret
  snippet, and `inspect`'s source pane went blank. Both now anchor to the
  workspace root `cargo metadata` already reports — `render` reads spans
  under it, and `inspect` launches the TUI with its cwd set there, so the
  TUI stays a pure reader. Reported in #67.

- **`--cc` reports an out-of-range or negative capability against the table,
  not as "non-numeric".** Every parse failure — overflow, negative, empty,
  non-digit — shared one "non-numeric major part" message, so `--cc 999.999`,
  `--cc 256.0`, and `--cc -1.0` each told the user their digits were not
  digits and hid the known-capabilities list that would have helped. Numeric
  but impossible values now fall through to the "not in the compute-capability
  table; known: …" message; only genuinely non-numeric input is named as
  such. In the same path, the validated value now travels on in its
  normalized `major.minor` form, so two spellings of one capability (`8.6`
  and `+8.6`) no longer look like a change that drops the build fingerprints
  and forces a full re-lint. Reported in #73.

### Numbers

- **Conformance:** 0 false positives at default confidence over the
  extracted upstream corpus; gating findings match the reviewed baseline
  exactly.
- **Mutation corpus:** precision 1.000 at default confidence over 466 gating
  findings across all compiling mutants — the published table
  ([`conformance/MUTATION.md`](conformance/MUTATION.md)) regenerates
  byte-identically.
- **Found in the wild:** all 27 issues closed here were reported against
  0.4.0 with a measured reproduction; none came from the corpus.


## [0.4.0] — 2026-08-26

### Fixed

- **RC004 reads a shared-memory length given as a named `const`.** It could
  only read a literal. `SharedArray<f32, TILE>` arrived as an unevaluated
  anonymous const body, `eval_target_usize` refused it, and the static was
  dropped from the budget with no finding and no diagnostic — so a kernel
  declaring 80 KiB came back **clean**.

  This was a false negative in a capacity gate, which is the worst shape a
  bug in this tool can take, and the path it hid on is the one that matters
  most: an autotuner rewrites *named* consts per candidate, so every tunable
  shared-memory size took it. launchbound pruned an eight-candidate space
  with an over-cap tile as 8/8 clean at `--cc 7.5`.

  Reported by a user gating a kernel crate (#65), with the cause correctly
  diagnosed in the report. Both halves are now sample kernels: a named const
  over the cap, and one under it, because resolving a const only counts if it
  resolves to the right number.

- **A size RC004 cannot read is reported, not skipped.** A length that
  depends on a generic parameter is not knowable before the kernel is
  instantiated. It is now a `deny` finding saying the budget is unchecked,
  rather than a silent omission that looks exactly like a kernel with no
  shared memory. Fires on none of the sixteen sample kernels.

- **A bad flag value answers in one line.** `--cc 80` printed the right
  message — `` `80` is not a compute capability; expected e.g. `8.6` `` — and
  then forty-four lines of usage text after it. A caller reading the tail of
  stderr, which is where a failing tool usually puts its reason, got the
  exit-code legend instead; launchbound reported that legend as the cause of
  the failure, once per candidate. Usage now accompanies an *unrecognised
  argument*, where the reference is the answer, and not a recognised argument
  with an unusable value, where the message already is.

### Changed

- **Counts agree with their nouns.** `1 finding(s)`, `4 file(s) watched`,
  `1 function(s), 3 value(s)` — on the last line of every run, in the TUI
  headers, in the driver's progress line and in the conformance scripts, which
  is to say in every CI log anyone pastes into an issue. They now read
  `1 warning finding (1 hidden; rerun with --strict to see it)`, verb and
  pronoun included.

- **`--baseline`'s help says what `--baseline` does.** It read as though a
  missing file were always an empty baseline. Only the *default* is; a path
  named explicitly must exist, so a typo cannot pass for a clean run. The
  behaviour is unchanged and was already tested — the documentation was
  wrong, and the error now explains the asymmetry rather than stating it.

### Corpus

- 387 warning-confidence findings, 8 RC001/RC002 chains complete, mutation
  precision 1.0 — unchanged.
- Found in the wild this cycle: one (#65, RC004 named consts).

## [0.3.0] — 2026-08-22

### Added

- **The witness view shows the whole run, not one step at a time.** Below the
  event block, every step is listed with the current one marked and a delta
  column saying how many lanes changed state and to what — the artifact's own
  `lane_changes`, counted. The entire bug is now legible without pressing a
  key: the modulo splits the warp at step 3, sixteen lanes wait at the barrier
  at step 4, sixteen leave at step 5.

  `h`/`l` previously answered "what does this run do?" only by paging blindly
  and remembering, and remembering is what a reader of a divergence bug has
  least to spare.

### Changed

- **The witness view fills the terminal it was given.** The verdict block was
  `Min(2)` and drew two or three lines into a region a dozen rows tall, so on
  an ordinary terminal half the screen said nothing. The timeline takes that
  space; the verdict follows it directly and any slack now falls off the
  bottom rather than sitting in the middle of the screen. On a terminal too
  short for both, the timeline collapses and the verdict survives — the
  conclusion is never the thing that scrolls away.

- **termlens 0.3 → 0.6** for the TUI test harness, across `reconverge-tui`
  and `cargo-reconverge`. Three releases of breaking change, of which one
  reached this suite: since 0.5 `send` returns `Result`, so the sixty
  keystrokes the flow tests type are now checked rather than discarded. A
  send that fails means the application stopped reading — precisely the
  failure a TUI test exists to catch, and until now it was dropped on the
  floor and the test failed later, somewhere else, as a timeout.

  Each site names its key, because knowing *which* keystroke was lost is
  most of the diagnosis.

  The upgrade also brings the `openpty` retry from 0.6: macOS recycles PTY
  devices through `revoke()` and refuses a suite that asks faster than the
  kernel returns them, which is what `cargo test` does by default on a
  many-core machine. This suite spawns a PTY per flow test.

### CI

- **A stress workflow, which did not exist.** `ci.yml` runs the suite once,
  which is the gate; this asks whether it passes *reliably*. The PTY suite
  runs many times over, split across five machines that each use a different
  `--test-threads`, on dispatch or weekly. Five machines because a race that
  only loses on a slow runner gets five rolls rather than one; five
  concurrencies because that is the axis these faults live on — at one thread
  nothing contends, at sixteen the PTYs open and close on top of each other.
- **A published-package check**, at release and weekly: `cargo install
  cargo-reconverge` into a clean directory from crates.io, then the shape that
  actually matters for a cargo subcommand — that `cargo reconverge` finds the
  binary on PATH, which a `--version` call would not have shown. It runs on
  **stable**, which pins a property worth pinning: this workspace needs a
  nightly for the rustc-driver crates, but the CLI does not depend on them, so
  a user installs it with whatever they have. If that ever stops being true,
  `cargo install` starts demanding a pinned nightly and nothing else would
  notice.

## [0.2.0] — 2026-08-20

The milestone that made the witness interpreter tell the truth about full-width
values, and then used that to reach the constructs it had been declining: the
lane-ordinal idiom, the ergonomic collective API, and a barrier behind a helper.

Thirteen issues, and the three-part chain in the middle of them had to land in
order. [#22](https://github.com/vyncint/reconverge/issues/22) made evaluation
width-typed, [#23](https://github.com/vyncint/reconverge/issues/23) added an
exact population count, and
[#24](https://github.com/vyncint/reconverge/issues/24) gave the positional lane
masks their values. Taken out of order the last of those routes exact 32-bit
masks through arithmetic that was wrong at full width — measured, not assumed:
before #22, `(!lane).count_ones() > 0` is true for all 32 lanes and the replay
called it a 1-of-32 hang.

### Numbers

Self-made, on this project's own corpus: conformance holds at zero false
positives with gating findings matching the baseline exactly; the mutation
corpus reports **precision 1.000 across 466 gating findings**, up from 443, with
the `wrapcol` family going from 14 mutants and none detected at the gating tier
to 42 with 23 detected.

Independent, from [simt-diff](https://github.com/vyncint/simt-diff) — 147
generated kernels whose convergence property is known *by construction*, with
oracles computed rather than inherited:

| | |
|---|---|
| safe-by-construction cases | 34 |
| of those gated (false positives) | **0** |
| unsafe-by-construction cases | 113 |
| of those gated | **107** |
| precision at the gating tier | **1.000** |
| recall at the gating tier | **0.947** |
| cases classified as worth a human's attention | **0** |

The six remaining recall gaps are all in the mask family and all documented:
three are the named-`const` mask boundary re-tested in
[#32](https://github.com/vyncint/reconverge/issues/32), and four of the six are
reported at `warning` rather than silent.

Still no findings from real code — the true-north number remains unearned.

### Added

- **The unmasked warp wrappers are analyzed**
  ([#21](https://github.com/vyncint/reconverge/issues/21)). A kernel written
  entirely against the ergonomic API used to be analyzed as though it held no
  collectives at all — silence, not a warning. `MaskSource` records where a
  collective's mask comes from: the first argument for the `*_sync` surface, an
  implicit `u32::MAX` for the 27 wrappers that delegate with one, and unknown
  for the `reduce_*_partial` helpers, which build theirs from a runtime
  `live_lanes` argument and would be a confident wrong answer called full.
- **Bounded inlining** ([#29](https://github.com/vyncint/reconverge/issues/29)).
  An interprocedural finding is witness-promoted when the callee can be spliced
  into the caller — non-recursive, at most two frames — which replaces "the
  summary says this may reach a barrier" with an actual path. Nothing is
  promoted on a summary bit; the bit raises the finding and a trace confirms it.
- **The replay says why it produced no witness**
  ([#27](https://github.com/vyncint/reconverge/issues/27),
  [#28](https://github.com/vyncint/reconverge/issues/28)). "Unreachable under
  the declared launch" and "a mask naming exactly the arriving lanes" are
  results, not failures to evaluate, and each now carries a matchable `replay:`
  note instead of being indistinguishable from an absence of knowledge.
- **The driver names the missing `rustup` component**
  ([#33](https://github.com/vyncint/reconverge/issues/33)) instead of failing
  with four `E0463`s.

### Changed

- **Unchecked operations evaluate at their operand's width**
  ([#22](https://github.com/vyncint/reconverge/issues/22)). Integer `!` is the
  complement at the operand's own width — which is boolean negation at width 1,
  so conditions fall out of the general rule — and casts truncate to their
  target width rather than being the identity. Where a width is unavailable the
  interpreter yields unknown: exact or unknown, never approximate.
- **`count_ones` is modeled with its operand's width**
  ([#23](https://github.com/vyncint/reconverge/issues/23)), recognized only on
  the primitive-integer impls, and declining an operand carrying bits its type
  cannot hold.
- **The positional lane masks evaluate**
  ([#24](https://github.com/vyncint/reconverge/issues/24)). `lanemask_lt/le/eq/
  ge/gt` are closed forms of the lane's own ordinal, so
  `warp::lanemask_lt().count_ones()` replays. `active_mask` stays unknown: its
  value depends on which lanes are still live, a path-dependent question rather
  than a positional one.
- **Per-warp convergence in the multi-warp replay**
  ([#30](https://github.com/vyncint/reconverge/issues/30)). A collective on a
  lane's path no longer aborts the attempt; a warp whose still-running lanes are
  all at the same collective passes it whatever the other warps are doing, and a
  configuration that would need warps to interact is declined rather than
  approximated. The site itself must still be a barrier beyond one warp.
- **The GitHub Action installs from crates.io** rather than building the
  analyzer from the materialized repo, and caches nothing.

### Fixed

- **`main` did not build.** The launch-matrix helpers merged with `i128`
  constants where `Operand::Const` holds a `u128`.
- **The commit-policy gate's guidance never reached fork contributors** — a fork
  PR gets a read-only token whatever the workflow asks for, so the step that
  explained the failure failed silently. It goes to the job summary now.

### Documented

- Promotion covers every site, not a prefix
  ([#25](https://github.com/vyncint/reconverge/issues/25),
  [#26](https://github.com/vyncint/reconverge/issues/26)). The prefix rule was
  measured at 0.1.11 and the chain above dissolved it; what remains are two
  cases where no lane reaches the later site at all, both correct. The second of
  those was found by simt-diff after the first documentation of this landed.
- The named-`const` mask boundary, with the APIs actually tried
  ([#32](https://github.com/vyncint/reconverge/issues/32)). `ConstDef` exposes
  no way to read the initializer, and `MirConst::eval_target_usize()` — the one
  evaluation entry point — ICEs on a `u32` const *after* resolving the value. The
  boundary is the exposed surface, not the compiler's ability.

## [0.1.12] — 2026-08-18

Fixes [#14](https://github.com/vyncint/reconverge/issues/14): whole-warp
divergence is witnessed at the block the launch contract declares, so a
kernel that is safe at one warp and undefined at two gates exactly when
its contract says two.

### Fixed

- **The witness replays the declared block.** When the one-warp replay
  finds nothing and the kernel's `#[launch_contract]` declares a
  one-dimensional block of several whole warps (64, 96, or 128), barrier
  findings are replayed again at that size. `warp_id()` becomes the warp
  of the thread index, `lane_id()` wraps per warp, `blockDim_x` is the
  declared width, and the lane diagram prints one row per warp. The
  multi-warp replay covers barriers only — any warp collective on any
  lane's path aborts it, since a collective synchronizes within each warp
  and modeling that per-warp choreography wrongly could fabricate a
  witness.
- **Thread-index witnesses now evaluate per name, closing a latent
  false-confirmation hole.** Every `ThreadIndexWitness` used to replay as
  the lane id — but `threadIdx_y` and `threadIdx_z` are 0 under the
  replay's one-dimensional block, not the lane id, so a barrier guarded
  on them (uniform on hardware, correct code) could have been falsely
  confirmed. Each witness name now maps to its cuda-device formula under
  the replayed launch (`index_2d_row` is 0, `warp_index` is the warp,
  `lane_id` wraps), and an unrecognized name evaluates to unknown, never
  to a guess.

## [0.1.11] — 2026-08-18

Fixes [#13](https://github.com/vyncint/reconverge/issues/13):
documentation only — the behavior is deliberate and was undocumented.

### Changed

- Written down, in the README Limitations and `--explain RC001`: a
  recognized construct that the declared launch cannot reach (for
  example a barrier behind mutually exclusive guards) is reported at
  `warning` tier and never witness-promoted. The split is intentional —
  a launch contract is a declaration, not a proof, so staying silent
  would lose the diagnostic for kernels launched outside their declared
  shape, while the replay honestly has nothing to confirm under the
  declared one. Such findings never gate.

## [0.1.10] — 2026-08-18

Fixes [#12](https://github.com/vyncint/reconverge/issues/12):
documentation only — a report about a stated *reason*, not a behavior,
and the reason is what changes.

### Changed

- The Limitations entry for the lane-environment gap named "truncating
  casts" as missing machinery, but casts on the thread index are
  evaluated today and such guards promote to `confirmed` (the issue's
  reproduction). The entry now says the precise thing: casts are
  evaluated *as the identity* — exact for the small thread-index values
  replays traffic in, wrong for full-width masks — and integer `!` is
  modeled for booleans only; width-typed evaluation of those unchecked
  operations is what `lanemask_*` promotion actually needs.
  Overflow-checked arithmetic has been width-typed since 0.1.8.

## [0.1.9] — 2026-08-18

Fixes [#11](https://github.com/vyncint/reconverge/issues/11):
documentation only — the behavior was measured to be better than the
claim, and the claim is what changes.

### Changed

- `conformance/MUTATION.md` said RC002 v1 "does not do mask arithmetic
  against launch shapes", which read literally predicts a gating finding
  for the correct guarded partial-warp idiom. The replay *does* compare
  the mask against the lanes it finds present — promotion happens exactly
  when a named lane is absent, and a mask naming exactly the arrivals is
  never promoted. The shrinkmask row's explanation now says the true,
  narrower thing: a shrunk mask at a *convergent* site names no absent
  lane, so there is nothing to witness; recall numbers are unchanged.
- The README Limitations now state positively what the replay checks
  (mask versus lanes present, under the one-warp launch it runs), instead
  of leaving the stronger property undocumented.

## [0.1.8] — 2026-08-18

Fixes [#10](https://github.com/vyncint/reconverge/issues/10): a divergent
guard *inside* a loop is witness-promoted like the same guard outside one.

### Fixed

- **Overflow-checked arithmetic evaluates in replays.** Debug builds lower
  `n += 1` to a checked pair (`CheckedBinaryOp` + assert + field read),
  which the interpreter did not model — the counter went unknown after one
  iteration, the loop condition became unknowable, and any site inside the
  loop's cyclic region was abandoned. The adapter now recognizes the whole
  idiom (the pair local, function-wide, excluded if anything else ever
  writes it; the `.0` read; the width from the operand's unsigned type)
  and the interpreter evaluates it **exactly within the type's width,
  yielding unknown past it** — the checked form panics the thread on
  overflow, so a wrapped value never exists in the real program and is
  never fabricated in a replay. Signed and 128-bit operands stay
  unmodeled.

## [0.1.7] — 2026-08-18

Fixes [#9](https://github.com/vyncint/reconverge/issues/9): witness
promotion no longer stops at the first barrier it cannot see past, so a
barrier added *above* a confirmable finding can no longer take it out of
the CI gate silently.

### Fixed

- **Lanes split between the site and an upstream barrier are a mutual
  deadlock, not an abort.** The site's arrived lanes wait forever (a
  barrier site waits for the whole block; a collective site is only
  emitted when its mask names an absent lane), so no upstream barrier can
  ever be satisfied either — the parked lanes provably never arrive, and
  the replay now concludes exactly that instead of declining. A divergent
  barrier below another divergent barrier is witness-confirmed again.
- **`warp_id()` and `live_lanes_1d()` evaluate in replays.** The replay
  always runs one full warp (`block [32,1,1]`, the same shape under which
  `blockDim_x` is already hardcoded), where those two are exactly 0 and
  32\. A `warp_id()`-guarded barrier upstream now releases uniformly
  instead of aborting the replay of everything below it — the issue's
  headline case. Findings *under* such guards still never promote (the
  guard is uniform across the replayed warp, so there is no divergence to
  witness), which keeps the documented tier for lane-environment guards
  intact.
- The per-lane registers (`lanemask_*`, `active_mask`) remain deliberately
  unevaluable — their 32-bit mask values would flow into evaluation that
  is not width-typed, and a wrong value could fabricate a confirmation.
  The Limitations section now also states the upstream-guard consequence,
  as the issue requested for any residual ordering effect.

## [0.1.6] — 2026-08-18

Two more coverage bugs from a second independent end-to-end review, plus
hygiene.

### Fixed

- **RC001 now covers every all-threads barrier, not just the block one.**
  `cluster::cluster_sync()` and `grid::sync()` deadlock exactly like
  `sync_threads()` when reached divergently — upstream's own safety note on
  the cluster barrier says so — but only `sync_threads` was classified, so
  a divergent cluster or grid barrier reported nothing, interprocedurally
  included. All three now classify as barriers (a divergent `cluster_sync`
  is witness-confirmed like any other). The mbarrier arrive/wait family
  stays out *deliberately*: it is a phase-counted split barrier where
  partial participation is the designed use, and the boundary is now
  written down in `--explain RC001` and the README.
- **The lane-environment registers are no longer read as uniform.** The
  `lanemask_*` registers (per-lane by definition — upstream documents
  `lanemask_eq()` as `1 << lane_id()`), `warp_id()`, and `live_lanes_1d()`
  took no arguments, so the lattice defaulted their results to uniform:
  guards built on them marked no divergence, silencing RC001 and RC002
  entirely, and the Inspector labeled per-lane hardware registers uniform.
  All seven now classify as divergent environment reads — findings under
  such guards fire at warning tier. They are not witness-promoted yet:
  giving the replay their exact values needs width-typed evaluation
  (integer `!`, truncating casts), which the interpreter does not have —
  and approximating would risk false confirmations, the one thing this
  tool must never produce. The README's Limitations section states the
  tier honestly.
- The mutation corpus's barrier operators now ask the dialect which calls
  are barriers (as the collectives already did), so cluster and grid
  barrier sites join the wrapbar/delbar classes and can never drift from
  the analyzer.

### Changed

- `reconverge-tui` on a non-TTY now explains that an interactive terminal
  is required and points at `--message-format json` / `--sarif`, instead
  of dying with a bare `os error 6`.
- The README's manual-install path now pins `reconverge-driver` and
  `reconverge-tui` to the CLI's version, matching the guarantee
  `cargo reconverge setup` provides.
- The conformance scripts build the extractor with `--locked`, and the
  extractor's lockfile is refreshed as part of a release bump — previously
  it drifted silently and every conformance run dirtied the tree.

## [0.1.5] — 2026-08-18

Dependency housekeeping; no behavior changes.

- The termlens PTY test harness is now a crates.io dependency (0.3.0)
  instead of a rev-pinned git dependency — the pinned rev was exactly the
  v0.3.0 release commit, so the bits are identical. Dev-dependency only;
  it never ships in the binaries.

## [0.1.4] — 2026-08-18

Release-pipeline change only; the shipped code is identical to 0.1.3.

- Publishing now authenticates to crates.io with [Trusted
  Publishing](https://crates.io/docs/trusted-publishing) (GitHub OIDC): the
  release workflow exchanges a per-run identity token for a ~30-minute
  crates.io token at publish time. No long-lived registry token exists
  anywhere anymore — this release is the end-to-end proof.

## [0.1.3] — 2026-08-18

Three bug fixes, from an independent end-to-end review of 0.1.1.

### Fixed

- **`--cc` changes now actually re-lint.** The `--cc` invalidation (and the
  missing-artifact self-heal) deleted `<build>/.fingerprint`, but cargo
  keeps freshness fingerprints under the *profile* directory
  (`<build>/debug/.fingerprint`), so the deletion hit a path that never
  existed and stale RC004 findings were re-rendered verbatim — reporting
  the first capability ever seen as fact, even when `--cc` was dropped
  entirely. Both sites now sweep the profile directories.
- **Workspaces with a proc-macro member no longer re-drive every run.**
  Findings artifacts are named `findings-<crate>-<crate types>.json` and
  the crate name was split off at the *last* hyphen — but `proc-macro` is a
  crate type with a hyphen in it, so those artifacts never matched a
  member, and the self-heal re-ran the whole wrapped `cargo check` on every
  warm invocation. The name now splits at the first hyphen (crate names
  cannot contain one).
- **RC002 now recognizes the collectives cuda-device actually exports.**
  The dialect matched CUDA C spellings (`shfl_*_sync`, `activemask`) that
  do not exist in the Rust API, so every real shuffle fell through
  unclassified. The classifier now covers the full masked `*_sync` surface
  at the pinned rev — `shuffle_*_sync` in every width, `match_*_sync`,
  `redux_sync_*`, `elect_sync`, `is_elected_sync` — plus `sync_mask`, the
  warp barrier, whose mask carries the same contract. `active_mask()` is
  classified as a divergent environment read: its result is divergent for
  the lattice, but it is never flagged (no mask, no synchronization, legal
  under divergence). The conformance extractor now asks the dialect itself
  which collectives it classifies, so the mutation corpus can never drift
  from the analyzer again; the unmasked convenience wrappers
  (`warp::shuffle`, `warp::ballot`, the `reduce_*` helpers) remain outside
  v1 and are now documented as such in `--explain RC002`, the masks lesson,
  and the README.
- **`check` works from any directory, on any default toolchain.** The
  wrapped `cargo check` now exports the pinned toolchain (as the CI action
  always did) and resolves the driver's dylib path from that toolchain
  instead of the ambient one, so a kernel crate no longer needs a copy of
  reconverge's `rust-toolchain.toml` just to keep the driver from dying in
  the dynamic linker. When the wrapped build still fails, the error no
  longer claims "build errors" unconditionally — it distinguishes a driver
  that failed to start and points at `cargo reconverge setup`.

## [0.1.2] — 2026-08-17

The crates.io pages, and a one-stop install.

- `cargo reconverge setup`: after `cargo install cargo-reconverge`, one
  command installs the pinned toolchain with the components the driver
  needs, then `reconverge-driver` and `reconverge-tui` at the CLI's own
  version — the three binaries cannot drift apart. Every command is printed
  before it runs, and failures end with the manual steps.
- Every crate now ships a README (the pages on crates.io were blank: the
  repository README sits at the workspace root, which is never packaged),
  plus keywords and categories. The driver's documentation link points at
  the repository, since docs.rs cannot build `rustc_private` crates.
- The bin crates no longer package their integration tests, which need
  sibling binaries and repository fixtures a package cannot carry.
- The driver/TUI not-found errors now tell installed users about `setup`
  instead of suggesting a `cargo build` that only works in a checkout.

## [0.1.1] — 2026-08-17

Packaging only; no behavior changes.

- The explain pages and the learn-mode lessons now live inside the crates
  that embed them (`cargo-reconverge/explain/`, `reconverge-tui/lessons/`)
  rather than in `docs/`. `include_str!` cannot reach outside a package
  directory, so the published crates would not have compiled without this.
  `docs/explain/` and `docs/learn/` remain as indexes.
- The recorded replays the lessons step through are copies of
  `fixtures/witness/`, with a test that fails if the two drift apart.
- Workspace crates carry version requirements on their path dependencies,
  and publishing is enabled.

## [0.1.0] — 2026-08-17

First public release. The analysis, the four terminal views, and the CI
integration are complete and tested; the version is `0.1.x` because nothing
here has met a real user yet, and the verdict wording still awaits
calibration against hardware.

### Analysis

- Uniformity dataflow over Stable MIR behind a dialect trait, with mandatory
  provenance chains from every divergent value back to its source, and
  declared degrades (irreducible CFGs, opaque statements, coverage reported
  next to findings).
- `RC001` divergent barriers and `RC002` non-convergent warp collectives,
  each promoted to `confirmed` when a 32-lane witness interpreter replays a
  concrete hang under a concrete launch configuration — and left at
  `warning` whenever anything the replay needed was unknowable.
- `RC003` (`&mut [T]` kernel parameters), `RC004` (static shared memory over
  the target's limit, with `--cc`), `RC005` (launch-contract inconsistency).

### Interfaces

- `cargo reconverge check` with `--strict`, `--cc`, `--message-format`,
  `--sarif`, `--baseline`, `--show-suppressed`; exit codes 0/1/2.
- `cargo reconverge inspect | witness | learn | triage | watch`, and
  `--explain RCxxx` for every code.
- Four terminal views (uniformity inspector, 32-lane witness debugger,
  SIMT lessons, findings triage) — pure readers of versioned
  artifacts, deterministic frames, `NO_COLOR` and `--ascii` honored.
- `findings.v1`, `unimap.v1`, `witness.v1`, and `baseline.v1` schemas, with
  fixtures acting as their API tests.
- A GitHub Action wrapper, verified on a separate repository in both
  directions: a clean crate passes, injected findings fail the job.

### Numbers

- **Conformance:** zero false positives at default confidence across the
  extracted upstream corpus (143 kernel crates at the pinned commit), gated
  on every CI run.
- **Mutation corpus:** precision **1.000** at default confidence over 513
  compiling mutants; recall published per bug class, including the honest
  zeros, in [`conformance/MUTATION.md`](conformance/MUTATION.md).
- **Found in the wild:** one candidate — a barrier upstream keeps under
  divergent control — reported at `warning` and *not* claimed as confirmed:
  its guard depends on values the interpreter cannot know, so hardware
  evidence comes first.

[0.5.0]: https://github.com/vyncint/reconverge/releases/tag/v0.5.0
[0.4.0]: https://github.com/vyncint/reconverge/releases/tag/v0.4.0
[0.3.0]: https://github.com/vyncint/reconverge/releases/tag/v0.3.0
[0.2.0]: https://github.com/vyncint/reconverge/releases/tag/v0.2.0
[0.1.12]: https://github.com/vyncint/reconverge/releases/tag/v0.1.12
[0.1.11]: https://github.com/vyncint/reconverge/releases/tag/v0.1.11
[0.1.10]: https://github.com/vyncint/reconverge/releases/tag/v0.1.10
[0.1.9]: https://github.com/vyncint/reconverge/releases/tag/v0.1.9
[0.1.8]: https://github.com/vyncint/reconverge/releases/tag/v0.1.8
[0.1.7]: https://github.com/vyncint/reconverge/releases/tag/v0.1.7
[0.1.6]: https://github.com/vyncint/reconverge/releases/tag/v0.1.6
[0.1.5]: https://github.com/vyncint/reconverge/releases/tag/v0.1.5
[0.1.4]: https://github.com/vyncint/reconverge/releases/tag/v0.1.4
[0.1.3]: https://github.com/vyncint/reconverge/releases/tag/v0.1.3
[0.1.2]: https://github.com/vyncint/reconverge/releases/tag/v0.1.2
[0.1.1]: https://github.com/vyncint/reconverge/releases/tag/v0.1.1
[0.1.0]: https://github.com/vyncint/reconverge/releases/tag/v0.1.0
