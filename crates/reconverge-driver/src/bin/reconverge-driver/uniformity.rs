//! RC001 and the `unimap.v1` artifact, from the core engine's results.

use reconverge_artifacts::findings::{
    Confidence, Finding, ProvenanceStep, RunCoverage, SourceSpan,
};
use reconverge_artifacts::unimap;
use reconverge_artifacts::witness::{
    FindingRef, LaneChange, LaneState, Launch, Step, Verdict, WitnessArtifact,
};
use std::collections::BTreeSet;

use reconverge_core::analysis::{
    self as engine, Analysis, BarrierSite, BranchCause, ReasonKind, Summaries,
};
use reconverge_core::dialect::CallKind;
use reconverge_core::inline::{MAX_DEPTH, inline_all, inline_calls};
use reconverge_core::interproc::{self, KernelContext, Reached};
use reconverge_core::model::{FnId, FnModel, TermKind};
use reconverge_core::{LaunchScope, Uniformity};
use reconverge_witness::{NoWitness, Replay, SiteKind, ascii_warp_diagram};

use crate::adapt::CrateModels;

/// The engine's results for one crate: the summaries, and per kernel every
/// function it reaches, analyzed with the arguments it passes (#188).
pub struct Results {
    pub summaries: Summaries,
    pub contexts: Vec<KernelContext>,
}

/// Analyze every kernel, and every local function a kernel reaches, with
/// the core engine.
pub fn analyze_kernels(models: &CrateModels) -> Results {
    let summaries = Summaries::compute(&models.fns);
    let contexts = models
        .kernels
        .iter()
        .map(|&id| interproc::analyze_kernel(&models.fns, &summaries, id))
        .collect();
    Results {
        summaries,
        contexts,
    }
}

fn span_of(models: &CrateModels, span_ref: usize) -> SourceSpan {
    models.spans[span_ref].clone()
}

/// Everything a replay attempt needs to know about the finding's site.
struct ReplaySite {
    block: usize,
    kind: SiteKind,
    cause_span: usize,
    /// Diagram glyph for lanes that reach the call (`W` barrier, `A`
    /// collective).
    arrived_glyph: char,
}

/// `model` as a replay may run it: every call that could synchronize inside
/// a body the replay does not enter becomes a jump it declines at.
///
/// The interpreter steps over an ordinary call, which is right for a
/// callee that only computes a value — its result is unknown, and unknowns
/// are handled. A callee that may hold a barrier is different: a lane
/// stepping over it is a lane the replay claims did not wait there, and a
/// verdict naming which lanes "never arrive" would rest on that claim. So a
/// local callee whose summary says it may synchronize must be spliced in
/// (the bounded inliner) or the lane declines; so must a call the analysis
/// cannot see into at all (#188, #189). Inline asm is left as it was:
/// its outputs are statements, so its terminator carries no destination.
///
/// A lane that never gets to such a call is unaffected, which is right for
/// every call but one kind: a callee that can reach the site's own function
/// again — recursion — reaches the very barrier being replayed, so the
/// lanes that take it have not "never arrived". Those calls also get an
/// edge to the site, so a lane heading for one is still a lane that can
/// reach the site, and declines there instead of being counted absent.
fn replay_model(
    models: &CrateModels,
    summaries: &Summaries,
    model: &FnModel,
    site: &ReplaySite,
    site_fn: Option<FnId>,
) -> FnModel {
    let n = summaries.may_contain_barrier.len();
    let may_sync =
        |c: FnId| c < n && (summaries.may_contain_barrier[c] || summaries.may_contain_warp_op[c]);
    let reenters = |c: FnId| {
        site_fn.is_some_and(|s| c < n && interproc::reachable(&models.fns, c).contains(&s))
    };
    let mut out = model.clone();
    for block in &mut out.blocks {
        let declined = match &block.term.kind {
            TermKind::Call { callee, target, .. }
                if callee.kind == CallKind::Other
                    && callee
                        .local_fn
                        .into_iter()
                        .chain(callee.may_call.iter().copied())
                        .any(may_sync) =>
            {
                Some(*target)
            }
            TermKind::Opaque {
                dest: Some(_),
                target,
                ..
            } => Some(*target),
            _ => None,
        };
        let reentering = match &block.term.kind {
            TermKind::Call { callee, .. } => callee
                .local_fn
                .into_iter()
                .chain(callee.may_call.iter().copied())
                .any(reenters),
            _ => false,
        };
        if let Some(target) = declined {
            let mut targets: Vec<usize> = target.into_iter().collect();
            if reentering {
                targets.push(site.block);
            }
            // `Jump` is the terminator the interpreter declines at, and
            // with no target it is still one: a lane reaching it bails.
            block.term.kind = TermKind::Jump { targets };
        }
    }
    out
}

/// What every replay attempt shares: the models, the summaries that say
/// which calls may synchronize, and the witnesses confirmed so far.
struct Replayer<'a> {
    models: &'a CrateModels,
    summaries: &'a Summaries,
    witnesses: &'a mut Vec<WitnessArtifact>,
}

impl Replayer<'_> {
    /// Replay one site of `model`: one warp, then the declared block. `site_fn`
    /// is the function the site's block was copied from, when it was spliced
    /// in from a helper.
    fn replay_at_launch(
        &self,
        model: &FnModel,
        site: &ReplaySite,
        site_fn: Option<FnId>,
    ) -> Result<Replay, NoWitness> {
        let model = &replay_model(self.models, self.summaries, model, site, site_fn);
        // One warp first — a divergent lane pair inside a warp is the common
        // case. When that finds nothing and the kernel's launch contract
        // declares a one-dimensional block of several whole warps, replay the
        // declared block: whole-warp divergence (a `warp_id()` guard) has no
        // divergent pair inside 32 lanes and only exists at the declared size.
        match reconverge_witness::replay_outcome(model, site.block, site.kind, site.cause_span) {
            Ok(replay) => Ok(replay),
            Err(one_warp) => match model.declared_block {
                // The declared shape is the launch this kernel claims, so when
                // it is replayed its answer is the one to report.
                Some([x, 1, 1]) if x > 32 => reconverge_witness::replay_outcome_at(
                    model,
                    site.block,
                    site.kind,
                    site.cause_span,
                    x,
                ),
                _ => Err(one_warp),
            },
        }
    }

    /// Attempt a witness replay for a direct site. On success: promote the
    /// finding to `confirmed`, add the concrete configuration and the ASCII
    /// warp diagram to its notes (they render in text and SARIF alike), and
    /// assemble the `witness.v1` artifact.
    fn try_witness(&mut self, f: &FnModel, finding: &mut Finding, site: &ReplaySite) {
        let mut outcome = self.replay_at_launch(f, site, None);
        // A guard that reads a helper's return value — `if
        // get_global_thread_id() % 2 == 0` — is an unknown to the replay while
        // the helper is a call, and arithmetic once it is spliced in (#188).
        // Splicing appends the callees, so the site keeps its block id. Only a
        // replay that could not decide is retried; a decided one already used
        // every value it needed.
        if matches!(outcome, Err(NoWitness::Indeterminate))
            && let Some(inlined) = inline_all(&self.models.fns, f, MAX_DEPTH)
        {
            let retried = self.replay_at_launch(&inlined.model, site, None);
            if retried.is_ok() {
                finding.notes.push(
                    "witness: the helpers on this path were inlined, so the values it branches \
                 on are concrete rather than call results"
                        .to_string(),
                );
            }
            outcome = retried;
        }
        let replay = match outcome {
            Ok(replay) => replay,
            // Two results that are not witnesses but are still knowledge. They
            // used to be indistinguishable from "could not evaluate", which
            // left a reader — and anything consuming findings.v1 — unable to
            // tell a checked-and-correct idiom from an absence of knowledge.
            // The `replay:` prefix and wording are meant to be matched on.
            Err(NoWitness::UnreachableUnderLaunch) => {
                finding.notes.push(
                    "replay: unreachable under the declared launch — no lane reaches this \
                 construct, so there is nothing to confirm. This is a result, not a \
                 failure to evaluate"
                        .to_string(),
                );
                return;
            }
            Err(NoWitness::MaskMatchesArrivals { mask, arrived }) => {
                // The generic note asks the reader to verify what the replay
                // has just verified. Leaving both in is what made this result
                // read as the weakest on this path rather than the strongest.
                finding
                    .notes
                    .retain(|n| !n.contains("verify the branch admits exactly the lanes"));
                finding.notes.push(format!(
                    "replay: the mask {mask:#010x} names exactly the lanes that arrive \
                 ({arrived:#010x}) — the guarded partial-warp idiom, checked and correct \
                 under this launch"
                ));
                return;
            }
            Err(NoWitness::Uniform | NoWitness::Indeterminate) => return,
        };
        self.promote(f, finding, &replay, site.arrived_glyph);
    }

    /// Promote a finding on a concrete replay: the launch, the verdict, the
    /// ASCII warp diagram, and the `witness.v1` artifact.
    fn promote(
        &mut self,
        f: &FnModel,
        finding: &mut Finding,
        replay: &Replay,
        arrived_glyph: char,
    ) {
        finding.confidence = Confidence::Confirmed;
        let warps = replay.block[0].div_ceil(32);
        finding.notes.push(format!(
            "witness: replayed with grid ({},{},{}) x block ({},{},{}), {} — {}",
            replay.grid[0],
            replay.grid[1],
            replay.grid[2],
            replay.block[0],
            replay.block[1],
            replay.block[2],
            if warps == 1 {
                "warp 0".to_string()
            } else {
                format!("all {warps} warps")
            },
            replay.verdict_message
        ));
        finding.notes.extend(ascii_warp_diagram(
            replay.arrived,
            replay.never_arrives,
            replay.block[0],
            arrived_glyph,
        ));
        self.witnesses
            .push(witness_artifact(self.models, f, finding, replay));
    }

    /// Replay an interprocedural site by splicing the callee in (#29).
    ///
    /// The summary tier is unchanged and still the fallback: this does not
    /// promote on "the callee *may* reach a barrier", it removes the call so
    /// there is an actual path to replay. Anything the inliner refuses —
    /// recursion, too many frames, too many blocks — leaves the finding
    /// exactly where it was.
    fn try_witness_through_inlining(
        &mut self,
        f: &FnModel,
        finding: &mut Finding,
        call_block: usize,
        cause_span: usize,
        arrived_glyph: char,
    ) {
        let Some(inlined) = inline_calls(&self.models.fns, f, MAX_DEPTH) else {
            return;
        };
        let Some((_, sites)) = inlined.exposed.iter().find(|(b, _)| *b == call_block) else {
            return;
        };
        for &block in sites {
            let Some(kind) = site_kind_of(&inlined.model.blocks[block]) else {
                continue;
            };
            let site = ReplaySite {
                block,
                kind,
                cause_span,
                arrived_glyph,
            };
            let site_fn = inlined.origins[block].map(|(id, _)| id);
            if let Ok(replay) = self.replay_at_launch(&inlined.model, &site, site_fn) {
                finding.notes.push(
                    "witness: the callee was inlined at the call site, so this is a \
                 concrete path rather than a summary bit"
                        .to_string(),
                );
                self.promote(f, finding, &replay, arrived_glyph);
                return;
            }
        }
    }

    /// Replay a site inside a helper, against the kernel that reaches it (#188).
    ///
    /// The helper is spliced into the kernel and every copy of the site's block
    /// is replayed from the kernel's entry, so the witness is a path through the
    /// kernel's own code with the helper's arguments bound to what the kernel
    /// passes — not the join of every call that the static finding rests on.
    /// Bounded like every inlining replay; outside the bounds the finding stays
    /// at `warning`.
    fn try_witness_in_helper(
        &mut self,
        kernel: &FnModel,
        finding: &mut Finding,
        helper: FnId,
        block: usize,
        cause_span: usize,
        arrived_glyph: char,
    ) {
        let Some(inlined) = inline_calls(&self.models.fns, kernel, MAX_DEPTH) else {
            return;
        };
        for (copy, origin) in inlined.origins.iter().enumerate() {
            if *origin != Some((helper, block)) {
                continue;
            }
            let Some(kind) = site_kind_of(&inlined.model.blocks[copy]) else {
                continue;
            };
            let site = ReplaySite {
                block: copy,
                kind,
                cause_span,
                arrived_glyph,
            };
            if let Ok(replay) = self.replay_at_launch(&inlined.model, &site, Some(helper)) {
                finding.notes.push(
                    "witness: the helper was inlined into the kernel, so this is a concrete path \
                 with the arguments this kernel passes"
                        .to_string(),
                );
                self.promote(kernel, finding, &replay, arrived_glyph);
                return;
            }
        }
    }
}

/// The site a spliced-in block represents, with its mask where the call
/// carries one.
fn site_kind_of(block: &reconverge_core::model::Block) -> Option<SiteKind> {
    let TermKind::Call {
        callee, const_args, ..
    } = &block.term.kind
    else {
        return None;
    };
    match callee.kind {
        CallKind::Barrier => Some(SiteKind::Barrier),
        CallKind::WarpCollective { .. } => Some(SiteKind::Collective {
            mask: if callee.kind.mask_is_unknown() {
                None
            } else {
                callee
                    .kind
                    .implicit_mask()
                    .or_else(|| const_args.first().copied().flatten())
            },
        }),
        _ => None,
    }
}

fn witness_artifact(
    models: &CrateModels,
    f: &FnModel,
    finding: &Finding,
    replay: &Replay,
) -> WitnessArtifact {
    let launch = Launch {
        grid: replay.grid,
        block: replay.block,
        // A one-warp replay is warp 0; a whole-block replay is not a single
        // warp's view.
        warp: (replay.block[0] == 32).then_some(0),
    };
    let steps = replay
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| Step {
            index,
            statement: step.statement.clone(),
            span: step.span.map(|s| span_of(models, s)),
            lane_changes: step
                .lane_changes
                .iter()
                .map(|&(lane, state)| LaneChange { lane, state })
                .collect(),
            barrier: step.barrier.map(|(arrived, expected)| {
                reconverge_artifacts::witness::BarrierEvent { arrived, expected }
            }),
            warp_op: step.warp_op.as_ref().map(|(op, mask, active)| {
                reconverge_artifacts::witness::WarpOpEvent {
                    op: op.clone(),
                    mask: format!("{mask:#010x}"),
                    active: format!("{active:#010x}"),
                }
            }),
        })
        .collect();
    let verdict = Verdict {
        kind: replay.verdict_kind,
        message: replay.verdict_message.clone(),
        step: Some(replay.verdict_step),
    };

    // `krate` stays empty here and is filled at write time with the crate
    // name, as it always was.
    let mut artifact = WitnessArtifact::new(
        f.name.clone(),
        launch,
        u8::try_from(replay.block[0]).unwrap_or(32),
        vec![LaneState::Active; replay.block[0] as usize],
        steps,
        verdict,
    );
    artifact.finding = Some(FindingRef {
        code: finding.code.clone(),
        span: Some(finding.span.clone()),
    });
    artifact
}

/// Where a site sits relative to the kernel a finding is reported against.
struct SiteScope<'a> {
    kernel: &'a FnModel,
    /// The function the site is in, and its analysis under this kernel.
    reached: &'a Reached,
}

impl SiteScope<'_> {
    /// The helper the site is inside, when it is not in the kernel itself.
    fn helper<'m>(&self, models: &'m CrateModels) -> Option<&'m str> {
        let f = &models.fns[self.reached.id];
        (!std::ptr::eq(f, self.kernel)).then_some(f.name.as_str())
    }
}

/// The suffix naming the helper a site is inside, empty for the kernel's own.
fn inside(helper: Option<&str>) -> String {
    helper.map_or_else(String::new, |h| format!(", inside `{h}`"))
}

/// A note for a finding inside a helper: what the helper was analyzed
/// with, so a reader does not take the join for a trace.
fn helper_note(helper: &str, kernel: &str) -> String {
    format!(
        "`{helper}` is analyzed with the arguments `{kernel}` passes it, joined over every \
         call in this kernel's call tree; a witness replays the actual calls"
    )
}

/// One finding per site and kernel, however many instances of a generic
/// helper reach it. Only helper sites are deduplicated: a kernel's own
/// findings are reported exactly as they always were.
#[derive(Default)]
struct Reported(BTreeSet<(String, String, String, usize, usize)>);

impl Reported {
    fn first(&mut self, finding: &Finding) -> bool {
        self.0.insert((
            finding.code.clone(),
            finding.kernel.clone().unwrap_or_default(),
            finding.span.file.clone(),
            finding.span.line_start,
            finding.span.column_start,
        ))
    }
}

/// The provenance a divergence finding prints: the branch, then the chain
/// back to its source — across the helper boundaries it crossed (#188).
fn divergence_provenance(
    models: &CrateModels,
    results: &Results,
    ctx: &KernelContext,
    scope: &SiteScope<'_>,
    cause: BranchCause,
    first: String,
) -> Vec<ProvenanceStep> {
    let mut provenance = vec![ProvenanceStep {
        what: first,
        span: span_of(models, cause.span),
    }];
    provenance.extend(
        interproc::provenance(
            &models.fns,
            &results.summaries,
            ctx,
            scope.reached.id,
            cause.cond,
        )
        .into_iter()
        .filter(|step| !is_temp_plumbing(&step.detail))
        .map(|step| ProvenanceStep {
            what: step.detail,
            span: span_of(models, step.span),
        }),
    );
    provenance
}

/// RC001 (warning until a witness confirms it, per docs/ARCHITECTURE.md):
/// a barrier reachable under thread-divergent control — in the kernel, or
/// in any local function it reaches (#188).
pub fn rc001_divergent_barriers(
    models: &CrateModels,
    results: &Results,
    findings: &mut Vec<Finding>,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    let mut replayer = Replayer {
        models,
        summaries: &results.summaries,
        witnesses,
    };
    let mut reported = Reported::default();
    for ctx in &results.contexts {
        let kernel = &models.fns[ctx.kernel];
        for reached in &ctx.reached {
            let scope = SiteScope { kernel, reached };
            let helper = scope.helper(models);
            let analysis = &reached.analysis;
            for site in &analysis.barriers {
                let Some(cause) = site.divergent_cause else {
                    // Not divergent for lanes — but a barrier whose
                    // participants span more than one block can still be
                    // entered by some of them and skipped by others (#133).
                    if let Some((guard, guard_cause)) = site.cross_block_cause {
                        let finding = cross_block_finding(
                            models,
                            results,
                            ctx,
                            &scope,
                            site,
                            guard,
                            guard_cause,
                        );
                        if helper.is_none() || reported.first(&finding) {
                            findings.push(finding);
                        }
                    }
                    continue;
                };
                let message = if site.interprocedural {
                    format!(
                        "kernel `{}` calls `{}`, which may execute a barrier, under \
                         thread-divergent control{}",
                        kernel.name,
                        site.callee_display,
                        inside(helper)
                    )
                } else {
                    format!(
                        "kernel `{}` may execute `{}()` under thread-divergent control{}",
                        kernel.name,
                        site.callee_display,
                        inside(helper)
                    )
                };

                let mut notes = vec![
                    "lanes that skip the barrier never arrive, and the lanes that reach it wait \
                     for them forever — on hardware this is undefined behavior, usually a \
                     permanent hang with no error"
                        .to_string(),
                ];
                if let Some(helper) = helper {
                    notes.push(helper_note(helper, &kernel.name));
                }
                if analysis.irreducible {
                    notes.push(
                        "this function has irreducible control flow; the analysis degraded to \
                         all-divergent for the whole body"
                            .to_string(),
                    );
                }
                if let Some([x, y, z]) = kernel.declared_cluster {
                    notes.push(format!(
                        "kernel declares a ({x}, {y}, {z}) cluster (`#[cluster_launch]`): a \
                         block-wide barrier that cannot be satisfied hangs this block, and a \
                         cluster-wide one (`cluster_sync`, `this_cluster().sync()`) hangs every \
                         block of the cluster waiting on it"
                    ));
                }
                let provenance = divergence_provenance(
                    models,
                    results,
                    ctx,
                    &scope,
                    cause,
                    "thread-divergent branch".to_string(),
                );

                let mut finding = Finding {
                    code: "RC001".to_string(),
                    confidence: Confidence::Warning,
                    message,
                    kernel: Some(kernel.name.clone()),
                    span: span_of(models, site.span),
                    notes,
                    help: Some(
                        "make every thread of the block reach the barrier: hoist it out of the \
                         divergent branch, or make the branch condition uniform"
                            .to_string(),
                    ),
                    explain: "RC001".to_string(),
                    provenance,
                };
                if helper.is_some() && !reported.first(&finding) {
                    continue;
                }
                witness_site(
                    &mut replayer,
                    &scope,
                    &mut finding,
                    site.interprocedural,
                    &ReplaySite {
                        block: site.block,
                        kind: SiteKind::Barrier,
                        cause_span: cause.span,
                        arrived_glyph: 'W',
                    },
                );
                findings.push(finding);
            }
        }
    }
}

/// Attempt a witness for one divergence site, by the route its position
/// allows. `site.block` is the block in the function the site is in.
fn witness_site(
    replayer: &mut Replayer<'_>,
    scope: &SiteScope<'_>,
    finding: &mut Finding,
    interprocedural: bool,
    site: &ReplaySite,
) {
    match (scope.helper(replayer.models).is_some(), interprocedural) {
        (false, false) => replayer.try_witness(scope.kernel, finding, site),
        (false, true) => replayer.try_witness_through_inlining(
            scope.kernel,
            finding,
            site.block,
            site.cause_span,
            site.arrived_glyph,
        ),
        (true, false) => replayer.try_witness_in_helper(
            scope.kernel,
            finding,
            scope.reached.id,
            site.block,
            site.cause_span,
            site.arrived_glyph,
        ),
        // A call inside a helper that may reach a barrier further down: two
        // frames of summary, and the bounded inliner's depth is spent
        // reaching it. It stays at `warning`.
        (true, true) => {}
    }
}

/// RC001 for a barrier wider than a block, guarded by a value that is not
/// constant across its participants (#133).
///
/// Always `warning`, never promotable. The witness interpreter replays the
/// lanes of one block; it cannot execute two blocks and so cannot show this
/// hang the way it shows a divergent `sync_threads`. Reporting it as
/// `confirmed` on static reasoning alone would be the confident wrong
/// answer this project spends its diagnostics avoiding.
fn cross_block_finding(
    models: &CrateModels,
    results: &Results,
    ctx: &KernelContext,
    scope: &SiteScope<'_>,
    site: &BarrierSite,
    guard: LaunchScope,
    cause: BranchCause,
) -> Finding {
    let barrier = site.scope.as_str();
    let helper = scope.helper(models);
    let provenance = divergence_provenance(
        models,
        results,
        ctx,
        scope,
        cause,
        format!("{}-uniform branch", guard.as_str()),
    );
    let mut notes = vec![
        format!(
            "every thread of a block agrees on this branch, so no lane diverges — but the \
             value differs between blocks, so some blocks of the {barrier} enter the \
             barrier and the rest never arrive"
        ),
        "the blocks that did arrive wait for the ones that did not — on hardware this is \
         undefined behavior, usually a permanent hang with no error"
            .to_string(),
        "reported as a warning and never promoted: the witness interpreter replays the \
         lanes of one block, so it cannot execute the second block this needs to show"
            .to_string(),
    ];
    if let Some(helper) = helper {
        notes.push(helper_note(helper, &scope.kernel.name));
    }
    Finding {
        code: "RC001".to_string(),
        confidence: Confidence::Warning,
        message: format!(
            "kernel `{}` may execute `{}()`, a {barrier}-wide barrier, under control that is \
             only {}-uniform{}",
            scope.kernel.name,
            site.callee_display,
            guard.as_str(),
            inside(helper)
        ),
        kernel: Some(scope.kernel.name.clone()),
        span: span_of(models, site.span),
        notes,
        help: Some(format!(
            "make the condition {barrier}-uniform, or hoist the barrier out of the branch so \
             every block of the {barrier} reaches it"
        )),
        explain: "RC001".to_string(),
        provenance,
    }
}

/// RC002 (warning until a witness confirms it, per docs/ARCHITECTURE.md): a
/// warp collective at a point where threads of the warp may be inactive —
/// in the kernel, or in any local function it reaches (#188).
///
/// Scope, from a survey of upstream's examples: they use FULL_MASK
/// literals or runtime-computed masks, and essentially no constant partial
/// masks — so the check is about convergence, with constant masks reported
/// as context (mask refinement) rather than arithmetically verified.
pub fn rc002_nonconvergent_warp_ops(
    models: &CrateModels,
    results: &Results,
    findings: &mut Vec<Finding>,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    let mut replayer = Replayer {
        models,
        summaries: &results.summaries,
        witnesses,
    };
    let mut reported = Reported::default();
    for ctx in &results.contexts {
        let kernel = &models.fns[ctx.kernel];
        for reached in &ctx.reached {
            let scope = SiteScope { kernel, reached };
            let helper = scope.helper(models);
            let analysis = &reached.analysis;
            for site in &analysis.warp_ops {
                let Some(cause) = site.divergent_cause else {
                    continue;
                };
                let message = if site.interprocedural {
                    format!(
                        "kernel `{}` calls `{}`, which may execute a warp collective, under \
                         thread-divergent control{}",
                        kernel.name,
                        site.callee_display,
                        inside(helper)
                    )
                } else {
                    format!(
                        "kernel `{}` calls `{}()` at a point where threads of the warp may be \
                         inactive{}",
                        kernel.name,
                        site.callee_display,
                        inside(helper)
                    )
                };

                let mut notes = vec![
                    "a warp collective synchronizes the lanes its participation mask names; a \
                     named lane that never reaches the call makes the operation undefined — \
                     upstream calls this the worst kind of bug, because there is no crash and \
                     no error, just a kernel that usually never finishes"
                        .to_string(),
                ];
                match site.mask {
                    Some(0xffff_ffff) => notes.push(
                        "the mask is 0xffffffff (every lane), but under this branch some lanes \
                         may never arrive"
                            .to_string(),
                    ),
                    Some(mask) => notes.push(format!(
                        "the mask is {mask:#010x}; verify the branch admits exactly the lanes \
                         it names"
                    )),
                    None if !site.interprocedural => notes.push(
                        "the mask is not a literal the analysis can evaluate (a runtime value, or \
                         a named const — opaque through rustc_public at this pin), so it cannot \
                         be checked statically"
                            .to_string(),
                    ),
                    None => {}
                }
                if let Some(helper) = helper {
                    notes.push(helper_note(helper, &kernel.name));
                }
                if analysis.irreducible {
                    notes.push(
                        "this function has irreducible control flow; the analysis degraded to \
                         all-divergent for the whole body"
                            .to_string(),
                    );
                }

                let provenance = divergence_provenance(
                    models,
                    results,
                    ctx,
                    &scope,
                    cause,
                    "thread-divergent branch".to_string(),
                );

                let mut finding = Finding {
                    code: "RC002".to_string(),
                    confidence: Confidence::Warning,
                    message,
                    kernel: Some(kernel.name.clone()),
                    span: span_of(models, site.span),
                    notes,
                    help: Some(
                        "make the call site convergent (hoist it out of the divergent branch), \
                         or derive the mask from the guard itself (e.g. ballot the condition) \
                         so it names exactly the lanes that arrive"
                            .to_string(),
                    ),
                    explain: "RC002".to_string(),
                    provenance,
                };
                if helper.is_some() && !reported.first(&finding) {
                    continue;
                }
                witness_site(
                    &mut replayer,
                    &scope,
                    &mut finding,
                    site.interprocedural,
                    &ReplaySite {
                        block: site.block,
                        kind: SiteKind::Collective { mask: site.mask },
                        cause_span: cause.span,
                        arrived_glyph: 'A',
                    },
                );
                findings.push(finding);
            }
        }
    }
}

/// Coverage summed over one kernel and every function it reaches: a helper
/// the kernel calls is code the kernel runs, and an `asm!` block inside one
/// is as unread as one in the kernel body.
fn kernel_coverage(ctx: &KernelContext) -> (usize, usize) {
    ctx.reached.iter().fold((0, 0), |(analyzed, opaque), r| {
        (
            analyzed + r.analysis.analyzed_statements,
            opaque + r.analysis.opaque_statements,
        )
    })
}

/// Attach the coverage note to every finding whose kernel was read only in
/// part — all five codes, not just RC001.
///
/// It used to be built inline inside `rc001_divergent_barriers` and nowhere
/// else, so an RC002 on a kernel with two opaque statements said nothing
/// about them, and the RC003/RC004/RC005 sites in `analysis.rs` could not
/// have: they run before the uniformity engine and never see an `Analysis`.
/// Attaching it here, once, after every rule has run, is what makes it a
/// property of the analysis rather than of one rule.
///
/// The case a note cannot reach at all — a kernel with no finding, whose
/// divergent barrier is spelled in `asm!` — is answered by the run-level
/// tally in [`run_coverage`], which the summary line reads.
pub fn annotate_coverage(models: &CrateModels, results: &Results, findings: &mut [Finding]) {
    for ctx in &results.contexts {
        let (analyzed, opaque) = kernel_coverage(ctx);
        if opaque == 0 {
            continue;
        }
        let total = analyzed + opaque;
        let name = &models.fns[ctx.kernel].name;
        let note = format!("coverage: {analyzed} of {total} statements analyzed ({opaque} opaque)");
        for finding in findings.iter_mut() {
            if finding.kernel.as_deref() == Some(name.as_str())
                && !finding.notes.iter().any(|n| n.starts_with("coverage: "))
            {
                finding.notes.push(note.clone());
            }
        }
    }
}

/// The whole target's coverage, for the `findings.v1` document: every
/// function some kernel reaches, each counted once.
///
/// This is the number that answers a *clean* run: `--strict` exiting 0 over
/// a kernel whose `bar.sync 0` sits inside an `asm!` block used to read as
/// a clean bill of health, and the tally that would have distinguished it
/// was sitting in `unimap.v1` a few bytes away. Since #189 it also counts
/// the calls the analysis could not follow — a function pointer, a `dyn`
/// method, a function in a sibling crate — which used to be read as
/// barrier-free.
#[must_use]
pub fn run_coverage(results: &Results) -> RunCoverage {
    let mut coverage = RunCoverage {
        analyzed_statements: 0,
        opaque_statements: 0,
        opaque_functions: 0,
    };
    let mut counted = BTreeSet::new();
    for reached in results.contexts.iter().flat_map(|ctx| &ctx.reached) {
        if !counted.insert(reached.id) {
            continue;
        }
        let analysis = &reached.analysis;
        coverage.analyzed_statements += analysis.analyzed_statements;
        coverage.opaque_statements += analysis.opaque_statements;
        if analysis.opaque_statements > 0 {
            coverage.opaque_functions += 1;
        }
    }
    coverage
}

/// A chain hop that only shuffles one unnamed temporary into another adds
/// nothing to the displayed story; the full graph stays in the unimap.
fn is_temp_plumbing(detail: &str) -> bool {
    detail.starts_with('_')
        && detail
            .split_once(": ")
            .is_some_and(|(_, rest)| rest.starts_with("derived from divergent _"))
}

/// Build the `unimap.v1` functions for the analyzed kernels.
pub fn build_unimap(models: &CrateModels, results: &Results) -> Vec<unimap::Function> {
    results
        .contexts
        .iter()
        .map(|ctx| {
            let analysis = &ctx.root().analysis;
            let f = &models.fns[ctx.kernel];
            unimap::Function {
                name: f.name.clone(),
                item: f.item_path.clone(),
                span: span_of(models, f.span),
                coverage: Some(unimap::Coverage {
                    analyzed_statements: analysis.analyzed_statements,
                    opaque_statements: analysis.opaque_statements,
                }),
                values: values_of(models, f, analysis),
                provenance: provenance_edges(f, analysis),
                blocks: blocks_of(models, f, analysis),
            }
        })
        .collect()
}

fn values_of(models: &CrateModels, f: &FnModel, analysis: &Analysis) -> Vec<unimap::Value> {
    (0..f.local_count)
        .map(|local| {
            let uniformity = match analysis.locals[local] {
                Uniformity::Uniform => unimap::Uniformity::Uniform,
                Uniformity::Divergent => unimap::Uniformity::Divergent,
            };
            let source = match &analysis.reasons[local] {
                Some(reason) => Some(value_source(&reason.kind, reason.source_call)),
                None if (1..=f.arg_count).contains(&local) => {
                    Some(unimap::ValueSource::KernelParam)
                }
                None => None,
            };
            unimap::Value {
                id: format!("_{local}"),
                name: f.local_names[local].clone(),
                uniformity,
                source,
                span: f.local_spans[local]
                    .map_or_else(|| span_of(models, f.span), |s| span_of(models, s)),
            }
        })
        .collect()
}

fn value_source(kind: &ReasonKind, call: Option<CallKind>) -> unimap::ValueSource {
    match kind {
        ReasonKind::Source => match call {
            Some(CallKind::ThreadIndexWitness) => unimap::ValueSource::ThreadIndex,
            Some(CallKind::AtomicRmw) => unimap::ValueSource::AtomicReturn,
            _ => unimap::ValueSource::Derived,
        },
        // A helper's return value or a parameter a call passed: derived,
        // from a source in another function.
        ReasonKind::DerivedFrom(_) | ReasonKind::Parameter | ReasonKind::Returned { .. } => {
            unimap::ValueSource::Derived
        }
        // A value written on only some lanes' paths is the MIR-level phi.
        ReasonKind::ControlDependent { .. } => unimap::ValueSource::DivergentPhi,
    }
}

fn provenance_edges(f: &FnModel, analysis: &Analysis) -> Vec<unimap::ProvenanceEdge> {
    let mut edges = Vec::new();
    for (local, reason) in analysis.reasons.iter().enumerate() {
        let Some(reason) = reason else { continue };
        match &reason.kind {
            ReasonKind::DerivedFrom(uses) => {
                for use_local in uses {
                    edges.push(unimap::ProvenanceEdge {
                        from: format!("_{use_local}"),
                        to: format!("_{local}"),
                        what: Some(reason.detail.clone()),
                    });
                }
            }
            ReasonKind::ControlDependent { branch } => {
                if let Some(cause) =
                    analysis.block_cause[*branch].or_else(|| match &f.blocks[*branch].term.kind {
                        TermKind::Branch { cond, .. } => Some(engine::BranchCause {
                            block: *branch,
                            cond: *cond,
                            span: f.blocks[*branch].term.span,
                        }),
                        _ => None,
                    })
                {
                    edges.push(unimap::ProvenanceEdge {
                        from: format!("_{}", cause.cond),
                        to: format!("_{local}"),
                        what: Some("written under thread-divergent control".to_string()),
                    });
                }
            }
            ReasonKind::Source | ReasonKind::Parameter | ReasonKind::Returned { .. } => {}
        }
    }
    edges
}

fn blocks_of(models: &CrateModels, f: &FnModel, analysis: &Analysis) -> Vec<unimap::Block> {
    f.blocks
        .iter()
        .enumerate()
        .map(|(b, block)| {
            let mut values: Vec<String> = block
                .stmts
                .iter()
                .filter_map(|s| s.dest)
                .map(|l| format!("_{l}"))
                .collect();
            if let TermKind::Call {
                dest: Some(dest), ..
            } = &block.term.kind
            {
                values.push(format!("_{dest}"));
            }
            values.dedup();
            unimap::Block {
                id: format!("bb{b}"),
                divergent_control: analysis.block_divergent[b],
                span: Some(span_of(models, block.term.span)),
                values,
            }
        })
        .collect()
}
