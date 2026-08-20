//! Which sites in a multi-site function get a witness (#25, #26).
//!
//! #25 measured promotion as covering a *prefix* of a function's divergent
//! sites, ending at the first change of divergence source, with a
//! lane-environment guard appearing first suppressing the whole function.
//! Those measurements were taken at 0.1.11, before the lane-environment
//! registers had values (#24) and before evaluation was width-typed (#22).
//!
//! What holds now is simpler, and these tests pin it: every site is
//! attempted independently, and the only thing that suppresses a later one
//! is an *unevaluable* branch whose diamond contains a synchronization
//! point. That case is not a gap — lanes may be stuck at that barrier and
//! never reach the later site, so a witness there would be fabricated.
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
        declared_block: None,
    }
}

/// Two guarded barriers in sequence. Each `src` is ("kind", "display"):
/// a thread-index guard (`i % 2 == 0`) or a lane-environment one
/// (`lanemask_eq & 1`).
fn two_sites(first_lane_env: bool, second_lane_env: bool) -> (FnModel, usize, usize) {
    let mut blocks = Vec::new();
    let mut local = 2usize;
    let mut sites = Vec::new();
    for lane_env in [first_lane_env, second_lane_env] {
        let base = blocks.len();
        let v = local;
        let c = local + 1;
        local += 2;
        if lane_env {
            blocks.push(Block {
                stmts: vec![],
                term: term(call(
                    CallKind::DivergentEnvRead,
                    "lanemask_eq",
                    vec![],
                    Some(v),
                    base + 1,
                )),
            });
            blocks.push(Block {
                stmts: vec![stmt_eval(
                    c,
                    &[v],
                    Eval::Binary(BinOp::BitAnd, Operand::Local(v), Operand::Const(1)),
                )],
                term: term(TermKind::Branch {
                    cond: c,
                    targets: vec![base + 3, base + 2],
                    values: vec![Some(0), None],
                }),
            });
        } else {
            blocks.push(Block {
                stmts: vec![],
                term: term(call(
                    CallKind::ThreadIndexWitness,
                    "lane_id",
                    vec![],
                    Some(v),
                    base + 1,
                )),
            });
            blocks.push(Block {
                stmts: vec![stmt_eval(
                    c,
                    &[v],
                    Eval::Binary(BinOp::Rem, Operand::Local(v), Operand::Const(2)),
                )],
                term: term(TermKind::Branch {
                    cond: c,
                    targets: vec![base + 2, base + 3],
                    values: vec![Some(0), None],
                }),
            });
        }
        sites.push(base + 2);
        blocks.push(Block {
            stmts: vec![],
            term: term(call(
                CallKind::Barrier,
                "sync_threads",
                vec![],
                None,
                base + 3,
            )),
        });
        blocks.push(Block {
            stmts: vec![],
            term: term(TermKind::Goto { target: base + 4 }),
        });
    }
    let n = blocks.len();
    blocks.push(Block {
        stmts: vec![],
        term: term(TermKind::Return),
    });
    let _ = n;
    let f = kernel(local, blocks);
    (f, sites[0], sites[1])
}

fn both_promoted(f: &FnModel, s1: usize, s2: usize, label: &str) {
    assert!(
        replay_hang(f, s1, SiteKind::Barrier, 0).is_some(),
        "{label}: the first site must be witnessed"
    );
    assert!(
        replay_hang(f, s2, SiteKind::Barrier, 0).is_some(),
        "{label}: the second site must be witnessed too — promotion is per site"
    );
}

/// An unevaluable guard (`active_mask`, still unknown after #24) first,
/// then a provable thread-index site. `barrier_inside` puts a barrier
/// within the unevaluable diamond; without it the diamond is skippable.
fn unevaluable_then_provable(barrier_inside: bool) -> (FnModel, usize) {
    let mut blocks = vec![
        Block {
            stmts: vec![],
            term: term(call(
                CallKind::DivergentEnvRead,
                "active_mask",
                vec![],
                Some(2),
                1,
            )),
        },
        Block {
            stmts: vec![stmt_eval(
                3,
                &[2],
                Eval::Binary(BinOp::BitAnd, Operand::Local(2), Operand::Const(1)),
            )],
            term: term(TermKind::Branch {
                cond: 3,
                targets: vec![3, 2],
                values: vec![Some(0), None],
            }),
        },
    ];
    if barrier_inside {
        blocks.push(Block {
            stmts: vec![],
            term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 3)),
        });
    } else {
        blocks.push(Block {
            stmts: vec![],
            term: term(TermKind::Goto { target: 3 }),
        });
    }
    blocks.push(Block {
        stmts: vec![],
        term: term(call(
            CallKind::ThreadIndexWitness,
            "lane_id",
            vec![],
            Some(4),
            4,
        )),
    });
    blocks.push(Block {
        stmts: vec![stmt_eval(
            5,
            &[4],
            Eval::Binary(BinOp::Rem, Operand::Local(4), Operand::Const(2)),
        )],
        term: term(TermKind::Branch {
            cond: 5,
            targets: vec![5, 6],
            values: vec![Some(0), None],
        }),
    });
    blocks.push(Block {
        stmts: vec![],
        term: term(call(CallKind::Barrier, "sync_threads", vec![], None, 6)),
    });
    blocks.push(Block {
        stmts: vec![],
        term: term(TermKind::Return),
    });
    (kernel(6, blocks), 5)
}

/// Both sites are witnessed whatever the pair of divergence sources —
/// including a lane-environment source first, which #25 measured as
/// suppressing the entire function.
#[test]
fn every_site_is_attempted_whatever_the_divergence_sources() {
    let (f, a, b) = two_sites(false, false);
    both_promoted(&f, a, b, "index, index");
    let (f, a, b) = two_sites(false, true);
    both_promoted(&f, a, b, "index, lane-environment");
    let (f, a, b) = two_sites(true, false);
    both_promoted(&f, a, b, "lane-environment, index");
    let (f, a, b) = two_sites(true, true);
    both_promoted(&f, a, b, "lane-environment, lane-environment");
}

/// An unevaluable guard upstream does not by itself suppress a later site:
/// when its diamond holds no synchronization point, the choice cannot
/// change who arrives, so the replay skips it and the later site is
/// witnessed.
#[test]
fn an_unevaluable_guard_upstream_does_not_suppress_a_later_site() {
    let (f, site) = unevaluable_then_provable(false);
    assert!(
        replay_hang(&f, site, SiteKind::Barrier, 0).is_some(),
        "a skippable unknown diamond must not cost the site below it"
    );
}

/// The one case that does suppress it, and correctly. With a barrier
/// inside the unevaluable diamond, the lanes may be stuck there and never
/// reach the later site — so a witness naming a lane split at that site
/// would be fabricated. Declining is the sound answer, not a gap.
#[test]
fn an_unevaluable_guard_over_a_barrier_declines_the_later_site() {
    let (f, site) = unevaluable_then_provable(true);
    assert!(
        replay_hang(&f, site, SiteKind::Barrier, 0).is_none(),
        "lanes may never leave the upstream barrier; the site is not provable"
    );
}

/// `A, !A, A`: the third site is not promoted, and correctly so.
///
/// Found by the differential corpus in `vyncint/simt-diff`, which had this
/// shape recorded and which the documentation written for #25 and #26 did
/// not describe — it named only the unevaluable-branch case. All three
/// guards here are evaluable. What stops the third site is that the first
/// two already deadlock the block: the even lanes wait forever at barrier
/// one and the odd lanes at barrier two, so nothing reaches barrier three.
/// Unreachable in fact, not merely unproven.
#[test]
fn earlier_barriers_that_deadlock_the_block_stop_a_later_site() {
    let mut blocks = Vec::new();
    // Three guarded barriers: lane % 2 == 0, then != 0, then == 0 again.
    for (i, want) in [0u128, 1, 0].into_iter().enumerate() {
        let base = i * 4;
        let (v, c) = (2 + i * 2, 3 + i * 2);
        blocks.push(Block {
            stmts: vec![],
            term: term(call(
                CallKind::ThreadIndexWitness,
                "lane_id",
                vec![],
                Some(v),
                base + 1,
            )),
        });
        blocks.push(Block {
            stmts: vec![stmt_eval(
                c,
                &[v],
                Eval::Binary(BinOp::Rem, Operand::Local(v), Operand::Const(2)),
            )],
            term: term(TermKind::Branch {
                cond: c,
                targets: vec![base + 2, base + 3],
                values: vec![Some(want), None],
            }),
        });
        blocks.push(Block {
            stmts: vec![],
            term: term(call(
                CallKind::Barrier,
                "sync_threads",
                vec![],
                None,
                base + 3,
            )),
        });
        blocks.push(Block {
            stmts: vec![],
            term: term(TermKind::Goto { target: base + 4 }),
        });
    }
    blocks.push(Block {
        stmts: vec![],
        term: term(TermKind::Return),
    });
    let f = kernel(2 + 6, blocks);

    assert!(
        replay_hang(&f, 2, SiteKind::Barrier, 0).is_some(),
        "the first site is witnessed"
    );
    assert!(
        replay_hang(&f, 6, SiteKind::Barrier, 0).is_some(),
        "so is the second: its complement guard is evaluable too"
    );
    assert!(
        replay_hang(&f, 10, SiteKind::Barrier, 0).is_none(),
        "the third is unreachable — both halves are already stuck above it"
    );
}
