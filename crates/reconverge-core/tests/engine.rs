//! Engine acceptance tests on hand-built models — including the
//! canonical pair (README.md):
//! - MUST flag `if idx.get() % 2 == 0 { sync_threads() }`
//! - MUST NOT flag `if block_idx() > 3 { sync_threads() }`

use reconverge_core::Uniformity;
use reconverge_core::analysis::{self, ReasonKind, Summaries};
use reconverge_core::dialect::CallKind;
use reconverge_core::model::{Block, Callee, FnModel, Local, Stmt, Term, TermKind};

fn stmt(dest: Local, uses: &[Local]) -> Stmt {
    Stmt {
        dest: Some(dest),
        uses: uses.to_vec(),
        eval: None,
        opaque: false,
        span: 0,
    }
}

fn term(kind: TermKind) -> Term {
    Term { kind, span: 0 }
}

fn call(kind: CallKind, display: &str, dest: Option<Local>, target: usize) -> TermKind {
    TermKind::Call {
        callee: Callee {
            kind,
            display: display.to_string(),
            local_fn: None,
        },
        args: Vec::new(),
        const_args: Vec::new(),
        arg_operands: Vec::new(),
        dest,
        target: Some(target),
    }
}

fn kernel(local_count: usize, arg_count: usize, blocks: Vec<Block>) -> FnModel {
    FnModel {
        name: "k".into(),
        item_path: "test::k".into(),
        span: 0,
        local_count,
        arg_count,
        local_names: vec![None; local_count],
        local_spans: vec![None; local_count],
        blocks,
        declared_block: None,
    }
}

fn no_summaries() -> Summaries {
    Summaries::compute(&[])
}

/// MUST flag: `if idx.get() % 2 == 0 { sync_threads() }`.
#[test]
fn canonical_divergent_barrier_is_flagged_with_provenance() {
    // locals: 0 ret, 1 out (param), 2 idx witness, 3 rem, 4 cond
    let f = kernel(
        5,
        1,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(2), 1)),
            },
            Block {
                stmts: vec![stmt(3, &[2]), stmt(4, &[3])],
                term: term(TermKind::Branch {
                    cond: 4,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());

    assert_eq!(a.locals[2], Uniformity::Divergent, "witness");
    assert_eq!(a.locals[4], Uniformity::Divergent, "condition");
    assert_eq!(a.barriers.len(), 1);
    let site = &a.barriers[0];
    assert!(!site.interprocedural);
    let cause = site.divergent_cause.expect("MUST be flagged");
    assert_eq!(cause.block, 1);
    assert_eq!(cause.cond, 4);

    // Provenance: cond ← rem ← witness (a source), in that order.
    let chain = analysis::provenance_chain(&f, &a, cause.cond);
    assert_eq!(chain.len(), 3, "chain: {chain:?}");
    assert!(chain[2].detail.contains("index_1d"), "chain: {chain:?}");
}

/// MUST NOT flag: `if block_idx() > 3 { sync_threads() }`.
#[test]
fn canonical_block_uniform_barrier_is_not_flagged() {
    // locals: 0 ret, 1 block idx, 2 cond
    let f = kernel(
        3,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::BlockUniform, "blockIdx_x", Some(1), 1)),
            },
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert_eq!(a.locals[1], Uniformity::Uniform);
    assert_eq!(a.locals[2], Uniformity::Uniform);
    assert_eq!(a.barriers.len(), 1);
    assert!(
        a.barriers[0].divergent_cause.is_none(),
        "MUST NOT be flagged"
    );
}

/// Post-dominator reconvergence: a barrier after the join point is clean,
/// while a value written inside the branch is control-dependent divergent.
#[test]
fn barrier_after_reconvergence_is_clean() {
    // locals: 0 ret, 1 witness, 2 cond, 3 x
    let f = kernel(
        4,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![stmt(3, &[])], // x = 1 under divergent control
                term: term(TermKind::Goto { target: 3 }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert!(a.block_divergent[2]);
    assert!(!a.block_divergent[3], "join point reconverges");
    assert!(a.barriers[0].divergent_cause.is_none());
    // The phi-equivalent: x differs across lanes after the join.
    assert_eq!(a.locals[3], Uniformity::Divergent);
    assert!(matches!(
        a.reasons[3].as_ref().unwrap().kind,
        ReasonKind::ControlDependent { .. }
    ));
}

/// The classic reduction shape: a uniform loop around a divergent `if`,
/// with the barrier after the `if` but inside the loop — must be clean.
#[test]
fn reduction_loop_barrier_is_clean() {
    // locals: 0 ret, 1 tid witness, 2 s (uniform), 3 loop cond, 4 tid<s, 5 x
    let f = kernel(
        6,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "threadIdx_x",
                    Some(1),
                    1,
                )),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::BlockUniform, "blockDim_x", Some(2), 2)),
            },
            // loop header: while s > 0 (uniform)
            Block {
                stmts: vec![stmt(3, &[2])],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![3, 7],
                    values: vec![],
                }),
            },
            // if tid < s (divergent)
            Block {
                stmts: vec![stmt(4, &[1, 2])],
                term: term(TermKind::Branch {
                    cond: 4,
                    targets: vec![4, 5],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![stmt(5, &[1])],
                term: term(TermKind::Goto { target: 5 }),
            },
            // barrier after the divergent if, still inside the loop
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 6)),
            },
            // s /= 2; back to header
            Block {
                stmts: vec![stmt(2, &[2])],
                term: term(TermKind::Goto { target: 2 }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert_eq!(a.locals[2], Uniformity::Uniform, "s stays uniform");
    assert_eq!(a.locals[4], Uniformity::Divergent, "tid < s is divergent");
    assert!(a.block_divergent[4], "the taken arm is divergent-control");
    assert!(!a.block_divergent[5], "the barrier block reconverged");
    assert_eq!(a.barriers.len(), 1);
    assert!(
        a.barriers[0].divergent_cause.is_none(),
        "the canonical reduction pattern must not be flagged"
    );
}

/// A divergent branch with an aborting arm (`if let` lowers to a switch
/// with an `unreachable` arm; unwrap/panic arms look the same) must still
/// reconverge at the surviving arms' join: a barrier after the join is
/// clean. Distilled from upstream's `barrier_sync_test`.
#[test]
fn aborting_arm_does_not_stretch_the_divergence_region() {
    // locals: 0 ret, 1 witness, 2 cond, 3 x
    let f = kernel(
        4,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            // switch on divergent discriminant: some-arm, none-arm, and an
            // unreachable arm
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 3, 5],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![stmt(3, &[])],
                term: term(TermKind::Goto { target: 3 }),
            },
            // the join, then the barrier
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
            // the aborting arm
            Block {
                stmts: vec![],
                term: term(TermKind::Halt),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert!(a.block_divergent[2], "the surviving arm is in the region");
    assert!(a.block_divergent[5], "the aborting arm is in the region");
    assert!(!a.block_divergent[3], "the join reconverges");
    assert!(
        a.barriers[0].divergent_cause.is_none(),
        "a barrier after the join must be clean despite the aborting arm"
    );
}

/// A barrier inside a loop whose trip count is thread-divergent is flagged.
#[test]
fn barrier_in_divergent_trip_count_loop_is_flagged() {
    // locals: 0 ret, 1 witness, 2 loop cond (divergent)
    let f = kernel(
        3,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            // header: while i < witness
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 4],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Goto { target: 1 }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert!(a.block_divergent[2]);
    assert!(!a.block_divergent[4], "after the loop reconverges");
    assert!(a.barriers[0].divergent_cause.is_some());
}

/// Interprocedural summary bits: calling a helper that may execute a
/// barrier, under divergent control, is a finding at the call site.
#[test]
fn call_to_barrier_helper_under_divergence_is_flagged() {
    let helper = FnModel {
        name: "helper".into(),
        item_path: "test::helper".into(),
        span: 0,
        local_count: 1,
        arg_count: 0,
        local_names: vec![None],
        local_spans: vec![None],
        declared_block: None,
        blocks: vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 1)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    };
    let mut k = kernel(
        3,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Call {
                    callee: Callee {
                        kind: CallKind::Other,
                        display: "helper".into(),
                        local_fn: Some(1),
                    },
                    args: vec![],
                    const_args: vec![],
                    arg_operands: vec![],
                    dest: None,
                    target: Some(3),
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    k.name = "caller".into();

    let fns = vec![k, helper];
    let summaries = Summaries::compute(&fns);
    assert!(summaries.may_contain_barrier[1]);
    assert!(summaries.may_contain_barrier[0], "transitively");

    let a = analysis::analyze(&fns[0], &summaries);
    assert_eq!(a.barriers.len(), 1);
    assert!(a.barriers[0].interprocedural);
    assert!(a.barriers[0].divergent_cause.is_some());
}

/// Warp collectives are collected with their constant masks; only the one
/// under divergent control carries a cause (RC002's subject).
#[test]
fn warp_collectives_and_masks_are_collected() {
    fn masked_call(display: &str, mask: u64, dest: Local, target: usize) -> TermKind {
        TermKind::Call {
            callee: Callee {
                kind: CallKind::WarpCollective,
                display: display.to_string(),
                local_fn: None,
            },
            args: Vec::new(),
            const_args: vec![Some(mask)],
            arg_operands: Vec::new(),
            dest: Some(dest),
            target: Some(target),
        }
    }
    // locals: 0 ret, 1 witness, 2 cond, 3 vote (convergent), 4 vote (divergent)
    let f = kernel(
        5,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            Block {
                stmts: vec![],
                term: term(masked_call("ballot_sync", 0xffff_ffff, 3, 2)),
            },
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![3, 4],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(masked_call("all_sync", 0xffff, 4, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert_eq!(a.warp_ops.len(), 2);
    assert!(
        a.warp_ops[0].divergent_cause.is_none(),
        "the convergent collective is clean"
    );
    assert_eq!(a.warp_ops[0].mask, Some(0xffff_ffff));
    assert!(
        a.warp_ops[1].divergent_cause.is_some(),
        "the guarded collective is flagged"
    );
    assert_eq!(a.warp_ops[1].mask, Some(0xffff));
    // Collective results are at most warp-uniform: divergent in the lattice.
    assert_eq!(a.locals[3], Uniformity::Divergent);
}

/// Calling a helper that may execute a warp collective, under divergent
/// control, is a site at the call (interprocedural summary bits).
#[test]
fn call_to_collective_helper_under_divergence_is_flagged() {
    let helper = FnModel {
        name: "collective_helper".into(),
        item_path: "test::collective_helper".into(),
        span: 0,
        local_count: 1,
        arg_count: 0,
        local_names: vec![None],
        local_spans: vec![None],
        declared_block: None,
        blocks: vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::WarpCollective, "all_sync", Some(0), 1)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    };
    let caller = kernel(
        3,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "index_1d", Some(1), 1)),
            },
            Block {
                stmts: vec![stmt(2, &[1])],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Call {
                    callee: Callee {
                        kind: CallKind::Other,
                        display: "collective_helper".into(),
                        local_fn: Some(1),
                    },
                    args: vec![],
                    const_args: vec![],
                    arg_operands: vec![],
                    dest: None,
                    target: Some(3),
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let fns = vec![caller, helper];
    let summaries = Summaries::compute(&fns);
    assert!(summaries.may_contain_warp_op[1]);
    assert!(summaries.may_contain_warp_op[0], "transitively");

    let a = analysis::analyze(&fns[0], &summaries);
    assert_eq!(a.warp_ops.len(), 1);
    assert!(a.warp_ops[0].interprocedural);
    assert!(a.warp_ops[0].divergent_cause.is_some());
    assert_eq!(a.warp_ops[0].mask, None, "no mask across a call boundary");
}

/// Irreducible CFGs degrade to all-divergent, and say so.
#[test]
fn irreducible_cfg_degrades_to_all_divergent() {
    // Two entries into a cycle: 0 → {1, 2}, 1 ⇄ 2, 1 → barrier → return.
    let f = kernel(
        2,
        0,
        vec![
            Block {
                stmts: vec![stmt(1, &[])],
                term: term(TermKind::Branch {
                    cond: 1,
                    targets: vec![1, 2],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Branch {
                    cond: 1,
                    targets: vec![2, 3],
                    values: vec![],
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Goto { target: 1 }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert!(a.irreducible);
    assert_eq!(a.locals[1], Uniformity::Divergent);
    assert!(
        a.barriers[0].divergent_cause.is_some() || a.block_divergent[3],
        "degraded mode treats the barrier as divergent-control"
    );
}

/// Coverage honesty: opaque terminators are counted, and their results are
/// conservatively divergent.
#[test]
fn opaque_code_is_counted_and_conservative() {
    let f = kernel(
        2,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(TermKind::Opaque {
                    uses: vec![],
                    dest: Some(1),
                    target: Some(1),
                }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let a = analysis::analyze(&f, &no_summaries());
    assert_eq!(a.opaque_statements, 1);
    assert_eq!(a.analyzed_statements, 1); // the return terminator
    assert_eq!(a.locals[1], Uniformity::Divergent);
}
