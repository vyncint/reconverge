//! Witness-interpreter acceptance tests on hand-built models — the
//! gate's shapes: replay a concrete hang for an injected RC001 and RC002.

use reconverge_artifacts::witness::VerdictKind;
use reconverge_core::dialect::CallKind;
use reconverge_core::model::{
    BinOp, Block, Callee, Eval, FnModel, Local, Operand, Stmt, Term, TermKind, UnOp,
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
        declared_block: None,
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

const EVEN_LANES: u128 = 0x5555_5555;
const ODD_LANES: u128 = 0xaaaa_aaaa;

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
        Some(("ballot_sync".to_string(), 0xffff_ffff, EVEN_LANES as u32))
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
        vec![Some(Operand::Const(EVEN_LANES))],
        Some(0),
        4,
    ));
    assert!(
        replay_hang(
            &f,
            3,
            SiteKind::Collective {
                mask: Some(EVEN_LANES as u64),
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

/// Issue #9: lanes split between the site and an upstream barrier are a
/// mutual deadlock, not an abort — the parked lanes never arrive, and the
/// site's own divergence is witnessed. Shape: even lanes park at a first
/// barrier, odd lanes reach the site behind the complementary guard.
///
/// locals: 0 ret, 1 param, 2 witness, 3 ref, 4 got, 5 rem, 6 even, 7 odd
#[test]
fn deadlock_behind_an_upstream_barrier_still_witnesses_the_site() {
    let f = kernel(
        8,
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
                // evens → the upstream barrier, odds → onward.
                term: term(TermKind::Branch {
                    cond: 6,
                    targets: vec![4, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 4)),
            },
            Block {
                stmts: vec![stmt_eval(
                    7,
                    &[5],
                    Eval::Binary(BinOp::Eq, Operand::Local(5), Operand::Const(1)),
                )],
                // odds → the site, evens (if ever released) → exit.
                term: term(TermKind::Branch {
                    cond: 7,
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
    let replay = replay_hang(&f, 5, SiteKind::Barrier, 0)
        .expect("the deadlocked shape must witness the site");
    assert_eq!(replay.arrived, ODD_LANES, "odd lanes reach the site");
    assert_eq!(
        replay.never_arrives, EVEN_LANES,
        "even lanes are parked forever at the upstream barrier"
    );
}

/// Issue #9: `warp_id` (and `live_lanes_1d`) are exact under the replay's
/// one-warp launch, so a warp-uniform-guarded barrier upstream no longer
/// takes a downstream finding out of the gate.
///
/// locals: 0 ret, 1 param, 2 witness, 3 ref, 4 got, 5 rem, 6 even,
/// 7 warp_id, 8 wcond
#[test]
fn warp_uniform_guard_upstream_does_not_ungate_the_site() {
    let f = kernel(
        9,
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
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp_id",
                    vec![],
                    Some(7),
                    3,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    8,
                    &[7],
                    Eval::Binary(BinOp::Eq, Operand::Local(7), Operand::Const(0)),
                )],
                // warp 0 (everyone, under one warp) → the upstream barrier.
                term: term(TermKind::Branch {
                    cond: 8,
                    targets: vec![5, 4],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 5)),
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
    let replay = replay_hang(&f, 6, SiteKind::Barrier, 0)
        .expect("the uniform upstream barrier releases and the site is witnessed");
    assert_eq!(replay.arrived, EVEN_LANES);
    assert_eq!(replay.never_arrives, ODD_LANES);
}

/// Issue #24: the positional lane masks are closed forms of the lane's
/// own ordinal, so a guard on one replays. `lanemask_eq & 1` is 1 for
/// lane 0 alone — a 1-of-32 split at the barrier.
///
/// This replaces a test asserting the opposite. The masks were withheld
/// while `!` was boolean-only and casts were the identity (#22); with
/// evaluation width-typed, withholding them only costs recall.
#[test]
fn a_positional_lanemask_guard_is_witnessed() {
    let f = lanemask_guard("lanemask_eq", BinOp::BitAnd, 1);
    let replay = replay_hang(&f, 2, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, 0x1_u128, "lane 0 alone reaches it");
    assert_eq!(replay.never_arrives, 0xffff_fffe_u128);
}

/// Issue #24: `active_mask` is deliberately *not* in that group. Its value
/// is the set of lanes still live, which changes as lanes diverge — a
/// path-dependent question rather than a positional one — so it stays
/// unknown and the site is not witnessed.
#[test]
fn active_mask_stays_unevaluable() {
    let f = lanemask_guard("active_mask", BinOp::BitAnd, 1);
    assert!(
        replay_hang(&f, 2, SiteKind::Barrier, 0).is_none(),
        "a path-dependent mask must not be given a value"
    );
}

/// Issues #22, #23 and #24 together — the reproduction #23 was filed with:
///
/// ```ignore
/// if warp::lanemask_lt().count_ones() > 4 { thread::sync_threads(); }
/// ```
///
/// `lanemask_lt` is every lane below this one, so its population count is
/// the lane's ordinal. Lanes 5..=31 take the branch and 0..=4 do not, so
/// 27 lanes wait at a barrier 5 never reach. This needed all three: the
/// mask's value, an exact popcount of it, and arithmetic that evaluates at
/// 32 bits.
#[test]
fn the_lane_ordinal_idiom_replays() {
    let f = kernel(
        5,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp::lanemask_lt",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::CountOnes { bits: 32 },
                    "count_ones",
                    vec![Some(Operand::Local(2))],
                    Some(3),
                    2,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    4,
                    &[3],
                    Eval::Binary(BinOp::Gt, Operand::Local(3), Operand::Const(4)),
                )],
                term: term(TermKind::Branch {
                    cond: 4,
                    targets: vec![4, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 3, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, 0xffff_ffe0_u128, "lanes 5..=31");
    assert_eq!(replay.never_arrives, 0x1f_u128, "lanes 0..=4");
}

/// `if <register> <op> <k> { sync_threads(); }` — the shape both lanemask
/// tests above use.
fn lanemask_guard(register: &str, op: BinOp, k: u128) -> FnModel {
    kernel(
        4,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    register,
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[2],
                    Eval::Binary(op, Operand::Local(2), Operand::Const(k)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

/// Issue #10: a divergent guard *inside* a loop is promoted. The loop
/// counter increments through overflow-checked arithmetic (what debug
/// builds lower `n += 1` to), which the interpreter now evaluates.
/// Shape of `while n < i%4 { if i%2 == 0 { site } n += 1 }`.
///
/// locals: 0 ret, 1 param, 2 witness, 3 ref, 4 got, 5 bound, 6 n,
/// 7 loop-cond, 8 rem2, 9 even
#[test]
fn guard_inside_a_loop_is_witnessed() {
    let f = kernel(
        10,
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
                        Eval::Binary(BinOp::Rem, Operand::Local(4), Operand::Const(4)),
                    ),
                    stmt_eval(6, &[], Eval::Use(Operand::Const(0))),
                ],
                term: term(TermKind::Goto { target: 3 }),
            },
            // loop header: n < bound?
            Block {
                stmts: vec![stmt_eval(
                    7,
                    &[6, 5],
                    Eval::Binary(BinOp::Lt, Operand::Local(6), Operand::Local(5)),
                )],
                term: term(TermKind::Branch {
                    cond: 7,
                    targets: vec![7, 4],
                    values: vec![Some(0), None],
                }),
            },
            // body: even lanes reach the site
            Block {
                stmts: vec![
                    stmt_eval(
                        8,
                        &[4],
                        Eval::Binary(BinOp::Rem, Operand::Local(4), Operand::Const(2)),
                    ),
                    stmt_eval(
                        9,
                        &[8],
                        Eval::Binary(BinOp::Eq, Operand::Local(8), Operand::Const(0)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 9,
                    targets: vec![6, 5],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 6)),
            },
            // n += 1, back to the header
            Block {
                stmts: vec![stmt_eval(
                    6,
                    &[6],
                    Eval::CheckedBinary(BinOp::Add, Operand::Local(6), Operand::Const(1), 32),
                )],
                term: term(TermKind::Goto { target: 3 }),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 5, SiteKind::Barrier, 0)
        .expect("the guard inside the loop must be witnessed");
    // Lanes with i%4 > 0 enter the loop; the even ones among them
    // (i % 4 == 2) reach the barrier on their first iteration.
    assert_eq!(replay.arrived, 0x4444_4444_u128);
    assert_eq!(replay.never_arrives, 0xbbbb_bbbb_u128);
}

/// The checked form panics the thread on overflow: past the width the
/// value does not exist, so the interpreter yields unknown — never a
/// wrapped value a real thread would not see.
///
/// locals: 0 ret, 1 param, 2 underflowed, 3 cond
#[test]
fn checked_arithmetic_never_fabricates_a_wrapped_value() {
    let f = kernel(
        4,
        vec![
            Block {
                stmts: vec![
                    stmt_eval(
                        2,
                        &[],
                        Eval::CheckedBinary(BinOp::Sub, Operand::Const(0), Operand::Const(1), 32),
                    ),
                    stmt_eval(
                        3,
                        &[2],
                        Eval::Binary(BinOp::Gt, Operand::Local(2), Operand::Const(5)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 3,
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
    assert!(
        replay_hang(&f, 1, SiteKind::Barrier, 0).is_none(),
        "an underflow must abort the replay, not wrap"
    );
}

/// Issue #14: whole-warp divergence exists only beyond one warp. A
/// `warp_id()`-guarded barrier replayed at the declared block of 64
/// threads has a divergent pair — warp 0 arrives, warp 1 never does —
/// while the same model at 32 lanes is uniform and correctly witnesses
/// nothing.
///
/// locals: 0 ret, 1 param, 2 warp_id, 3 cond
#[test]
fn whole_warp_divergence_is_witnessed_at_the_declared_block() {
    let f = kernel(
        4,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp_id",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[2],
                    Eval::Binary(BinOp::Eq, Operand::Local(2), Operand::Const(0)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    use reconverge_witness::replay_hang_at;
    assert!(
        replay_hang(&f, 2, SiteKind::Barrier, 0).is_none(),
        "at one warp the guard is uniform — nothing to witness"
    );
    let replay = replay_hang_at(&f, 2, SiteKind::Barrier, 0, 64).expect("two warps diverge");
    assert_eq!(replay.arrived, 0xffff_ffff, "warp 0 arrives");
    assert_eq!(
        replay.never_arrives, 0xffff_ffff_0000_0000,
        "warp 1 never does"
    );
    assert_eq!(replay.block, [64, 1, 1]);
    assert!(replay.verdict_message.contains("32 of 64 lanes"));
}

/// A collective anywhere on a lane's path aborts a multi-warp replay:
/// its synchronization is per warp, which the replay does not model.
#[test]
fn multi_warp_replay_bails_on_any_collective() {
    let f = kernel(
        4,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::WarpCollective,
                    "ballot_sync",
                    vec![Some(Operand::Const(0xffff_ffff))],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp_id",
                    vec![],
                    Some(3),
                    2,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[3],
                    Eval::Binary(BinOp::Eq, Operand::Local(3), Operand::Const(0)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![4, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(reconverge_witness::replay_hang_at(&f, 3, SiteKind::Barrier, 0, 64).is_none());
}

/// The thread-index witnesses evaluate per name: `threadIdx_y` is 0 under
/// the replay's one-dimensional block, never the lane id. A barrier
/// guarded on it is uniform — treating the guard as per-lane would have
/// fabricated a confirmation of a correct kernel.
///
/// locals: 0 ret, 1 param, 2 y, 3 cond
#[test]
fn thread_idx_y_is_zero_not_the_lane_id() {
    let f = kernel(
        4,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "threadIdx_y",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[2],
                    Eval::Binary(BinOp::Eq, Operand::Local(2), Operand::Const(0)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(
        replay_hang(&f, 2, SiteKind::Barrier, 0).is_none(),
        "every lane has y = 0, every lane arrives — nothing to witness, \
         and definitely not a confirmation"
    );
}

/// `lane_id` is the in-warp lane, not the thread index: at 64 threads a
/// `lane_id() % 2` guard splits *every* warp the same way.
///
/// locals: 0 ret, 1 param, 2 lane, 3 rem, 4 cond
#[test]
fn lane_id_wraps_per_warp_in_a_multi_warp_replay() {
    let f = kernel(
        5,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "lane_id",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![
                    stmt_eval(
                        3,
                        &[2],
                        Eval::Binary(BinOp::Rem, Operand::Local(2), Operand::Const(2)),
                    ),
                    stmt_eval(
                        4,
                        &[3],
                        Eval::Binary(BinOp::Eq, Operand::Local(3), Operand::Const(0)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 4,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = reconverge_witness::replay_hang_at(&f, 2, SiteKind::Barrier, 0, 64)
        .expect("even lanes of both warps diverge");
    assert_eq!(replay.arrived, 0x5555_5555_5555_5555_u128);
    assert_eq!(replay.never_arrives, 0xaaaa_aaaa_aaaa_aaaa_u128);
}

/// Helper to construct a kernel model with a `warp_id` comparison guard:
/// `if warp_id OP const_val { sync_threads(); }`
fn warp_id_guard_model(op: BinOp, const_val: u128) -> FnModel {
    kernel(
        4,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp_id",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[2],
                    Eval::Binary(op, Operand::Local(2), Operand::Const(const_val)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

/// Helper to construct a kernel model with a `warp_id % modulus == target` guard.
fn warp_id_rem_eq_model(modulus: u128, target: u128) -> FnModel {
    kernel(
        5,
        vec![
            Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "warp_id",
                    vec![],
                    Some(2),
                    1,
                )),
            },
            Block {
                stmts: vec![
                    stmt_eval(
                        3,
                        &[2],
                        Eval::Binary(BinOp::Rem, Operand::Local(2), Operand::Const(modulus)),
                    ),
                    stmt_eval(
                        4,
                        &[3],
                        Eval::Binary(BinOp::Eq, Operand::Local(3), Operand::Const(target)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 4,
                    targets: vec![3, 2],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

/// Differential launch matrix test for multi-warp replay across block sizes 32, 64, and 128.
/// Records evidence that `warp_id`-guarded barriers that are uniform at 1 warp (block 32)
/// correctly promote to confirmed findings at multi-warp declared blocks (64 and 128)
/// when the warp configuration causes divergence.
#[test]
fn multi_warp_replay_differential_launch_matrix_warp_id() {
    use reconverge_witness::replay_hang_at;

    // 1. `warp_id == 0`:
    let eq_zero = warp_id_guard_model(BinOp::Eq, 0);
    // Block 32 (1 warp): warp 0 arrives -> uniform -> no witness.
    assert!(replay_hang_at(&eq_zero, 2, SiteKind::Barrier, 0, 32).is_none());
    // Block 64 (2 warps): warp 0 arrives (32 lanes), warp 1 never does (32 lanes) -> hang witnessed.
    let replay_64 = replay_hang_at(&eq_zero, 2, SiteKind::Barrier, 0, 64)
        .expect("warp_id == 0 must witness divergence at 64 threads");
    assert_eq!(replay_64.arrived, 0x0000_0000_ffff_ffff_u128);
    assert_eq!(replay_64.never_arrives, 0xffff_ffff_0000_0000_u128);
    assert_eq!(replay_64.block, [64, 1, 1]);
    assert!(replay_64.verdict_message.contains("32 of 64 lanes"));

    // Block 128 (4 warps): warp 0 arrives (32 lanes), warps 1..3 never do (96 lanes) -> hang witnessed.
    let replay_128 = replay_hang_at(&eq_zero, 2, SiteKind::Barrier, 0, 128)
        .expect("warp_id == 0 must witness divergence at 128 threads");
    assert_eq!(replay_128.arrived, 0xffff_ffff_u128);
    assert_eq!(
        replay_128.never_arrives,
        0xffff_ffff_ffff_ffff_ffff_ffff_0000_0000_u128
    );
    assert_eq!(replay_128.block, [128, 1, 1]);
    assert!(replay_128.verdict_message.contains("32 of 128 lanes"));

    // 2. `warp_id < 2`:
    let lt_two = warp_id_guard_model(BinOp::Lt, 2);
    // Block 32 (1 warp): warp 0 < 2 -> true for all 32 lanes -> uniform -> no witness.
    assert!(replay_hang_at(&lt_two, 2, SiteKind::Barrier, 0, 32).is_none());
    // Block 64 (2 warps): warps 0, 1 < 2 -> true for all 64 lanes -> uniform -> no witness (SAFE at block 64).
    assert!(replay_hang_at(&lt_two, 2, SiteKind::Barrier, 0, 64).is_none());
    // Block 128 (4 warps): warps 0, 1 arrive (64 lanes), warps 2, 3 never do (64 lanes) -> hang witnessed.
    let replay_lt2_128 = replay_hang_at(&lt_two, 2, SiteKind::Barrier, 0, 128)
        .expect("warp_id < 2 must witness divergence at 128 threads");
    assert_eq!(replay_lt2_128.arrived, 0xffff_ffff_ffff_ffff_u128);
    assert_eq!(
        replay_lt2_128.never_arrives,
        0xffff_ffff_ffff_ffff_0000_0000_0000_0000_u128
    );
    assert_eq!(replay_lt2_128.block, [128, 1, 1]);
    assert!(replay_lt2_128.verdict_message.contains("64 of 128 lanes"));

    // 3. `warp_id % 2 == 0`:
    let rem_even = warp_id_rem_eq_model(2, 0);
    // Block 32 (1 warp): warp 0 % 2 == 0 -> uniform -> no witness.
    assert!(replay_hang_at(&rem_even, 2, SiteKind::Barrier, 0, 32).is_none());
    // Block 64 (2 warps): warp 0 arrives (32 lanes), warp 1 never does (32 lanes) -> hang witnessed.
    let replay_rem_64 = replay_hang_at(&rem_even, 2, SiteKind::Barrier, 0, 64)
        .expect("warp_id % 2 == 0 must witness divergence at 64 threads");
    assert_eq!(replay_rem_64.arrived, 0x0000_0000_ffff_ffff_u128);
    assert_eq!(replay_rem_64.never_arrives, 0xffff_ffff_0000_0000_u128);
    // Block 128 (4 warps): warps 0, 2 arrive (64 lanes), warps 1, 3 never do (64 lanes) -> hang witnessed.
    let replay_rem_128 = replay_hang_at(&rem_even, 2, SiteKind::Barrier, 0, 128)
        .expect("warp_id % 2 == 0 must witness divergence at 128 threads");
    assert_eq!(
        replay_rem_128.arrived,
        0x0000_0000_ffff_ffff_0000_0000_ffff_ffff_u128
    );
    assert_eq!(
        replay_rem_128.never_arrives,
        0xffff_ffff_0000_0000_ffff_ffff_0000_0000_u128
    );
    assert_eq!(replay_rem_128.block, [128, 1, 1]);
    assert!(replay_rem_128.verdict_message.contains("64 of 128 lanes"));
}

/// Differential check verifying that safe launches (where every warp in the declared
/// block shape evaluates the guard uniformly) never produce a witness / finding.
#[test]
fn multi_warp_replay_safe_launches_produce_no_findings() {
    use reconverge_witness::replay_hang_at;

    // `warp_id < 4` is uniform for block 32 (1 warp), block 64 (2 warps), and block 128 (4 warps).
    let lt_four = warp_id_guard_model(BinOp::Lt, 4);
    assert!(
        replay_hang_at(&lt_four, 2, SiteKind::Barrier, 0, 32).is_none(),
        "block 32 safe launch must not produce a witness"
    );
    assert!(
        replay_hang_at(&lt_four, 2, SiteKind::Barrier, 0, 64).is_none(),
        "block 64 safe launch must not produce a witness"
    );
    assert!(
        replay_hang_at(&lt_four, 2, SiteKind::Barrier, 0, 128).is_none(),
        "block 128 safe launch must not produce a witness"
    );

    // `warp_id < 128` is uniform across block sizes 32, 64, and 128.
    let lt_large = warp_id_guard_model(BinOp::Lt, 128);
    for &threads in &[32, 64, 128] {
        assert!(
            replay_hang_at(&lt_large, 2, SiteKind::Barrier, 0, threads).is_none(),
            "block {threads} uniform warp_id guard must produce no finding"
        );
    }
}

/// Issue #22: `!x` is the complement of `x`'s own type. `(!lane) & 1` is 1
/// on the even lanes, because `!lane` is odd exactly when `lane` is even —
/// so the barrier is reached by 16 lanes and skipped by 16.
///
/// Evaluated as a *boolean* negation, `!lane` would be 1 for lane 0 alone
/// and 0 everywhere else, making this look like a 1-of-32 split. The
/// masks below are what tells the two apart.
#[test]
fn bitwise_not_evaluates_at_the_operand_width() {
    let f = kernel(
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
                    stmt_eval(5, &[4], Eval::Unary(UnOp::Not, Operand::Local(4), 32)),
                    stmt_eval(
                        6,
                        &[5],
                        Eval::Binary(BinOp::BitAnd, Operand::Local(5), Operand::Const(1)),
                    ),
                ],
                term: term(TermKind::Branch {
                    cond: 6,
                    targets: vec![4, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 3, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, 0x5555_5555_u128, "the even lanes reach it");
    assert_eq!(replay.never_arrives, 0xaaaa_aaaa_u128);
}

/// Issue #22: a narrowing cast truncates. `(lane * 16) as u8` is zero for
/// lane 0 and lane 16, because 256 is discarded by the cast — two lanes
/// reach the barrier, thirty do not.
///
/// Treated as the identity, only lane 0 would be zero. Widening is the
/// identity on the store's zero-extended embedding, which is why the old
/// behaviour looked correct on thread-index values and fails here.
#[test]
fn a_narrowing_cast_truncates_rather_than_passing_the_value_through() {
    let f = kernel(
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
                        Eval::Binary(BinOp::Mul, Operand::Local(4), Operand::Const(16)),
                    ),
                    stmt_eval(6, &[5], Eval::Cast(Operand::Local(5), 8)),
                ],
                term: term(TermKind::Branch {
                    cond: 6,
                    targets: vec![3, 4],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 4)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 3, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, 0x0001_0001_u128, "lane 0 and lane 16");
    assert_eq!(replay.never_arrives, 0xfffe_fffe_u128);
}

/// Issue #23: `count_ones` on an operand that fits its declared width
/// evaluates to the population count, so the branch on it replays.
#[test]
fn count_ones_evaluates_popcount_within_its_width() {
    let f = kernel(
        6,
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
                stmts: vec![],
                term: term(call(
                    CallKind::CountOnes { bits: 32 },
                    "count_ones",
                    vec![Some(Operand::Local(4))],
                    Some(5),
                    3,
                )),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Branch {
                    cond: 5,
                    targets: vec![5, 4],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 5)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let replay = replay_hang(&f, 4, SiteKind::Barrier, 0).expect("must produce a witness");
    assert_eq!(replay.arrived, 0xffff_fffe_u128);
    assert_eq!(replay.never_arrives, 0x1_u128);
}

/// A popcount is meaningless without the operand's width, and the store is
/// an untyped `u128` whose unchecked arithmetic wraps at 128 bits rather
/// than at 32. `0u32.wrapping_sub(lane).count_ones()` is 0 for lane 0 and
/// 32 for the rest, so `> 40` is false for every lane and the barrier is
/// never reached — there is no hang to witness.
///
/// Counting the store's 128 bits instead would make the guard true for 31
/// lanes and mint a concrete witness for a kernel that cannot hang. The
/// interpreter must decline: an operand carrying bits its type cannot hold
/// is a value the program never had.
#[test]
fn count_ones_declines_an_operand_wider_than_its_type() {
    let f = kernel(
        8,
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
                // `0u32.wrapping_sub(lane)` — MIR `SubUnchecked`, which
                // carries no width, so the store wraps at 128 bits.
                stmts: vec![stmt_eval(
                    5,
                    &[4],
                    Eval::Binary(BinOp::Sub, Operand::Const(0), Operand::Local(4)),
                )],
                term: term(call(
                    CallKind::CountOnes { bits: 32 },
                    "count_ones",
                    vec![Some(Operand::Local(5))],
                    Some(6),
                    3,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    7,
                    &[6],
                    Eval::Binary(BinOp::Gt, Operand::Local(6), Operand::Const(40)),
                )],
                term: term(TermKind::Branch {
                    cond: 7,
                    targets: vec![5, 4],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 5)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(
        replay_hang(&f, 4, SiteKind::Barrier, 0).is_none(),
        "a popcount of an out-of-range operand must not mint a witness"
    );
}

/// #22 and #23 together: `(!lane).count_ones()` is 27..=32 for every lane,
/// so `> 0` holds for all 32 and the barrier is uniform — there is nothing
/// to witness.
///
/// This is the composition that made the ordering matter. A popcount is
/// exact only if its operand is, and with `!` evaluated as boolean
/// negation the operand was 1 for lane 0 and 0 elsewhere — narrow enough
/// to pass the width check in `popcount_within`, and wrong. The guard
/// against a *wide* operand cannot catch one that is the right width and
/// simply wrong; only evaluating `!` at its own width can.
#[test]
fn not_feeding_popcount_does_not_fabricate_a_witness() {
    let f = kernel(
        8,
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
                stmts: vec![stmt_eval(
                    5,
                    &[4],
                    Eval::Unary(UnOp::Not, Operand::Local(4), 32),
                )],
                term: term(call(
                    CallKind::CountOnes { bits: 32 },
                    "count_ones",
                    vec![Some(Operand::Local(5))],
                    Some(6),
                    3,
                )),
            },
            Block {
                stmts: vec![stmt_eval(
                    7,
                    &[6],
                    Eval::Binary(BinOp::Gt, Operand::Local(6), Operand::Const(0)),
                )],
                term: term(TermKind::Branch {
                    cond: 7,
                    targets: vec![5, 4],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 5)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    assert!(
        replay_hang(&f, 4, SiteKind::Barrier, 0).is_none(),
        "every lane reaches the barrier; there is no hang to witness"
    );
}
