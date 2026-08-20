//! The uniformity dataflow (docs/ARCHITECTURE.md).
//!
//! Per-local lattice `Uniform ⊑ Divergent`, optimistic initialization,
//! iterate to fixpoint. Divergent branch conditions mark their control
//! region — every block reachable from a successor short of the branch's
//! immediate post-dominator — as divergent-control; any definition inside
//! such a region is divergent (this subsumes divergent phis: after
//! reconvergence, a value written on only some lanes' paths differs across
//! lanes). Reconvergence is the post-dominator, so a barrier *after* the
//! join point is clean.
//!
//! Provenance is recorded during the dataflow, not reconstructed after:
//! the first reason each local turned divergent survives, and
//! [`provenance_chain`] walks reasons back to a source.
//!
//! Irreducible CFGs degrade to all-divergent for the whole function, and
//! the result says so (`Analysis::irreducible`).

use crate::Uniformity;
use crate::dialect::CallKind;
use crate::graph::{self, Cfg};
use crate::model::{BlockId, FnModel, Local, SpanRef, TermKind};

/// Interprocedural summary bits (docs/ARCHITECTURE.md): whether each function may
/// execute a barrier / warp collective, directly or transitively.
#[derive(Debug, Clone)]
pub struct Summaries {
    pub may_contain_barrier: Vec<bool>,
    pub may_contain_warp_op: Vec<bool>,
}

impl Summaries {
    /// Fixpoint over the local call graph.
    #[must_use]
    pub fn compute(fns: &[FnModel]) -> Summaries {
        let n = fns.len();
        let mut barrier = vec![false; n];
        let mut warp = vec![false; n];
        let mut changed = true;
        while changed {
            changed = false;
            for (i, f) in fns.iter().enumerate() {
                for block in &f.blocks {
                    if let TermKind::Call { callee, .. } = &block.term.kind {
                        let (b, w) = match callee.kind {
                            CallKind::Barrier => (true, false),
                            CallKind::WarpCollective { .. } => (false, true),
                            _ => callee
                                .local_fn
                                .map_or((false, false), |c| (barrier[c], warp[c])),
                        };
                        if b && !barrier[i] {
                            barrier[i] = true;
                            changed = true;
                        }
                        if w && !warp[i] {
                            warp[i] = true;
                            changed = true;
                        }
                    }
                }
            }
        }
        Summaries {
            may_contain_barrier: barrier,
            may_contain_warp_op: warp,
        }
    }
}

/// Why a local is divergent — the first cause wins and is stable.
#[derive(Debug, Clone)]
pub struct Reason {
    pub kind: ReasonKind,
    pub span: SpanRef,
    /// Human-readable one-liner, e.g. "thread-index witness `index_1d()`".
    pub detail: String,
    /// For `Source` reasons rooted in a call: what kind of call it was.
    pub source_call: Option<CallKind>,
}

#[derive(Debug, Clone)]
pub enum ReasonKind {
    /// A divergence source in its own right (index witness, atomic, …).
    Source,
    /// Derived from already-divergent inputs.
    DerivedFrom(Vec<Local>),
    /// Written under thread-divergent control.
    ControlDependent { branch: BlockId },
}

/// The branch that made a block divergent-control.
#[derive(Debug, Clone, Copy)]
pub struct BranchCause {
    pub block: BlockId,
    pub cond: Local,
    pub span: SpanRef,
}

/// A barrier-relevant call site.
#[derive(Debug, Clone)]
pub struct BarrierSite {
    pub block: BlockId,
    pub span: SpanRef,
    /// What was called (`sync_threads`, or a local function for the
    /// interprocedural case).
    pub callee_display: String,
    /// True when this is a call to a local function that *may* execute a
    /// barrier, rather than a direct barrier.
    pub interprocedural: bool,
    /// Set when the site executes under thread-divergent control.
    pub divergent_cause: Option<BranchCause>,
}

/// A warp-collective call site (RC002's subject).
#[derive(Debug, Clone)]
pub struct WarpOpSite {
    pub block: BlockId,
    pub span: SpanRef,
    pub callee_display: String,
    /// True for a call into a local function that may execute a warp
    /// collective, rather than a direct collective.
    pub interprocedural: bool,
    /// Set when the site executes under thread-divergent control.
    pub divergent_cause: Option<BranchCause>,
    /// The participation mask, when it is a literal constant at the call
    /// (direct calls only; the mask is the collective's first argument).
    pub mask: Option<u64>,
}

/// Analysis result for one function.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub locals: Vec<Uniformity>,
    pub reasons: Vec<Option<Reason>>,
    pub block_divergent: Vec<bool>,
    pub block_cause: Vec<Option<BranchCause>>,
    /// The CFG was irreducible; everything was degraded to divergent.
    pub irreducible: bool,
    pub analyzed_statements: usize,
    pub opaque_statements: usize,
    pub barriers: Vec<BarrierSite>,
    pub warp_ops: Vec<WarpOpSite>,
}

/// Run the uniformity dataflow over one function.
#[must_use]
pub fn analyze(f: &FnModel, summaries: &Summaries) -> Analysis {
    let n = f.blocks.len();
    let cfg = Cfg::build(f);
    let reachable = graph::reachable(&cfg, 0);
    let idom = graph::dominators(&cfg, 0);

    let mut locals = vec![Uniformity::Uniform; f.local_count];
    let mut reasons: Vec<Option<Reason>> = vec![None; f.local_count];
    let mut block_divergent = vec![false; n];
    let mut block_cause: Vec<Option<BranchCause>> = vec![None; n];

    let irreducible = !graph::is_reducible(&cfg, 0, &idom);
    if irreducible {
        // §5: degrade to all-divergent in this function and say so.
        for (local, slot) in locals.iter_mut().enumerate() {
            *slot = Uniformity::Divergent;
            reasons[local] = Some(Reason {
                kind: ReasonKind::Source,
                span: f.span,
                detail: "irreducible control flow: analysis degraded to all-divergent".into(),
                source_call: None,
            });
        }
        block_divergent.copy_from_slice(&reachable);
    }

    // Only genuine returns anchor reconvergence; aborting dead-ends
    // (unreachable arms, panic calls) must not stretch divergence regions.
    let exits: Vec<bool> = f
        .blocks
        .iter()
        .map(|block| matches!(block.term.kind, TermKind::Return))
        .collect();
    let ipdom = graph::post_dominators(&cfg, &exits);

    // Optimistic fixpoint. Both lattices only move upward, so this
    // terminates; the outer loop re-runs until nothing changes.
    let mut changed = !irreducible;
    while changed {
        changed = false;
        for b in (0..n).filter(|&b| reachable[b]) {
            let under_divergent_control = block_divergent[b];

            let raise = |locals: &mut Vec<Uniformity>,
                         reasons: &mut Vec<Option<Reason>>,
                         dest: Local,
                         value: Uniformity,
                         reason: Reason|
             -> bool {
                if value == Uniformity::Divergent && locals[dest] == Uniformity::Uniform {
                    locals[dest] = Uniformity::Divergent;
                    reasons[dest] = Some(reason);
                    true
                } else {
                    false
                }
            };

            for stmt in &f.blocks[b].stmts {
                let Some(dest) = stmt.dest else { continue };
                if stmt.opaque {
                    // Opaque code (inline asm outputs): conservative source.
                    changed |= raise(
                        &mut locals,
                        &mut reasons,
                        dest,
                        Uniformity::Divergent,
                        Reason {
                            kind: ReasonKind::Source,
                            span: stmt.span,
                            detail: "written by opaque code (inline asm)".into(),
                            source_call: None,
                        },
                    );
                    continue;
                }
                let divergent_use: Option<Local> = stmt
                    .uses
                    .iter()
                    .copied()
                    .find(|&u| locals[u] == Uniformity::Divergent);
                let (value, reason) = if let Some(u) = divergent_use {
                    (
                        Uniformity::Divergent,
                        Reason {
                            kind: ReasonKind::DerivedFrom(vec![u]),
                            span: stmt.span,
                            detail: format!("derived from divergent {}", f.local_display(u)),
                            source_call: None,
                        },
                    )
                } else if under_divergent_control {
                    (
                        Uniformity::Divergent,
                        Reason {
                            kind: ReasonKind::ControlDependent { branch: b },
                            span: stmt.span,
                            detail: "written under thread-divergent control".into(),
                            source_call: None,
                        },
                    )
                } else {
                    continue;
                };
                changed |= raise(&mut locals, &mut reasons, dest, value, reason);
            }

            match &f.blocks[b].term.kind {
                TermKind::Call {
                    callee,
                    args,
                    dest: Some(dest),
                    ..
                } => {
                    {
                        let dest = *dest;
                        let base = callee.kind.result_base();
                        let divergent_arg = args
                            .iter()
                            .copied()
                            .find(|&a| locals[a] == Uniformity::Divergent);
                        // A uniform-classified result still diverges when
                        // written under divergent control: lanes outside
                        // the region keep their old value.
                        let outcome = if base == Some(Uniformity::Divergent)
                            || divergent_arg.is_some()
                            || under_divergent_control
                        {
                            Uniformity::Divergent
                        } else {
                            Uniformity::Uniform
                        };
                        if outcome == Uniformity::Divergent {
                            let reason = match base {
                                Some(Uniformity::Divergent) => Reason {
                                    kind: ReasonKind::Source,
                                    span: f.blocks[b].term.span,
                                    detail: source_detail(callee.kind, &callee.display),
                                    source_call: Some(callee.kind),
                                },
                                _ => {
                                    if let Some(a) = divergent_arg {
                                        Reason {
                                            kind: ReasonKind::DerivedFrom(vec![a]),
                                            span: f.blocks[b].term.span,
                                            detail: format!(
                                                "result of `{}` on divergent {}",
                                                callee.display,
                                                f.local_display(a)
                                            ),
                                            source_call: None,
                                        }
                                    } else {
                                        Reason {
                                            kind: ReasonKind::ControlDependent { branch: b },
                                            span: f.blocks[b].term.span,
                                            detail: "written under thread-divergent control".into(),
                                            source_call: None,
                                        }
                                    }
                                }
                            };
                            changed |= raise(
                                &mut locals,
                                &mut reasons,
                                dest,
                                Uniformity::Divergent,
                                reason,
                            );
                        }
                    }
                }
                TermKind::Opaque {
                    dest: Some(dest), ..
                } => {
                    changed |= raise(
                        &mut locals,
                        &mut reasons,
                        *dest,
                        Uniformity::Divergent,
                        Reason {
                            kind: ReasonKind::Source,
                            span: f.blocks[b].term.span,
                            detail: "result of opaque code (inline asm)".into(),
                            source_call: None,
                        },
                    );
                }
                TermKind::Branch { cond, .. } if locals[*cond] == Uniformity::Divergent => {
                    let cause = BranchCause {
                        block: b,
                        cond: *cond,
                        span: f.blocks[b].term.span,
                    };
                    for (r, in_region) in graph::divergence_region(&cfg, b, ipdom[b])
                        .into_iter()
                        .enumerate()
                    {
                        if in_region && !block_divergent[r] {
                            block_divergent[r] = true;
                            block_cause[r] = Some(cause);
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Coverage honesty (§5): statements plus terminators, opaque counted
    // separately.
    let mut analyzed = 0usize;
    let mut opaque = 0usize;
    for b in (0..n).filter(|&b| reachable[b]) {
        for stmt in &f.blocks[b].stmts {
            if stmt.opaque {
                opaque += 1;
            } else {
                analyzed += 1;
            }
        }
        match &f.blocks[b].term.kind {
            TermKind::Opaque { .. } => opaque += 1,
            _ => analyzed += 1,
        }
    }

    // Barrier and warp-collective sites: direct calls, and calls into
    // local functions whose summary bits say they may execute one.
    let mut barriers = Vec::new();
    let mut warp_ops = Vec::new();
    for b in (0..n).filter(|&b| reachable[b]) {
        let TermKind::Call {
            callee, const_args, ..
        } = &f.blocks[b].term.kind
        else {
            continue;
        };
        let divergent_cause = if block_divergent[b] {
            block_cause[b]
        } else {
            None
        };

        let barrier = match callee.kind {
            CallKind::Barrier => Some(false),
            _ => callee
                .local_fn
                .filter(|&c| summaries.may_contain_barrier[c])
                .map(|_| true),
        };
        if let Some(interprocedural) = barrier {
            barriers.push(BarrierSite {
                block: b,
                span: f.blocks[b].term.span,
                callee_display: callee.display.clone(),
                interprocedural,
                divergent_cause,
            });
        }

        let warp = match callee.kind {
            CallKind::WarpCollective { .. } => Some(false),
            _ => callee
                .local_fn
                .filter(|&c| summaries.may_contain_warp_op[c])
                .map(|_| true),
        };
        if let Some(interprocedural) = warp {
            warp_ops.push(WarpOpSite {
                block: b,
                span: f.blocks[b].term.span,
                callee_display: callee.display.clone(),
                interprocedural,
                divergent_cause,
                // By the dialect convention the participation mask is
                // the collective's first argument — except for the
                // unmasked wrappers, which take no mask argument at all
                // and supply a full one themselves, so it is known from
                // the call rather than read off it.
                mask: if interprocedural || callee.kind.mask_is_unknown() {
                    None
                } else if let Some(implicit) = callee.kind.implicit_mask() {
                    implicit
                        // A wrapper supplies the mask itself; its first
                        // argument is a value, not a mask.
                        .into()
                } else {
                    const_args.first().copied().flatten()
                },
            });
        }
    }

    Analysis {
        locals,
        reasons,
        block_divergent,
        block_cause,
        irreducible,
        analyzed_statements: analyzed,
        opaque_statements: opaque,
        barriers,
        warp_ops,
    }
}

fn source_detail(kind: CallKind, display: &str) -> String {
    match kind {
        CallKind::ThreadIndexWitness => format!("thread-index witness `{display}()`"),
        CallKind::AtomicRmw => format!("atomic return value of `{display}()`"),
        CallKind::WarpCollective { .. } => {
            format!("warp-collective result of `{display}()`")
        }
        CallKind::DivergentEnvRead => format!("per-lane environment read `{display}()`"),
        _ => format!("result of `{display}()`"),
    }
}

/// One hop of a provenance chain.
#[derive(Debug, Clone)]
pub struct ProvenanceStep {
    pub detail: String,
    pub span: SpanRef,
}

/// Walk a divergent local's recorded reasons back to a source
/// (def→use chain, §5: provenance is mandatory). The chain starts at the
/// given local and ends at a `Source` (or a cycle/uniform stop).
#[must_use]
pub fn provenance_chain(f: &FnModel, analysis: &Analysis, start: Local) -> Vec<ProvenanceStep> {
    let mut chain = Vec::new();
    let mut visited = vec![false; f.local_count];
    let mut current = start;
    loop {
        if visited[current] {
            break;
        }
        visited[current] = true;
        let Some(reason) = &analysis.reasons[current] else {
            break;
        };
        chain.push(ProvenanceStep {
            detail: format!("{}: {}", f.local_display(current), reason.detail),
            span: reason.span,
        });
        match &reason.kind {
            ReasonKind::Source => break,
            ReasonKind::DerivedFrom(uses) => {
                let Some(&next) = uses.first() else { break };
                current = next;
            }
            ReasonKind::ControlDependent { branch } => {
                let Some(cause) = analysis.block_cause[*branch].or_else(|| {
                    // The branch block itself carries the divergent
                    // condition when it is the region root.
                    match &f.blocks[*branch].term.kind {
                        TermKind::Branch { cond, .. } => Some(BranchCause {
                            block: *branch,
                            cond: *cond,
                            span: f.blocks[*branch].term.span,
                        }),
                        _ => None,
                    }
                }) else {
                    break;
                };
                current = cause.cond;
            }
        }
    }
    chain
}
