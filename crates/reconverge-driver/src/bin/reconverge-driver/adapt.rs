//! Stable MIR → engine-model adapter.
//!
//! Builds `reconverge_core::model::FnModel`s for every local function with
//! a body, classifying call targets through the dialect
//! (`SimtDialect::classify_call`). Spans become indices into a side table
//! the driver owns, so the engine stays free of compiler types.
//!
//! Modeling notes:
//! - Only normal control edges are followed; unwind/cleanup edges are
//!   dropped, which leaves cleanup blocks unreachable and out of the
//!   analysis.
//! - A store through a pointer (`*p = …`) defines no local; a projection
//!   write (`x.f = …`) is modeled as a (joining) definition of the base.
//! - Inline asm is opaque: each output place becomes an opaque statement
//!   (a conservative divergence source), and the terminator itself counts
//!   toward the opaque-coverage tally.

use std::collections::HashMap;

use reconverge_artifacts::findings::SourceSpan;
use reconverge_core::dialect::SimtDialect;
use reconverge_core::model::{
    self, Block, Callee, Eval, FnId, FnModel, Local, SpanRef, Stmt, Term, TermKind,
};
use reconverge_dialect_oxide::kernel_base_name;
use rustc_public::mir::{
    Body, Operand, Place, ProjectionElem, Statement, StatementKind, TerminatorKind,
    VarDebugInfoContents,
};
use rustc_public::ty::{IntTy, RigidTy, Span, Ty, TyKind, UintTy};
use rustc_public::{CrateDef, CrateItem, ItemKind};

use crate::emit;

/// Models for every local function, plus the span table and the subset of
/// functions that are kernels.
pub struct CrateModels {
    pub fns: Vec<FnModel>,
    /// Indices into `fns` for detected kernels, sorted by kernel name.
    pub kernels: Vec<FnId>,
    /// The span table `SpanRef` indexes into.
    pub spans: Vec<SourceSpan>,
}

pub fn build(dialect: &dyn SimtDialect) -> CrateModels {
    let mut items: Vec<CrateItem> = rustc_public::all_local_items()
        .into_iter()
        .filter(|item| item.kind() == ItemKind::Fn && item.has_body())
        .collect();
    items.sort_by_key(|a| a.name());

    let path_to_id: HashMap<String, FnId> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.name(), i))
        .collect();

    let mut spans = Vec::new();
    let mut fns = Vec::new();
    for item in &items {
        let body = item.body().expect("filtered on has_body");
        fns.push(adapt_fn(item, &body, dialect, &path_to_id, &mut spans));
    }

    let mut kernels: Vec<FnId> = (0..fns.len())
        .filter(|&i| kernel_base_name(&fns[i].item_path).is_some())
        .collect();
    kernels.sort_by(|&a, &b| fns[a].name.cmp(&fns[b].name));

    CrateModels {
        fns,
        kernels,
        spans,
    }
}

fn intern(spans: &mut Vec<SourceSpan>, span: Span) -> SpanRef {
    spans.push(emit::source_span(span));
    spans.len() - 1
}

fn adapt_fn(
    item: &CrateItem,
    body: &Body,
    dialect: &dyn SimtDialect,
    path_to_id: &HashMap<String, FnId>,
    spans: &mut Vec<SourceSpan>,
) -> FnModel {
    let path = item.name();
    let name = kernel_base_name(&path)
        .map(str::to_string)
        .unwrap_or_else(|| item.trimmed_name());

    let local_count = body.locals().len();
    let mut local_names: Vec<Option<String>> = vec![None; local_count];
    for info in &body.var_debug_info {
        if let VarDebugInfoContents::Place(place) = &info.value
            && place.projection.is_empty()
            && local_names[place.local].is_none()
        {
            local_names[place.local] = Some(info.name.clone());
        }
    }
    let local_spans: Vec<Option<SpanRef>> = body
        .locals()
        .iter()
        .map(|decl| Some(intern(spans, decl.span)))
        .collect();

    let overflow_tuples = overflow_tuple_locals(body);
    let declared_block = body.blocks.iter().find_map(|bb| {
        if let TerminatorKind::Call { func, .. } = &bb.terminator.kind {
            block_config_dims(func)
        } else {
            None
        }
    });
    let blocks = body
        .blocks
        .iter()
        .map(|bb| {
            let mut stmts: Vec<Stmt> = bb
                .statements
                .iter()
                .filter_map(|s| adapt_stmt(s, body, &overflow_tuples, spans))
                .collect();
            let term = adapt_term(
                &bb.terminator.kind,
                bb.terminator.span,
                dialect,
                path_to_id,
                spans,
                &mut stmts,
            );
            Block { stmts, term }
        })
        .collect();

    FnModel {
        name,
        item_path: path,
        span: intern(spans, item.span()),
        local_count,
        arg_count: body.arg_locals().len(),
        local_names,
        local_spans,
        blocks,
        declared_block,
    }
}

/// The `(X, Y, Z)` a `#[launch_contract(block = …)]` declares, read from
/// the const generics of the `__launch_contract_block_config::<X, Y, Z>()`
/// marker the macro plants in the kernel body.
fn block_config_dims(func: &Operand) -> Option<[u32; 3]> {
    let Operand::Constant(constant) = func else {
        return None;
    };
    let TyKind::RigidTy(RigidTy::FnDef(def, args)) = constant.const_.ty().kind() else {
        return None;
    };
    if !def.name().ends_with("__launch_contract_block_config") {
        return None;
    }
    let dims: Vec<u32> = args
        .0
        .iter()
        .filter_map(|arg| {
            if let rustc_public::ty::GenericArgKind::Const(c) = arg {
                u32::try_from(ty_const_uint(c)?).ok()
            } else {
                None
            }
        })
        .collect();
    <[u32; 3]>::try_from(dims).ok()
}

/// The value of an unsigned const generic argument.
fn ty_const_uint(c: &rustc_public::ty::TyConst) -> Option<u128> {
    if let rustc_public::ty::TyConstKind::Value(_, allocation) = c.kind()
        && allocation.provenance.ptrs.is_empty()
        && !allocation.bytes.is_empty()
        && allocation.bytes.len() <= 16
    {
        return allocation.read_uint().ok();
    }
    None
}

/// Locals that hold the `(value, overflowed)` pair of an overflow-checked
/// arithmetic operation and nothing else, ever. Debug builds lower
/// `n += 1` to `_t = CheckedBinaryOp(..); assert(!(_t.1)); _n = (_t.0)`,
/// with the assert as a block terminator — so the pair's definition and its
/// field reads span blocks, and the set must be computed function-wide.
/// A local also assigned by anything else is excluded outright.
fn overflow_tuple_locals(body: &Body) -> Vec<bool> {
    use rustc_public::mir::{Rvalue, StatementKind};
    let mut checked = vec![false; body.locals().len()];
    let mut poisoned = vec![false; body.locals().len()];
    for bb in &body.blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(place, rvalue) = &stmt.kind {
                if !place.projection.is_empty() {
                    poisoned[place.local] = true;
                } else if matches!(rvalue, Rvalue::CheckedBinaryOp(..)) {
                    checked[place.local] = true;
                } else {
                    poisoned[place.local] = true;
                }
            }
        }
        // A terminator writing the local also disqualifies it.
        match &bb.terminator.kind {
            TerminatorKind::Call {
                destination: place, ..
            } => poisoned[place.local] = true,
            TerminatorKind::InlineAsm { operands, .. } => {
                for op in operands {
                    if let Some(out) = &op.out_place {
                        poisoned[out.local] = true;
                    }
                }
            }
            _ => {}
        }
    }
    checked
        .iter()
        .zip(&poisoned)
        .map(|(&c, &p)| c && !p)
        .collect()
}

fn adapt_stmt(
    stmt: &Statement,
    body: &Body,
    overflow_tuples: &[bool],
    spans: &mut Vec<SourceSpan>,
) -> Option<Stmt> {
    match &stmt.kind {
        StatementKind::Assign(place, rvalue) => {
            let (dest, mut uses) = write_target(place);
            uses.extend(rvalue_locals(rvalue));
            // Semantics only make sense for whole-local destinations.
            let eval = if place.projection.is_empty() {
                rvalue_eval(rvalue, body, overflow_tuples)
            } else {
                None
            };
            Some(Stmt {
                dest,
                uses,
                eval,
                opaque: false,
                span: intern(spans, stmt.span),
            })
        }
        StatementKind::SetDiscriminant { place, .. } => {
            let (dest, uses) = write_target(place);
            Some(Stmt {
                dest,
                uses,
                eval: None,
                opaque: false,
                span: intern(spans, stmt.span),
            })
        }
        // Storage markers, retags, coverage, fake reads, intrinsics like
        // assume/copy_nonoverlapping (memory-to-memory): no local defs.
        _ => None,
    }
}

/// The modeled destination of a write to `place`, plus the locals the
/// place expression itself reads.
fn write_target(place: &Place) -> (Option<Local>, Vec<Local>) {
    let mut uses = index_locals(place);
    if place
        .projection
        .iter()
        .any(|p| matches!(p, ProjectionElem::Deref))
    {
        // Store through a pointer: the base local is read, no local is
        // defined (memory uniformity is not tracked).
        uses.push(place.local);
        (None, uses)
    } else {
        // Whole or partial (field/index) definition of the local; the
        // flow-insensitive join keeps partial writes sound.
        (Some(place.local), uses)
    }
}

fn index_locals(place: &Place) -> Vec<Local> {
    place
        .projection
        .iter()
        .filter_map(|p| match p {
            ProjectionElem::Index(local) => Some(*local),
            _ => None,
        })
        .collect()
}

fn place_locals(place: &Place) -> Vec<Local> {
    let mut locals = vec![place.local];
    locals.extend(index_locals(place));
    locals
}

fn operand_locals(operand: &Operand) -> Vec<Local> {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => place_locals(place),
        _ => Vec::new(),
    }
}

fn rvalue_locals(rvalue: &rustc_public::mir::Rvalue) -> Vec<Local> {
    use rustc_public::mir::Rvalue;
    match rvalue {
        Rvalue::Use(op)
        | Rvalue::Repeat(op, _)
        | Rvalue::Cast(_, op, _)
        | Rvalue::UnaryOp(_, op) => operand_locals(op),
        Rvalue::BinaryOp(_, a, b) | Rvalue::CheckedBinaryOp(_, a, b) => {
            let mut locals = operand_locals(a);
            locals.extend(operand_locals(b));
            locals
        }
        Rvalue::Aggregate(_, ops) => ops.iter().flat_map(operand_locals).collect(),
        Rvalue::AddressOf(_, place)
        | Rvalue::Ref(_, _, place)
        | Rvalue::CopyForDeref(place)
        | Rvalue::Discriminant(place)
        | Rvalue::Len(place) => place_locals(place),
        Rvalue::ThreadLocalRef(_) => Vec::new(),
    }
}

fn adapt_term(
    kind: &TerminatorKind,
    span: Span,
    dialect: &dyn SimtDialect,
    path_to_id: &HashMap<String, FnId>,
    spans: &mut Vec<SourceSpan>,
    stmts: &mut Vec<Stmt>,
) -> Term {
    let span_ref = intern(spans, span);
    let kind = match kind {
        TerminatorKind::Goto { target } => TermKind::Goto { target: *target },
        TerminatorKind::SwitchInt { discr, targets } => {
            let mut all: Vec<usize> = targets.branches().map(|(_, b)| b).collect();
            let mut values: Vec<Option<u128>> = targets.branches().map(|(v, _)| Some(v)).collect();
            all.push(targets.otherwise());
            values.push(None);
            match operand_locals(discr).first() {
                Some(&cond) => TermKind::Branch {
                    cond,
                    targets: all,
                    values,
                },
                None => match const_scalar(discr) {
                    // Constant discriminant: resolve the edge right here.
                    Some(value) => {
                        let target = targets
                            .branches()
                            .find(|(v, _)| *v == value)
                            .map_or(targets.otherwise(), |(_, b)| b);
                        TermKind::Goto { target }
                    }
                    None => TermKind::Jump { targets: all },
                },
            }
        }
        TerminatorKind::Call {
            func,
            args,
            destination,
            target,
            ..
        } => {
            let (path, display) = callee_path(func);
            let kind = dialect.classify_call(&path);
            let (dest, mut arg_locals) = write_target(destination);
            arg_locals.extend(args.iter().flat_map(operand_locals));
            arg_locals.extend(operand_locals(func));
            let const_args = args
                .iter()
                .map(|arg| const_scalar(arg).and_then(|v| u64::try_from(v).ok()))
                .collect();
            let arg_operands = args.iter().map(model_operand).collect();
            TermKind::Call {
                callee: Callee {
                    kind,
                    display,
                    local_fn: path_to_id.get(&path).copied(),
                },
                args: arg_locals,
                const_args,
                arg_operands,
                dest,
                target: *target,
            }
        }
        TerminatorKind::Assert { target, .. } | TerminatorKind::Drop { target, .. } => {
            TermKind::Goto { target: *target }
        }
        TerminatorKind::InlineAsm {
            operands,
            destination,
            ..
        } => {
            let mut uses = Vec::new();
            for op in operands {
                if let Some(input) = &op.in_value {
                    uses.extend(operand_locals(input));
                }
                if let Some(out) = &op.out_place {
                    // Asm outputs are conservative divergence sources.
                    let (dest, out_uses) = write_target(out);
                    stmts.push(Stmt {
                        dest,
                        uses: out_uses,
                        eval: None,
                        opaque: true,
                        span: span_ref,
                    });
                }
            }
            TermKind::Opaque {
                uses,
                dest: None,
                target: *destination,
            }
        }
        TerminatorKind::Return => TermKind::Return,
        TerminatorKind::Resume | TerminatorKind::Abort | TerminatorKind::Unreachable => {
            TermKind::Halt
        }
    };
    Term {
        kind,
        span: span_ref,
    }
}

/// The value of a literal integer/bool argument (e.g. a warp participation
/// mask), when the operand is a plain scalar constant.
///
/// Deliberately avoids `MirConst::eval_target_usize`, which panics (rather
/// than erroring) on non-`usize` constants; reading the evaluated
/// allocation handles every integer width, endianness-correctly.
fn const_scalar(operand: &Operand) -> Option<u128> {
    let Operand::Constant(constant) = operand else {
        return None;
    };
    if !matches!(
        constant.const_.ty().kind(),
        TyKind::RigidTy(RigidTy::Uint(_) | RigidTy::Int(_) | RigidTy::Bool)
    ) {
        return None;
    }
    // Known limitation: a *named* const operand (`ballot_sync(FULL_MASK,
    // …)`) reaches analysis MIR as `ConstantKind::Unevaluated` and stays
    // unknown, while the same mask spelled as a literal arrives as
    // `Allocated` and evaluates. Re-tested at the pin (#32), against the
    // driver rather than from memory:
    //
    //   - `ConstDef` exposes no `body`, `has_body`, `eval`, `const_value`
    //     or `ty` — there is nothing to read the initializer from.
    //   - `MirConst` exposes exactly one evaluation entry point,
    //     `eval_target_usize()`. It is not a door for a mask: on a `u32`
    //     const it ICEs the compiler with "expected int of size 8, but got
    //     size 4". The assertion fires *after* the value is resolved, so
    //     the value is reachable in principle — what is missing is an API
    //     that returns it at its own width.
    //
    // So the boundary is the exposed surface, not the compiler's ability.
    // Downstream this is why an RC002 with a named-const mask reports the
    // mask as not evaluable and is never witness-promoted.
    if let rustc_public::ty::ConstantKind::Allocated(allocation) = constant.const_.kind()
        && allocation.provenance.ptrs.is_empty()
        && !allocation.bytes.is_empty()
        && allocation.bytes.len() <= 16
    {
        return allocation.read_uint().ok();
    }
    None
}

/// Lower an operand into the interpreter's form: a projection-free local,
/// or an integer literal.
fn model_operand(operand: &Operand) -> Option<model::Operand> {
    match operand {
        Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
            Some(model::Operand::Local(place.local))
        }
        _ => const_scalar(operand).map(model::Operand::Const),
    }
}

/// An operand for the interpreter, seeing through the `.0` of an
/// overflow-checked pair — reading it is reading the arithmetic result
/// the interpreter stored in the pair's local.
fn scalar_operand(operand: &Operand, overflow_tuples: &[bool]) -> Option<model::Operand> {
    if let Operand::Copy(place) | Operand::Move(place) = operand
        && overflow_tuples.get(place.local) == Some(&true)
        && let [ProjectionElem::Field(0, _)] = place.projection.as_slice()
    {
        return Some(model::Operand::Local(place.local));
    }
    model_operand(operand)
}

/// Width in bits of a scalar as the interpreter's store embeds it, and
/// whether it is signed. Everything the store holds is zero-extended from
/// this many bits, `bool` included at one bit. A type with no such width —
/// a float, a pointer, an aggregate — has no answer, and the caller must
/// yield unknown rather than assume one.
///
/// `usize`/`isize` are taken as 64, matching the assumption the
/// overflow-checked form already makes.
fn scalar_width(ty: &Ty) -> Option<(u32, bool)> {
    match ty.kind() {
        TyKind::RigidTy(RigidTy::Bool) => Some((1, false)),
        TyKind::RigidTy(RigidTy::Uint(u)) => Some((
            match u {
                UintTy::U8 => 8,
                UintTy::U16 => 16,
                UintTy::U32 => 32,
                UintTy::U64 | UintTy::Usize => 64,
                UintTy::U128 => 128,
            },
            false,
        )),
        TyKind::RigidTy(RigidTy::Int(i)) => Some((
            match i {
                IntTy::I8 => 8,
                IntTy::I16 => 16,
                IntTy::I32 => 32,
                IntTy::I64 | IntTy::Isize => 64,
                IntTy::I128 => 128,
            },
            true,
        )),
        _ => None,
    }
}

/// Evaluable semantics for simple right-hand sides (the witness
/// interpreter's kernel subset). References to plain locals are
/// value-transparent: the interpreter tracks scalars, not memory.
fn rvalue_eval(
    rvalue: &rustc_public::mir::Rvalue,
    body: &Body,
    overflow_tuples: &[bool],
) -> Option<Eval> {
    use rustc_public::mir::Rvalue;
    match rvalue {
        Rvalue::Use(op) => Some(Eval::Use(scalar_operand(op, overflow_tuples)?)),
        // A cast is not the identity. Widening is, on the store's
        // zero-extended embedding — which is why this used to look right
        // on thread-index values — but narrowing discards high bits the
        // program has already discarded, and a mask is exactly where that
        // shows. A signed source would need sign extension, which the
        // unsigned embedding cannot express, so it stays unknown rather
        // than being approximated.
        Rvalue::Cast(_, op, target) => {
            let (_, from_signed) = scalar_width(&op.ty(body.locals()).ok()?)?;
            if from_signed {
                return None;
            }
            let (to_bits, _) = scalar_width(target)?;
            Some(Eval::Cast(scalar_operand(op, overflow_tuples)?, to_bits))
        }
        Rvalue::Ref(_, _, place) | Rvalue::AddressOf(_, place) | Rvalue::CopyForDeref(place)
            if place.projection.is_empty() =>
        {
            Some(Eval::Use(model::Operand::Local(place.local)))
        }
        Rvalue::BinaryOp(op, a, b) => Some(Eval::Binary(
            model_binop(op)?,
            model_operand(a)?,
            model_operand(b)?,
        )),
        // Debug builds lower `+`/`-`/`*` to the overflow-checked form; the
        // interpreter evaluates it exactly within the type's width and
        // yields unknown past it (the real program panics there — a
        // wrapped value must never be fabricated). Unsigned only: the
        // store's u128 embedding has no signed semantics.
        Rvalue::CheckedBinaryOp(op, a, b) => {
            let bits = match a.ty(body.locals()).ok()?.kind() {
                TyKind::RigidTy(RigidTy::Uint(u)) => match u {
                    UintTy::U8 => 8,
                    UintTy::U16 => 16,
                    UintTy::U32 => 32,
                    UintTy::U64 | UintTy::Usize => 64,
                    UintTy::U128 => return None,
                },
                _ => return None,
            };
            Some(Eval::CheckedBinary(
                model_binop(op)?,
                model_operand(a)?,
                model_operand(b)?,
                bits,
            ))
        }
        // Width-typed: `!x` is the complement of `x`'s own type, not of
        // the 128-bit store. Exact for signed operands too — the store
        // holds two's complement zero-extended from the type's width, so
        // complement and negation within that width are the program's own
        // bit patterns.
        Rvalue::UnaryOp(op, a) => {
            let (bits, _) = scalar_width(&a.ty(body.locals()).ok()?)?;
            Some(Eval::Unary(model_unop(op)?, model_operand(a)?, bits))
        }
        _ => None,
    }
}

fn model_binop(op: &rustc_public::mir::BinOp) -> Option<model::BinOp> {
    use rustc_public::mir::BinOp as B;
    Some(match op {
        B::Add | B::AddUnchecked => model::BinOp::Add,
        B::Sub | B::SubUnchecked => model::BinOp::Sub,
        B::Mul | B::MulUnchecked => model::BinOp::Mul,
        B::Div => model::BinOp::Div,
        B::Rem => model::BinOp::Rem,
        B::BitAnd => model::BinOp::BitAnd,
        B::BitOr => model::BinOp::BitOr,
        B::BitXor => model::BinOp::BitXor,
        B::Shl | B::ShlUnchecked => model::BinOp::Shl,
        B::Shr | B::ShrUnchecked => model::BinOp::Shr,
        B::Eq => model::BinOp::Eq,
        B::Ne => model::BinOp::Ne,
        B::Lt => model::BinOp::Lt,
        B::Le => model::BinOp::Le,
        B::Gt => model::BinOp::Gt,
        B::Ge => model::BinOp::Ge,
        _ => return None,
    })
}

fn model_unop(op: &rustc_public::mir::UnOp) -> Option<model::UnOp> {
    use rustc_public::mir::UnOp as U;
    Some(match op {
        U::Not => model::UnOp::Not,
        U::Neg => model::UnOp::Neg,
        _ => return None,
    })
}

/// The definition path and display name of a call target; indirect calls
/// (function pointers) come back as `Other`-classified placeholders.
fn callee_path(func: &Operand) -> (String, String) {
    if let Operand::Constant(constant) = func
        && let TyKind::RigidTy(RigidTy::FnDef(def, _)) = constant.const_.ty().kind()
    {
        let path = def.name();
        let display = def.trimmed_name();
        return (path, display);
    }
    (String::new(), "<indirect call>".to_string())
}
