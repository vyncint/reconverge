//! The engine's function model — a deliberately small, dialect-agnostic
//! mirror of MIR, built by a driver-side adapter.
//!
//! Spans are opaque handles (`SpanRef`): the engine records where things
//! happened, the driver owns the table that maps handles back to real
//! source spans. This keeps the engine free of compiler and serde types,
//! and unit-testable with hand-built models.

use crate::dialect::CallKind;

/// A local slot, mirroring MIR numbering: `0` is the return slot and
/// `1..=arg_count` are the parameters.
pub type Local = usize;
/// Basic-block index within one function.
pub type BlockId = usize;
/// Index of a function within the crate's model set.
pub type FnId = usize;
/// Opaque span handle; the driver owns the mapping to real spans.
pub type SpanRef = usize;

/// One function, ready for analysis.
#[derive(Debug, Clone)]
pub struct FnModel {
    /// User-facing name (the kernel base name for kernels).
    pub name: String,
    /// Fully qualified item path.
    pub item_path: String,
    /// Where the function is defined.
    pub span: SpanRef,
    /// Number of locals, including the return place and the arguments.
    pub local_count: usize,
    /// Locals `1..=arg_count` are parameters (uniform by docs/ARCHITECTURE.md).
    pub arg_count: usize,
    /// Source-level names, where debug info provides them.
    pub local_names: Vec<Option<String>>,
    /// Where each local is declared, where debug info provides it.
    pub local_spans: Vec<Option<SpanRef>>,
    /// Basic blocks, indexed by [`BlockId`]; block 0 is the entry.
    pub blocks: Vec<Block>,
    /// Block dimensions declared by the kernel's `#[launch_contract]`
    /// (`block = (X, Y, Z)`), when present — the launch shape a witness may
    /// replay beyond one warp.
    pub declared_block: Option<[u32; 3]>,
    /// Cluster dimensions declared by the kernel's `#[cluster_launch(X, Y, Z)]`,
    /// when present. Recorded so a cluster-scoped barrier can be reported
    /// against the shape the kernel claims; the witness still replays one
    /// block, so this does not widen a replay.
    pub declared_cluster: Option<[u32; 3]>,
}

/// One basic block: statements, then a terminator.
#[derive(Debug, Clone)]
pub struct Block {
    /// Statements, in order.
    pub stmts: Vec<Stmt>,
    /// The terminator.
    pub term: Term,
}

/// A statement, reduced to def/use structure plus (when expressible)
/// evaluable semantics for the witness interpreter.
#[derive(Debug, Clone)]
pub struct Stmt {
    /// The local written, when the destination is (part of) a local.
    /// Stores through pointers have no modeled destination.
    pub dest: Option<Local>,
    /// Every local read: operands, place bases, and index locals.
    pub uses: Vec<Local>,
    /// Computable semantics, when the right-hand side is simple enough for
    /// the witness interpreter. `None` makes the destination unknown at
    /// replay time (the dataflow above is unaffected).
    pub eval: Option<Eval>,
    /// The statement could not be modeled (inline asm, an unmodeled
    /// intrinsic); it is counted as coverage rather than guessed at.
    pub opaque: bool,
    /// Where the statement is.
    pub span: SpanRef,
}

/// An interpreter operand: a local slot or an integer literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// A local slot.
    Local(Local),
    /// An integer literal, zero-extended to 128 bits.
    Const(u128),
}

/// Evaluable right-hand sides (the witness interpreter's kernel subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eval {
    /// Plain copy, reference-to-scalar, or literal.
    Use(Operand),
    /// A binary operation on two operands, wrapping at 128 bits.
    Binary(BinOp, Operand, Operand),
    /// Unary operation evaluated at the operand's own width in bits.
    ///
    /// The width is the whole point: the store is an untyped `u128`, so a
    /// width-less `!x` has to guess which type's complement it is taking.
    /// It used to guess "boolean", which is exact for a condition and
    /// wrong for every mask. At width `n`, `Not` is the bitwise complement
    /// within `n` bits — which *is* boolean negation when `n` is 1, so the
    /// condition case falls out rather than being special-cased.
    Unary(UnOp, Operand, u32),
    /// An integer cast to a target of the given width: the operand
    /// truncated to `bits`.
    ///
    /// Widening is the identity on the store's zero-extended embedding,
    /// which is why treating every cast as the identity looked correct on
    /// the small thread-index values replays used to traffic in. Narrowing
    /// is a real truncation, and a mask narrowed by the identity keeps
    /// high bits the program has already discarded.
    Cast(Operand, u32),
    /// Overflow-checked arithmetic on an unsigned integer of the given
    /// width in bits (debug builds lower `+`/`-`/`*` to this). The checked
    /// form panics the thread on overflow, so past the width the result is
    /// not a value the program ever sees: the interpreter yields the exact
    /// in-range value or unknown, never a wrapped one.
    CheckedBinary(BinOp, Operand, Operand, u32),
}

/// Integer/boolean operators the interpreter evaluates. Comparison results
/// are 0/1; arithmetic wraps at 128 bits (kernels index with unsigned
/// values far below that). Width-sensitive operations do not live here —
/// see [`Eval::Unary`], [`Eval::Cast`] and [`Eval::CheckedBinary`], which
/// carry the width they need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Remainder.
    Rem,
    /// Bitwise and.
    BitAnd,
    /// Bitwise or.
    BitOr,
    /// Bitwise exclusive or.
    BitXor,
    /// Shift left.
    Shl,
    /// Shift right.
    Shr,
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
}

/// Unary operators the interpreter evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// Bitwise complement — boolean negation at width 1.
    Not,
    /// Arithmetic negation.
    Neg,
}

/// A block terminator.
#[derive(Debug, Clone)]
pub struct Term {
    /// What the terminator does.
    pub kind: TermKind,
    /// Where it is.
    pub span: SpanRef,
}

/// The terminators the model distinguishes.
#[derive(Debug, Clone)]
pub enum TermKind {
    /// An unconditional jump.
    Goto {
        /// The block jumped to.
        target: BlockId,
    },
    /// A multi-way branch on a local's value.
    Branch {
        /// The local tested.
        cond: Local,
        /// Successor blocks, aligned with `values`.
        targets: Vec<BlockId>,
        /// Guard values aligned with `targets` for the interpreter:
        /// `Some(v)` takes its target when the condition equals `v`, `None`
        /// is the otherwise edge. Empty when the mapping was not modeled
        /// (the interpreter then treats the branch as unknown).
        values: Vec<Option<u128>>,
    },
    /// A multi-way jump whose discriminant is a constant: never divergent.
    Jump {
        /// Successor blocks.
        targets: Vec<BlockId>,
    },
    /// A call; its classification lives in `callee`.
    Call {
        /// What is called.
        callee: Callee,
        /// Every local the call reads — flattened; see `const_args`.
        args: Vec<Local>,
        /// Per **original argument position**: the argument's value when it
        /// is a literal integer constant (e.g. a warp participation mask).
        /// `args` above is the flattened set of locals the call reads and
        /// does not correspond position-wise.
        const_args: Vec<Option<u64>>,
        /// Per original argument position, the interpreter-usable operand
        /// (a plain local, a reference to one, or a literal), when the
        /// argument is that simple.
        arg_operands: Vec<Option<Operand>>,
        /// The local the result is written to, when any.
        dest: Option<Local>,
        /// The block control continues in when the call returns.
        target: Option<BlockId>,
    },
    /// Inline asm or similar: opaque to the analysis.
    Opaque {
        /// Locals the opaque statement reads.
        uses: Vec<Local>,
        /// The local it writes, when known.
        dest: Option<Local>,
        /// The block control continues in.
        target: Option<BlockId>,
    },
    /// A normal return — the one exit that constrains reconvergence.
    Return,
    /// No successors and no normal return (unreachable, abort, resume).
    Halt,
}

/// A classified call target.
#[derive(Debug, Clone)]
pub struct Callee {
    /// The dialect's classification.
    pub kind: CallKind,
    /// How far this barrier's participant set reaches, from
    /// [`SimtDialect::barrier_scope`](crate::dialect::SimtDialect::barrier_scope).
    /// Meaningful only when `kind` is [`CallKind::Barrier`]; the dialect is
    /// not asked about anything else, and everything else carries
    /// [`LaunchScope::Block`](crate::LaunchScope::Block).
    pub scope: crate::LaunchScope,
    /// Human-facing name for diagnostics (a trimmed path).
    pub display: String,
    /// The callee's index in the crate model set, when it is a local
    /// function whose body was modeled (drives the interprocedural
    /// summary bits).
    pub local_fn: Option<FnId>,
}

impl FnModel {
    /// Successor blocks along normal (non-unwind) edges.
    #[must_use]
    pub fn successors(&self, block: BlockId) -> Vec<BlockId> {
        match &self.blocks[block].term.kind {
            TermKind::Goto { target } => vec![*target],
            TermKind::Branch { targets, .. } | TermKind::Jump { targets } => targets.clone(),
            TermKind::Call { target, .. } | TermKind::Opaque { target, .. } => {
                target.map(|t| vec![t]).unwrap_or_default()
            }
            TermKind::Return | TermKind::Halt => Vec::new(),
        }
    }

    /// A display name for a local: its debug name, or `_N`.
    #[must_use]
    pub fn local_display(&self, local: Local) -> String {
        self.local_names
            .get(local)
            .and_then(Clone::clone)
            .map_or_else(|| format!("_{local}"), |name| format!("`{name}`"))
    }
}
