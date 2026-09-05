//! RC001 and the `unimap.v1` artifact, from the core engine's results.

use reconverge_artifacts::findings::{
    Confidence, Finding, ProvenanceStep, RunCoverage, SourceSpan,
};
use reconverge_artifacts::unimap;
use reconverge_artifacts::witness::{
    FindingRef, LaneChange, LaneState, Launch, Step, Verdict, WitnessArtifact,
};
use reconverge_core::Uniformity;
use reconverge_core::analysis::{self as engine, Analysis, ReasonKind, Summaries};
use reconverge_core::dialect::CallKind;
use reconverge_core::inline::{MAX_DEPTH, inline_calls};
use reconverge_core::model::{FnId, FnModel, TermKind};
use reconverge_witness::{NoWitness, Replay, SiteKind, ascii_warp_diagram};

use crate::adapt::CrateModels;

/// Analyze every kernel with the core engine.
pub fn analyze_kernels(models: &CrateModels) -> Vec<(FnId, Analysis)> {
    let summaries = Summaries::compute(&models.fns);
    models
        .kernels
        .iter()
        .map(|&id| (id, engine::analyze(&models.fns[id], &summaries)))
        .collect()
}

fn span_of(models: &CrateModels, span_ref: usize) -> SourceSpan {
    models.spans[span_ref].clone()
}

/// Attempt a witness replay for a direct site. On success: promote the
/// finding to `confirmed`, add the concrete configuration and the ASCII
/// warp diagram to its notes (they render in text and SARIF alike), and
/// assemble the `witness.v1` artifact.
///
/// Interprocedural sites are never replayed: the summary bit only says the
/// callee *may* reach a barrier or collective, which is not a concrete
/// trace to stand behind.
/// Everything a replay attempt needs to know about the finding's site.
struct ReplaySite {
    block: usize,
    kind: SiteKind,
    cause_span: usize,
    /// Diagram glyph for lanes that reach the call (`W` barrier, `A`
    /// collective).
    arrived_glyph: char,
}

fn try_witness(
    models: &CrateModels,
    f: &FnModel,
    finding: &mut Finding,
    site: &ReplaySite,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    // One warp first — a divergent lane pair inside a warp is the common
    // case. When that finds nothing and the kernel's launch contract
    // declares a one-dimensional block of several whole warps, replay the
    // declared block: whole-warp divergence (a `warp_id()` guard) has no
    // divergent pair inside 32 lanes and only exists at the declared size.
    let outcome =
        match reconverge_witness::replay_outcome(f, site.block, site.kind, site.cause_span) {
            Ok(replay) => Ok(replay),
            Err(one_warp) => match f.declared_block {
                // The declared shape is the launch this kernel claims, so when
                // it is replayed its answer is the one to report.
                Some([x, 1, 1]) if x > 32 => reconverge_witness::replay_outcome_at(
                    f,
                    site.block,
                    site.kind,
                    site.cause_span,
                    x,
                ),
                _ => Err(one_warp),
            },
        };
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
    promote(models, f, finding, &replay, site.arrived_glyph, witnesses);
}

/// Promote a finding on a concrete replay: the launch, the verdict, the
/// ASCII warp diagram, and the `witness.v1` artifact.
fn promote(
    models: &CrateModels,
    f: &FnModel,
    finding: &mut Finding,
    replay: &Replay,
    arrived_glyph: char,
    witnesses: &mut Vec<WitnessArtifact>,
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
    witnesses.push(witness_artifact(models, f, finding, replay));
}

/// Replay an interprocedural site by splicing the callee in (#29).
///
/// The summary tier is unchanged and still the fallback: this does not
/// promote on "the callee *may* reach a barrier", it removes the call so
/// there is an actual path to replay. Anything the inliner refuses —
/// recursion, too many frames, too many blocks — leaves the finding
/// exactly where it was.
fn try_witness_through_inlining(
    models: &CrateModels,
    f: &FnModel,
    finding: &mut Finding,
    call_block: usize,
    cause_span: usize,
    arrived_glyph: char,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    let Some(inlined) = inline_calls(&models.fns, f, MAX_DEPTH) else {
        return;
    };
    let Some((_, sites)) = inlined.exposed.iter().find(|(b, _)| *b == call_block) else {
        return;
    };
    for &block in sites {
        let Some(kind) = site_kind_of(&inlined.model.blocks[block]) else {
            continue;
        };
        if let Ok(replay) =
            reconverge_witness::replay_outcome(&inlined.model, block, kind, cause_span)
        {
            finding.notes.push(
                "witness: the callee was inlined at the call site, so this is a \
                 concrete path rather than a summary bit"
                    .to_string(),
            );
            promote(models, f, finding, &replay, arrived_glyph, witnesses);
            return;
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
    WitnessArtifact {
        schema: reconverge_witness::emitted_schema().to_string(),
        tool: reconverge_artifacts::findings::ToolInfo::current(),
        krate: String::new(), // filled at write time with the crate name
        kernel: f.name.clone(),
        finding: Some(FindingRef {
            code: finding.code.clone(),
            span: Some(finding.span.clone()),
        }),
        launch: Launch {
            grid: replay.grid,
            block: replay.block,
            // A one-warp replay is warp 0; a whole-block replay is not a
            // single warp's view.
            warp: (replay.block[0] == 32).then_some(0),
        },
        lanes: u8::try_from(replay.block[0]).unwrap_or(32),
        initial_lane_states: vec![LaneState::Active; replay.block[0] as usize],
        steps: replay
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
            .collect(),
        verdict: Verdict {
            kind: replay.verdict_kind,
            message: replay.verdict_message.clone(),
            step: Some(replay.verdict_step),
        },
    }
}

/// RC001 (warning until a witness confirms it, per docs/ARCHITECTURE.md):
/// a barrier reachable under thread-divergent control.
pub fn rc001_divergent_barriers(
    models: &CrateModels,
    results: &[(FnId, Analysis)],
    findings: &mut Vec<Finding>,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    for (fn_id, analysis) in results {
        let f = &models.fns[*fn_id];
        for site in &analysis.barriers {
            let Some(cause) = site.divergent_cause else {
                continue;
            };
            let message = if site.interprocedural {
                format!(
                    "kernel `{}` calls `{}`, which may execute a barrier, under \
                     thread-divergent control",
                    f.name, site.callee_display
                )
            } else {
                format!(
                    "kernel `{}` may execute `{}()` under thread-divergent control",
                    f.name, site.callee_display
                )
            };

            let mut notes = vec![
                "lanes that skip the barrier never arrive, and the lanes that reach it wait \
                 for them forever — on hardware this is undefined behavior, usually a \
                 permanent hang with no error"
                    .to_string(),
            ];
            if analysis.irreducible {
                notes.push(
                    "this function has irreducible control flow; the analysis degraded to \
                     all-divergent for the whole body"
                        .to_string(),
                );
            }
            let mut provenance = vec![ProvenanceStep {
                what: "thread-divergent branch".to_string(),
                span: span_of(models, cause.span),
            }];
            provenance.extend(
                engine::provenance_chain(f, analysis, cause.cond)
                    .into_iter()
                    .filter(|step| !is_temp_plumbing(&step.detail))
                    .map(|step| ProvenanceStep {
                        what: step.detail,
                        span: span_of(models, step.span),
                    }),
            );

            let mut finding = Finding {
                code: "RC001".to_string(),
                confidence: Confidence::Warning,
                message,
                kernel: Some(f.name.clone()),
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
            if site.interprocedural {
                try_witness_through_inlining(
                    models,
                    f,
                    &mut finding,
                    site.block,
                    cause.span,
                    'W',
                    witnesses,
                );
            } else {
                try_witness(
                    models,
                    f,
                    &mut finding,
                    &ReplaySite {
                        block: site.block,
                        kind: SiteKind::Barrier,
                        cause_span: cause.span,
                        arrived_glyph: 'W',
                    },
                    witnesses,
                );
            }
            findings.push(finding);
        }
    }
}

/// RC002 (warning until a witness confirms it, per docs/ARCHITECTURE.md): a
/// warp collective at a point where threads of the warp may be inactive.
///
/// Scope, from a survey of upstream's examples: they use FULL_MASK
/// literals or runtime-computed masks, and essentially no constant partial
/// masks — so the check is about convergence, with constant masks reported
/// as context (mask refinement) rather than arithmetically verified.
pub fn rc002_nonconvergent_warp_ops(
    models: &CrateModels,
    results: &[(FnId, Analysis)],
    findings: &mut Vec<Finding>,
    witnesses: &mut Vec<WitnessArtifact>,
) {
    for (fn_id, analysis) in results {
        let f = &models.fns[*fn_id];
        for site in &analysis.warp_ops {
            let Some(cause) = site.divergent_cause else {
                continue;
            };
            let message = if site.interprocedural {
                format!(
                    "kernel `{}` calls `{}`, which may execute a warp collective, under \
                     thread-divergent control",
                    f.name, site.callee_display
                )
            } else {
                format!(
                    "kernel `{}` calls `{}()` at a point where threads of the warp may be \
                     inactive",
                    f.name, site.callee_display
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
                    "the mask is not a literal the analysis can evaluate (a runtime value, or a \
                     named const — opaque through rustc_public at this pin), so it cannot be \
                     checked statically"
                        .to_string(),
                ),
                None => {}
            }
            if analysis.irreducible {
                notes.push(
                    "this function has irreducible control flow; the analysis degraded to \
                     all-divergent for the whole body"
                        .to_string(),
                );
            }

            let mut provenance = vec![ProvenanceStep {
                what: "thread-divergent branch".to_string(),
                span: span_of(models, cause.span),
            }];
            provenance.extend(
                engine::provenance_chain(f, analysis, cause.cond)
                    .into_iter()
                    .filter(|step| !is_temp_plumbing(&step.detail))
                    .map(|step| ProvenanceStep {
                        what: step.detail,
                        span: span_of(models, step.span),
                    }),
            );

            let mut finding = Finding {
                code: "RC002".to_string(),
                confidence: Confidence::Warning,
                message,
                kernel: Some(f.name.clone()),
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
            if site.interprocedural {
                try_witness_through_inlining(
                    models,
                    f,
                    &mut finding,
                    site.block,
                    cause.span,
                    'A',
                    witnesses,
                );
            } else {
                try_witness(
                    models,
                    f,
                    &mut finding,
                    &ReplaySite {
                        block: site.block,
                        kind: SiteKind::Collective { mask: site.mask },
                        cause_span: cause.span,
                        arrived_glyph: 'A',
                    },
                    witnesses,
                );
            }
            findings.push(finding);
        }
    }
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
pub fn annotate_coverage(
    models: &CrateModels,
    results: &[(FnId, Analysis)],
    findings: &mut [Finding],
) {
    for (fn_id, analysis) in results {
        if analysis.opaque_statements == 0 {
            continue;
        }
        let total = analysis.analyzed_statements + analysis.opaque_statements;
        if total == 0 {
            continue;
        }
        let name = &models.fns[*fn_id].name;
        let note = format!(
            "coverage: {} of {total} statements analyzed ({} opaque)",
            analysis.analyzed_statements, analysis.opaque_statements
        );
        for finding in findings.iter_mut() {
            if finding.kernel.as_deref() == Some(name.as_str())
                && !finding.notes.iter().any(|n| n.starts_with("coverage: "))
            {
                finding.notes.push(note.clone());
            }
        }
    }
}

/// The whole target's coverage, for the `findings.v1` document.
///
/// This is the number that answers a *clean* run: `--strict` exiting 0 over
/// a kernel whose `bar.sync 0` sits inside an `asm!` block used to read as
/// a clean bill of health, and the tally that would have distinguished it
/// was sitting in `unimap.v1` a few bytes away.
#[must_use]
pub fn run_coverage(results: &[(FnId, Analysis)]) -> RunCoverage {
    let mut coverage = RunCoverage {
        analyzed_statements: 0,
        opaque_statements: 0,
        opaque_functions: 0,
    };
    for (_, analysis) in results {
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
pub fn build_unimap(models: &CrateModels, results: &[(FnId, Analysis)]) -> Vec<unimap::Function> {
    results
        .iter()
        .map(|(fn_id, analysis)| {
            let f = &models.fns[*fn_id];
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
        ReasonKind::DerivedFrom(_) => unimap::ValueSource::Derived,
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
            ReasonKind::Source => {}
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
