//! Witness-interpreter acceptance tests on hand-built models — the
//! gate's shapes: replay a concrete hang for an injected RC001 and RC002.

use reconverge_artifacts::witness::VerdictKind;
use reconverge_core::dialect::CallKind;
use reconverge_core::model::{
    BinOp, Block, Callee, Eval, FnModel, Local, Operand, Stmt, Term, TermKind,
};
use reconverge_witness::{SiteKind, replay_hang};

fn term(kind: TermKind) -> Term {
    Term { kind, span: 0 }
}

fn stmt_eval(dest: Local, uses: &[Local], eval: Eval) -> Stmt {
    Stmt {
        dest: Some(dest),
        uses: uses.to_vec(),
        eval: Some(eval),
        opaque: false,
        span: 0,
    }
}

fn call(
    kind: CallKind,
    display: &str,
    arg_operands: Vec<Option<Operand>>,
    dest: Option<Local>,
    target: usize,
) -> TermKind {
    TermKind::Call {
        callee: Callee {
            kind,
            display: display.to_string(),
            local_fn: None,
        },
        args: Vec::new(),
        const_args: Vec::new(),
        arg_operands,
        dest,
        target: Some(target),
    }
}

fn kernel(local_count: usize, blocks: Vec<Block>) -> FnModel {
    FnModel {
        name: "k".into(),
        item_path: "test::k".into(),
        span: 0,
        local_count,
        arg_count: 1,
        local_names: vec![None; local_count],
        local_spans: vec![None; local_count],
        blocks,
    }
}

/// The canonical `if idx.get() % 2 == 0 { <site> }` shape, mirroring the
/// real MIR: witness mint, a reference, a WitnessRead, arithmetic, branch.
///
/// locals: 0 ret, 1 param, 2 witness, 3 ref, 4 got, 5 rem, 6 cond
fn canonical(site: TermKind) -> FnModel {
    kernel(
        7,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "index_1d",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![stmt_eval(3, &[2], Eval::Use(Operand::Local(2)))],
                term: term(call(
                    CallKind::WitnessRead,
                    "ThreadIndex::get",
                    vec![Some(Operand::Local(3))],
                    Some(4),
                    2,
                )),
            },
            Block {
                stmts: vec![
                    stmt_eval(
                        5,
                        &[4],
                        Eval::Binary(BinOp::Rem, Operand::Local(4), Operand::Const(2)),
                    ),
                    stmt_eval(
                        6,
                        &[5],
                        Eval::Binary(BinOp::Eq, Operand::Local(5), Operand::Const(0)),
                    ),
                ],
                // switchInt(cond): 0 → skip, otherwise → the site block.
                term: term(TermKind::Branch {
                    cond: 6,
                    targets: vec![4, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(site),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

const EVEN_LANES: u32 = 0x5555_5555;
const ODD_LANES: u32 = 0xaaaa_aaaa;

/// M4 gate, RC001 half: the canonical divergent barrier replays as a hang.
#[test]
fn rc001_hang_replays_concretely() {
    let f = canonical(call(CallKind::Barrier, "sync_threads", vec![], None, 4));
    let replay = replay_hang(&f, 3, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, EVEN_LANES, "even lanes arrive");
    assert_eq!(replay.never_arrives, ODD_LANES, "odd lanes never do");
    assert_eq!(replay.verdict_kind, VerdictKind::UndefinedBehavior);
    assert!(replay.verdict_message.contains("16 of 32 lanes"));
    assert!(
        replay.verdict_message.contains("usually"),
        "calibrated wording, never 'always': {}",
        replay.verdict_message
    );
    assert_eq!(replay.block, [32, 1, 1]);
    let barrier_step = replay
        .steps
        .iter()
        .find(|s| s.barrier.is_some())
        .expect("a barrier step");
    assert_eq!(barrier_step.barrier, Some((16, 32)));
    assert_eq!(barrier_step.lane_changes.len(), 16);
}

/// M4 gate, RC002 half: a full-mask collective reached by half the warp.
#[test]
fn rc002_mask_mismatch_replays_concretely() {
    let f = canonical(call(
        CallKind::WarpCollective,
        "ballot_sync",
        vec![Some(Operand::Const(0xffff_ffff)), Some(Operand::Const(1))],
        Some(0),
        4,
    ));
    let replay = replay_hang(
        &f,
        3,
        SiteKind::Collective {
            mask: Some(0xffff_ffff),
        },
        0,
    )
    .expect("must produce a witness");
    assert_eq!(replay.arrived, EVEN_LANES);
    let op_step = replay
        .steps
        .iter()
        .find(|s| s.warp_op.is_some())
        .expect("a collective step");
    assert_eq!(
        op_step.warp_op,
        Some(("ballot_sync".to_string(), 0xffff_ffff, EVEN_LANES))
    );
    assert!(replay.verdict_message.contains("16 lane(s)"));
}

/// Mask refinement: a constant mask naming exactly the arriving lanes is
/// the guarded partial-warp idiom — no witness.
#[test]
fn matching_partial_mask_produces_no_witness() {
    let f = canonical(call(
        CallKind::WarpCollective,
        "ballot_sync",
        vec![Some(Operand::Const(u64::from(EVEN_LANES) as u128))],
        Some(0),
        4,
    ));
    assert!(
        replay_hang(
            &f,
            3,
            SiteKind::Collective {
                mask: Some(u64::from(EVEN_LANES)),
            },
            0,
        )
        .is_none()
    );
}

/// A branch on an unknown value (a parameter) bails: no witness, the
/// static result stands.
#[test]
fn unknown_branch_produces_no_witness() {
    // locals: 0 ret, 1 param, 2 cond (unmodeled)
    let f = kernel(
        3,
        vec![
            Block {
                stmts: vec![Stmt {
                    dest: Some(2),
                    uses: vec![1],
                    eval: None, // e.g. derived through memory
                    opaque: false,
                    span: 0,
                }],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![2, 1],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 2)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(replay_hang(&f, 1, SiteKind::Barrier, 0).is_none());
}

/// An unknown branch whose arms rejoin before the site — the everyday
/// short-circuit `&&` / `if let` diamond — cannot change who arrives: the
/// diamond is skipped with its effects erased, and the canonical divergent
/// barrier after it still replays concretely.
#[test]
fn unknowable_diamond_before_the_site_is_skipped() {
    // locals: 0 ret, 1 param, 2 unknown cond, 3 k (arm-assigned), 4 witness,
    // 5 got, 6 rem, 7 cond
    let unknown = Stmt {
        dest: Some(2),
        uses: vec![1],
        eval: None, // e.g. loaded through memory from a parameter
        opaque: false,
        span: 0,
    };
    let f = kernel(
        8,
        vec![
            Block {
                stmts: vec![unknown],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![1, 2],
                    values: vec![Some(0), None],
                }),
            },
            // The arms assign different constants to `k`; taking either arm
            // for real would fabricate a value, so the skip must erase it.
            Block {
                stmts: vec![stmt_eval(3, &[], Eval::Use(Operand::Const(1)))],
                term: term(TermKind::Goto { target: 3 }),
            },
            Block {
                stmts: vec![stmt_eval(3, &[], Eval::Use(Operand::Const(2)))],
                term: term(TermKind::Goto { target: 3 }),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "index_1d",
                    vec![],
                    Some(4),
                    4,
                )),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::WitnessRead,
                    "ThreadIndex::get",
                    vec![Some(Operand::Local(4))],
                    Some(5),
                    5,
                )),
            },
            Block {
                stmts: vec![
                    stmt_eval(
                        6,
                        &[5],
                        Eval::Binary(BinOp::Rem, Operand::Local(5), Operand::Const(2)),
                    ),
                    stmt_eval(
                        7,
                        &[6],
                        Eval::Binary(BinOp::Eq, Operand::Local(6), Operand::Const(0)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 7,
                    targets: vec![7, 6],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 7)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 6, SiteKind::Barrier, 0).expect("diamond must not spoil this");
    assert_eq!(replay.arrived, 0x5555_5555, "even lanes arrive");
    assert_eq!(replay.never_arrives, 0xAAAA_AAAA, "odd lanes never do");
}

/// A diamond that hides a synchronization point must NOT be skipped — its
/// release choreography has to be simulated, so the replay honestly bails.
#[test]
fn diamond_hiding_a_sync_point_is_not_skipped() {
    // locals: 0 ret, 1 param, 2 unknown cond, 3 witness, 4 got, 5 rem, 6 cond
    let unknown = Stmt {
        dest: Some(2),
        uses: vec![1],
        eval: None,
        opaque: false,
        span: 0,
    };
    let f = kernel(
        7,
        vec![
            Block {
                stmts: vec![unknown],
                term: term(TermKind::Branch {
                    cond: 2,
                    targets: vec![1, 2],
                    values: vec![Some(0), None],
                }),
            },
            // One arm synchronizes; skipping it would erase a barrier.
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 2)),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "index_1d",
                    vec![],
                    Some(3),
                    3,
                )),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::WitnessRead,
                    "ThreadIndex::get",
                    vec![Some(Operand::Local(3))],
                    Some(4),
                    4,
                )),
            },
            Block {
                stmts: vec![
                    stmt_eval(
                        5,
                        &[4],
                        Eval::Binary(BinOp::Rem, Operand::Local(4), Operand::Const(2)),
                    ),
                    stmt_eval(
                        6,
                        &[5],
                        Eval::Binary(BinOp::Eq, Operand::Local(5), Operand::Const(0)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 6,
                    targets: vec![6, 5],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 6)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(replay_hang(&f, 5, SiteKind::Barrier, 0).is_none());
}

/// A convergent barrier before the site releases all lanes together and
/// the replay still finds the hang at the real site.
#[test]
fn intervening_convergent_barrier_is_released() {
    // locals: 0 ret, 1 param, 2 witness, 3 ref, 4 got, 5 rem, 6 cond
    let mut f = canonical(call(CallKind::Barrier, "sync_threads", vec![], None, 4));
    // Prepend a convergent barrier: entry now goes through it.
    f.blocks.insert(
        0,
        Block {
            stmts: vec![],
            term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 1)),
        },
    );
    // Shift every target by one.
    for block in &mut f.blocks[1..] {
        match &mut block.term.kind {
            TermKind::Goto { target } => *target += 1,
            TermKind::Branch { targets, .. } | TermKind::Jump { targets } => {
                for t in targets {
                    *t += 1;
                }
            }
            TermKind::Call {
                target: Some(t), ..
            } => *t += 1,
            _ => {}
        }
    }
    let replay = replay_hang(&f, 4, SiteKind::Barrier, 0).expect("released and replayed");
    assert_eq!(replay.arrived, EVEN_LANES);
}

/// Uniform control flow (every lane arrives) is nothing to witness.
#[test]
fn uniform_arrival_produces_no_witness() {
    // locals: 0 ret, 1 param
    let f = kernel(
        2,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 1)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(replay_hang(&f, 0, SiteKind::Barrier, 0).is_none());
}
