//! Bounded inlining of local callees, so a barrier behind a helper can be
//! replayed.
//!
//! Interprocedural findings stay at `warning` because the summary bit says
//! a callee *may* reach a barrier, and "may" is not a trace to stand
//! behind. That reasoning is sound, and this module does not weaken it: it
//! removes the call instead, splicing the callee's own blocks into the
//! caller so the barrier becomes a site on a concrete path. What gets
//! replayed is then an ordinary intraprocedural trace.
//!
//! Deliberately bounded, because the fallback is already correct:
//! non-recursive callees, at most [`MAX_DEPTH`] frames, at most
//! [`MAX_BLOCKS`] blocks in the result. Anything outside those bounds
//! yields `None` and the caller keeps the summary tier.

use crate::dialect::CallKind;
use crate::model::{Block, BlockId, Eval, FnId, FnModel, Local, Operand, Stmt, Term, TermKind};

/// Frames of inlining. Two covers a helper calling a helper, which is as
/// deep as the shapes this exists for go.
pub const MAX_DEPTH: u32 = 2;

/// A ceiling on the spliced result. Inlining is a replay aid, not a
/// program transformation, and an unbounded one would trade a `warning`
/// for a very slow `warning`.
pub const MAX_BLOCKS: usize = 4096;

/// A model with its local calls spliced in, and the sites that exposed.
pub struct Inlined {
    pub model: FnModel,
    /// Barrier and collective blocks that only exist because a call was
    /// inlined, paired with the caller block whose call produced them.
    /// These are the sites an interprocedural finding can now be replayed
    /// against.
    pub exposed: Vec<(BlockId, Vec<BlockId>)>,
}

/// Splice non-recursive local callees of `f` into it, up to `depth`.
///
/// Returns `None` when nothing was inlined, or when the bounds above are
/// exceeded — in both cases the caller should keep its existing behaviour.
#[must_use]
pub fn inline_calls(fns: &[FnModel], f: &FnModel, depth: u32) -> Option<Inlined> {
    let mut active = Vec::new();
    let mut out = f.clone();
    let mut exposed = Vec::new();
    if !splice(fns, &mut out, depth, &mut active, &mut exposed) {
        return None;
    }
    if exposed.is_empty() || out.blocks.len() > MAX_BLOCKS {
        return None;
    }
    Some(Inlined {
        model: out,
        exposed,
    })
}

/// Splice one level of calls in `caller`, recursing into each callee.
/// Returns false when a bound is hit and the result must be discarded.
fn splice(
    fns: &[FnModel],
    caller: &mut FnModel,
    depth: u32,
    active: &mut Vec<FnId>,
    exposed: &mut Vec<(BlockId, Vec<BlockId>)>,
) -> bool {
    if depth == 0 {
        return true;
    }
    for b in 0..caller.blocks.len() {
        let TermKind::Call {
            callee,
            arg_operands,
            dest,
            target: Some(target),
            ..
        } = &caller.blocks[b].term.kind
        else {
            continue;
        };
        // Only a modeled local body can be spliced, and only an ordinary
        // call: a barrier or collective *is* the site, not a frame.
        let (Some(id), CallKind::Other) = (callee.local_fn, callee.kind) else {
            continue;
        };
        if active.contains(&id) || id >= fns.len() {
            continue; // recursion, or a callee outside this crate's models
        }
        let (arg_operands, dest, target) = (arg_operands.clone(), *dest, *target);

        // Inline the callee's own calls first, so depth counts frames.
        let mut body = fns[id].clone();
        active.push(id);
        let mut nested = Vec::new();
        let ok = splice(fns, &mut body, depth - 1, active, &mut nested);
        active.pop();
        if !ok {
            return false;
        }

        let base = caller.blocks.len();
        let local_base = caller.local_count;
        if base + body.blocks.len() > MAX_BLOCKS {
            return false;
        }
        caller.local_count += body.local_count;
        caller
            .local_names
            .extend(std::iter::repeat_n(None, body.local_count));
        caller
            .local_spans
            .extend(std::iter::repeat_n(None, body.local_count));

        // The callee's return slot flows to the call's destination, and
        // its returns become jumps back to the call's successor.
        let ret_block = caller_return_block(&body, local_base, dest, target, base);
        let ret_id = base + body.blocks.len();

        let mut sites = Vec::new();
        for (i, block) in body.blocks.iter().enumerate() {
            let mut block = block.clone();
            remap_block(&mut block, local_base, base, ret_id);
            if is_site(&block) {
                sites.push(base + i);
            }
            caller.blocks.push(block);
        }
        caller.blocks.push(ret_block);
        for (_, inner) in nested {
            sites.extend(inner.into_iter().map(|s| s + base));
        }

        // Bind the parameters at the callee's entry, then enter it.
        let binds = param_binds(&body, local_base, &arg_operands);
        caller.blocks[base].stmts.splice(0..0, binds);
        caller.blocks[b].term.kind = TermKind::Goto { target: base };
        if !sites.is_empty() {
            exposed.push((b, sites));
        }
    }
    true
}

/// `dest = <callee return slot>; goto <call successor>`.
fn caller_return_block(
    body: &FnModel,
    local_base: Local,
    dest: Option<Local>,
    target: BlockId,
    _base: BlockId,
) -> Block {
    let stmts = dest
        .map(|d| Stmt {
            dest: Some(d),
            uses: vec![local_base],
            eval: Some(Eval::Use(Operand::Local(local_base))),
            opaque: false,
            span: body.span,
        })
        .into_iter()
        .collect();
    Block {
        stmts,
        term: Term {
            kind: TermKind::Goto { target },
            span: body.span,
        },
    }
}

/// `param_i = <the caller's i-th argument>`, where the argument is simple
/// enough to be an operand. An argument that is not stays unknown, exactly
/// as it would have been across the call.
fn param_binds(body: &FnModel, local_base: Local, arg_operands: &[Option<Operand>]) -> Vec<Stmt> {
    (0..body.arg_count)
        .filter_map(|i| {
            let operand = (*arg_operands.get(i)?)?;
            let param = local_base + 1 + i;
            Some(Stmt {
                dest: Some(param),
                uses: match operand {
                    Operand::Local(l) => vec![l],
                    Operand::Const(_) => Vec::new(),
                },
                eval: Some(Eval::Use(operand)),
                opaque: false,
                span: body.span,
            })
        })
        .collect()
}

fn is_site(block: &Block) -> bool {
    matches!(
        &block.term.kind,
        TermKind::Call { callee, .. }
            if matches!(callee.kind, CallKind::Barrier | CallKind::WarpCollective { .. })
    )
}

/// Shift a spliced block's locals and block ids into the caller's space.
fn remap_block(block: &mut Block, local_base: Local, block_base: BlockId, ret_id: BlockId) {
    let l = |x: &mut Local| *x += local_base;
    let op = |o: &mut Operand| {
        if let Operand::Local(x) = o {
            *x += local_base;
        }
    };
    for s in &mut block.stmts {
        if let Some(d) = &mut s.dest {
            l(d);
        }
        for u in &mut s.uses {
            l(u);
        }
        if let Some(e) = &mut s.eval {
            match e {
                Eval::Use(a) | Eval::Unary(_, a, _) | Eval::Cast(a, _) => op(a),
                Eval::Binary(_, a, b) | Eval::CheckedBinary(_, a, b, _) => {
                    op(a);
                    op(b);
                }
            }
        }
    }
    let t = |x: &mut BlockId| *x += block_base;
    match &mut block.term.kind {
        TermKind::Goto { target } => t(target),
        TermKind::Branch { cond, targets, .. } => {
            l(cond);
            targets.iter_mut().for_each(t);
        }
        TermKind::Jump { targets } => targets.iter_mut().for_each(t),
        TermKind::Call {
            args,
            arg_operands,
            dest,
            target,
            ..
        } => {
            args.iter_mut().for_each(l);
            arg_operands.iter_mut().flatten().for_each(op);
            if let Some(d) = dest {
                l(d);
            }
            if let Some(tt) = target {
                t(tt);
            }
        }
        TermKind::Opaque { uses, dest, target } => {
            uses.iter_mut().for_each(l);
            if let Some(d) = dest {
                l(d);
            }
            if let Some(tt) = target {
                t(tt);
            }
        }
        // A callee's return is the caller's continuation.
        TermKind::Return => block.term.kind = TermKind::Goto { target: ret_id },
        TermKind::Halt => {}
    }
}
