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
    pub span: SpanRef,
    pub local_count: usize,
    /// Locals `1..=arg_count` are parameters (uniform by docs/ARCHITECTURE.md).
    pub arg_count: usize,
    /// Source-level names, where debug info provides them.
    pub local_names: Vec<Option<String>>,
    pub local_spans: Vec<Option<SpanRef>>,
    pub blocks: Vec<Block>,
    /// Block dimensions declared by the kernel's `#[launch_contract]`
    /// (`block = (X, Y, Z)`), when present — the launch shape a witness may
    /// replay beyond one warp.
    pub declared_block: Option<[u32; 3]>,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
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
    pub opaque: bool,
    pub span: SpanRef,
}

/// An interpreter operand: a local slot or an integer literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Local(Local),
    Const(u128),
}

/// Evaluable right-hand sides (the witness interpreter's kernel subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eval {
    /// Plain copy, reference-to-scalar, or literal.
    Use(Operand),
    Binary(BinOp, Operand, Operand),
    Unary(UnOp, Operand),
    /// Overflow-checked arithmetic on an unsigned integer of the given
    /// width in bits (debug builds lower `+`/`-`/`*` to this). The checked
    /// form panics the thread on overflow, so past the width the result is
    /// not a value the program ever sees: the interpreter yields the exact
    /// in-range value or unknown, never a wrapped one.
    CheckedBinary(BinOp, Operand, Operand, u32),
}

/// Integer/boolean operators the interpreter evaluates. Comparison results
/// are 0/1; arithmetic wraps at 128 bits (kernels index with unsigned
/// values far below that).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone)]
pub struct Term {
    pub kind: TermKind,
    pub span: SpanRef,
}

#[derive(Debug, Clone)]
pub enum TermKind {
    Goto {
        target: BlockId,
    },
    /// A multi-way branch on a local's value.
    Branch {
        cond: Local,
        targets: Vec<BlockId>,
        /// Guard values aligned with `targets` for the interpreter:
        /// `Some(v)` takes its target when the condition equals `v`, `None`
        /// is the otherwise edge. Empty when the mapping was not modeled
        /// (the interpreter then treats the branch as unknown).
        values: Vec<Option<u128>>,
    },
    /// A multi-way jump whose discriminant is a constant: never divergent.
    Jump {
        targets: Vec<BlockId>,
    },
    Call {
        callee: Callee,
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
        dest: Option<Local>,
        target: Option<BlockId>,
    },
    /// Inline asm or similar: opaque to the analysis.
    Opaque {
        uses: Vec<Local>,
        dest: Option<Local>,
        target: Option<BlockId>,
    },
    Return,
    /// No successors and no normal return (unreachable, abort, resume).
    Halt,
}

/// A classified call target.
#[derive(Debug, Clone)]
pub struct Callee {
    pub kind: CallKind,
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
