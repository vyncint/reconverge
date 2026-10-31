//! Divergence across function boundaries (#188).
//!
//! [`Summaries`] answer what a call does on its own — may it reach a
//! barrier, does it return something divergent. This module answers the
//! other half: what a helper's *parameters* hold, so a barrier inside the
//! helper is checked against the arguments it is actually given.
//!
//! The analysis is per kernel and context-insensitive inside it: every
//! function a kernel reaches is analyzed once, with each parameter joined
//! over every call to it from that kernel's call tree. A parameter is
//! divergent when any call passes it a divergent argument, and only as
//! constant as the narrowest argument passed. Keying the join by kernel is
//! what lets a finding inside a helper name the kernels it was actually
//! reached from with divergent arguments, rather than every kernel that
//! calls it at all.
//!
//! What this does not do is distinguish two calls from the same kernel:
//! `reduce_step(tid)` and `reduce_step(0)` in one kernel analyze the helper
//! once, with `tid`. A finding that depends on that join is reported at
//! `warning` like any other, and only a concrete replay — the bounded
//! inliner, which does tell the calls apart — promotes it.

use crate::Uniformity;
use crate::analysis::{
    Analysis, ParamSeed, ProvenanceStep, Reason, ReasonKind, Summaries, analyze, analyze_with,
    provenance_chain,
};
use crate::graph::{self, Cfg};
use crate::model::{BlockId, FnId, FnModel, Local, SpanRef, TermKind};

/// Where a parameter's divergence came from: the first call found passing
/// it a divergent argument.
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    /// The calling function.
    pub caller: FnId,
    /// The block whose terminator is the call.
    pub block: BlockId,
    /// The caller's divergent local that was passed.
    pub local: Local,
    /// Where the call is.
    pub span: SpanRef,
}

/// One function a kernel reaches, analyzed with the parameters that kernel
/// gives it.
#[derive(Debug, Clone)]
pub struct Reached {
    /// The function.
    pub id: FnId,
    /// Its analysis, under the joined parameters.
    pub analysis: Analysis,
    /// Per parameter: where its divergence came from, when it is divergent.
    pub origins: Vec<Option<Origin>>,
}

/// Everything one kernel reaches.
#[derive(Debug, Clone)]
pub struct KernelContext {
    /// The kernel.
    pub kernel: FnId,
    /// The kernel first, then every local function it reaches, in
    /// discovery order.
    pub reached: Vec<Reached>,
}

impl KernelContext {
    /// The kernel's own entry.
    #[must_use]
    pub fn root(&self) -> &Reached {
        &self.reached[0]
    }

    /// The entry for one reached function.
    #[must_use]
    pub fn get(&self, id: FnId) -> Option<&Reached> {
        self.reached.iter().find(|r| r.id == id)
    }
}

/// The local functions one call may run: its direct callee and whatever
/// it may run indirectly, each flagged `true` when it is the direct one.
fn call_targets(fns: &[FnModel], term: &TermKind) -> Vec<(FnId, bool)> {
    let TermKind::Call { callee, .. } = term else {
        return Vec::new();
    };
    callee
        .local_fn
        .map(|c| (c, true))
        .into_iter()
        .chain(callee.may_call.iter().map(|&c| (c, false)))
        .filter(|&(c, _)| c < fns.len())
        .collect()
}

/// Every local function reachable from `root` through direct calls and
/// indirect ones, `root` first.
#[must_use]
pub fn reachable(fns: &[FnModel], root: FnId) -> Vec<FnId> {
    let mut order = vec![root];
    let mut i = 0;
    while i < order.len() {
        let f = order[i];
        i += 1;
        for block in &fns[f].blocks {
            for (c, _) in call_targets(fns, &block.term.kind) {
                if !order.contains(&c) {
                    order.push(c);
                }
            }
        }
    }
    order
}

/// The locals the call passes for parameter `param` of the callee.
///
/// Positional where the model says which locals each argument reads; a
/// closure called through `Fn::call` receives its arguments as one tuple,
/// so every parameter past the last argument reads that one. An indirect
/// target — a closure handed to a library function — is passed something
/// the library chose, which is only known to come from the call's own
/// arguments: all of them, joined.
fn passed(term: &TermKind, param: usize, direct: bool) -> Vec<Local> {
    let TermKind::Call { args, arg_uses, .. } = term else {
        return Vec::new();
    };
    if !direct || arg_uses.is_empty() {
        return args.clone();
    }
    arg_uses[param.min(arg_uses.len() - 1)].clone()
}

/// Analyze one kernel and every function it reaches.
#[must_use]
pub fn analyze_kernel(fns: &[FnModel], summaries: &Summaries, kernel: FnId) -> KernelContext {
    let mut order = vec![kernel];
    let mut seeds: Vec<Vec<ParamSeed>> = vec![Vec::new(); fns.len()];
    let mut origins: Vec<Vec<Option<Origin>>> = vec![Vec::new(); fns.len()];
    let mut seen = vec![false; fns.len()];
    seen[kernel] = true;

    // Both facts only move one way — a parameter turns divergent, a scope
    // narrows — so this reaches a fixpoint.
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = 0;
        while i < order.len() {
            let f = order[i];
            i += 1;
            let model = &fns[f];
            let a = analyze_with(model, summaries, &seeds[f]);
            let cfg = Cfg::build(model);
            let live = graph::reachable(&cfg, 0);
            for (b, block) in model.blocks.iter().enumerate() {
                if !live[b] {
                    continue;
                }
                for (g, direct) in call_targets(fns, &block.term.kind) {
                    if !seen[g] {
                        seen[g] = true;
                        order.push(g);
                        changed = true;
                    }
                    let arity = fns[g].arg_count;
                    if seeds[g].len() < arity {
                        seeds[g].resize(arity, ParamSeed::default());
                        origins[g].resize(arity, None);
                    }
                    for p in 0..arity {
                        let locals = passed(&block.term.kind, p, direct);
                        let scope = locals.iter().fold(crate::LaunchScope::Grid, |acc, &l| {
                            acc.meet(a.local_scopes[l])
                        });
                        if scope < seeds[g][p].scope {
                            seeds[g][p].scope = scope;
                            changed = true;
                        }
                        if seeds[g][p].divergent.is_some() {
                            continue;
                        }
                        let Some(&local) = locals
                            .iter()
                            .find(|&&l| a.locals[l] == Uniformity::Divergent)
                        else {
                            continue;
                        };
                        seeds[g][p].divergent = Some(Reason {
                            kind: ReasonKind::Parameter,
                            span: block.term.span,
                            detail: format!(
                                "parameter receives a divergent argument from `{}`",
                                model.name
                            ),
                            source_call: None,
                        });
                        origins[g][p] = Some(Origin {
                            caller: f,
                            block: b,
                            local,
                            span: block.term.span,
                        });
                        changed = true;
                    }
                }
            }
        }
    }

    let reached = order
        .into_iter()
        .map(|id| Reached {
            id,
            analysis: analyze_with(&fns[id], summaries, &seeds[id]),
            origins: std::mem::take(&mut origins[id]),
        })
        .collect();
    KernelContext { kernel, reached }
}

/// [`provenance_chain`] across function boundaries: from a local of one
/// reached function back to a source, following a divergent parameter into
/// the call that passed it and a divergent return value into the function
/// that returned it. The hop count is bounded, so a cycle through recursion
/// ends the chain rather than the analysis.
#[must_use]
pub fn provenance(
    fns: &[FnModel],
    summaries: &Summaries,
    ctx: &KernelContext,
    func: FnId,
    start: Local,
) -> Vec<ProvenanceStep> {
    let mut steps = Vec::new();
    let Some(reached) = ctx.get(func) else {
        return steps;
    };
    // A returned value is followed through the callee analyzed on its own,
    // which is what the summary was computed from; owned, since it is not
    // one of the context's analyses.
    let mut owned: Option<Analysis> = None;
    let mut current = (func, start, Some(reached));
    for _ in 0..32 {
        let (f, local, in_ctx) = current;
        let analysis = match in_ctx {
            Some(r) => &r.analysis,
            None => match &owned {
                Some(a) => a,
                None => break,
            },
        };
        steps.extend(provenance_chain(&fns[f], analysis, local));
        let Some(last) = last_local(&fns[f], analysis, local) else {
            break;
        };
        let next = match analysis.reasons[last].as_ref().map(|r| &r.kind) {
            Some(ReasonKind::Parameter) => Hop::Caller(last),
            Some(ReasonKind::Returned { callee }) if *callee < fns.len() => Hop::Callee(*callee),
            _ => break,
        };
        match next {
            Hop::Caller(param_local) => {
                let Some(Some(origin)) =
                    in_ctx.and_then(|r| r.origins.get(param_local.wrapping_sub(1)))
                else {
                    break;
                };
                current = (origin.caller, origin.local, ctx.get(origin.caller));
            }
            Hop::Callee(callee) => {
                owned = Some(analyze(&fns[callee], summaries));
                current = (callee, 0, None);
            }
        }
    }
    steps
}

/// Where a chain goes after it stops at a function boundary.
enum Hop {
    /// Into the call that passed this parameter local a divergent argument.
    Caller(Local),
    /// Into the return slot of the function that returned it.
    Callee(FnId),
}

/// The local [`provenance_chain`] stops at, walking the same way.
fn last_local(f: &FnModel, analysis: &Analysis, start: Local) -> Option<Local> {
    let mut visited = vec![false; f.local_count];
    let mut current = start;
    loop {
        if visited[current] {
            return None;
        }
        visited[current] = true;
        let reason = analysis.reasons[current].as_ref()?;
        match &reason.kind {
            ReasonKind::Source | ReasonKind::Parameter | ReasonKind::Returned { .. } => {
                return Some(current);
            }
            ReasonKind::DerivedFrom(uses) => current = *uses.first()?,
            ReasonKind::ControlDependent { branch } => {
                let cond = analysis.block_cause[*branch].map(|c| c.cond).or_else(|| {
                    match &f.blocks[*branch].term.kind {
                        TermKind::Branch { cond, .. } => Some(*cond),
                        _ => None,
                    }
                })?;
                current = cond;
            }
        }
    }
}
