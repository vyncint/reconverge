//! Bounded inlining: a barrier behind a helper becomes a replayable site.
use reconverge_core::dialect::CallKind;
use reconverge_core::inline::inline_calls;
use reconverge_core::model::{
    BinOp, Block, Callee, Eval, FnId, FnModel, Local, Operand, Stmt, Term, TermKind,
};

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
fn call(kind: CallKind, display: &str, local_fn: Option<FnId>, target: usize) -> TermKind {
    TermKind::Call {
        callee: Callee {
            kind,
            display: display.to_string(),
            local_fn,
        },
        args: Vec::new(),
        const_args: Vec::new(),
        arg_operands: Vec::new(),
        dest: None,
        target: Some(target),
    }
}
fn model(name: &str, local_count: usize, arg_count: usize, blocks: Vec<Block>) -> FnModel {
    FnModel {
        name: name.into(),
        item_path: format!("test::{name}"),
        span: 0,
        local_count,
        arg_count,
        local_names: vec![None; local_count],
        local_spans: vec![None; local_count],
        blocks,
        declared_block: None,
    }
}

/// `fn helper() { sync_threads(); }`
fn helper() -> FnModel {
    model(
        "helper",
        1,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::Barrier, "sync_threads", None, 1)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

/// `if lane % 2 == 0 { helper(); }` — the issue's reproduction.
fn kernel_calling(helper_id: FnId) -> FnModel {
    model(
        "probe",
        4,
        1,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::ThreadIndexWitness, "lane_id", None, 1)),
            },
            Block {
                stmts: vec![stmt_eval(
                    3,
                    &[2],
                    Eval::Binary(BinOp::Rem, Operand::Local(2), Operand::Const(2)),
                )],
                term: term(TermKind::Branch {
                    cond: 3,
                    targets: vec![2, 3],
                    values: vec![Some(0), None],
                }),
            },
            Block {
                stmts: vec![],
                term: term(call(CallKind::Other, "helper", Some(helper_id), 3)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    )
}

#[test]
fn a_barrier_behind_a_helper_becomes_a_site() {
    let fns = vec![helper(), kernel_calling(0)];
    let inlined = inline_calls(&fns, &fns[1], 2).expect("the helper is inlinable");
    assert_eq!(inlined.exposed.len(), 1, "one call was spliced");
    let (call_block, sites) = &inlined.exposed[0];
    assert_eq!(*call_block, 2, "the helper() call block");
    assert_eq!(sites.len(), 1, "one barrier came with it");
    assert!(matches!(
        inlined.model.blocks[sites[0]].term.kind,
        TermKind::Call { ref callee, .. } if callee.kind == CallKind::Barrier
    ));
    // The call is gone: nothing is left to defer to a summary.
    assert!(matches!(
        inlined.model.blocks[2].term.kind,
        TermKind::Goto { .. }
    ));
}

#[test]
fn recursion_is_refused_rather_than_unrolled() {
    // `fn loop_forever() { loop_forever(); }`
    let recursive = model(
        "loop_forever",
        1,
        0,
        vec![
            Block {
                stmts: vec![],
                term: term(call(CallKind::Other, "loop_forever", Some(0), 1)),
            },
            Block {
                stmts: vec![],
                term: term(TermKind::Return),
            },
        ],
    );
    let fns = vec![recursive];
    assert!(
        inline_calls(&fns, &fns[0], 2).is_none(),
        "a self-call exposes no site and must not be unrolled"
    );
}

#[test]
fn a_callee_with_no_site_inlines_to_nothing_worth_replaying() {
    let plain = model(
        "plain",
        1,
        0,
        vec![Block {
            stmts: vec![],
            term: term(TermKind::Return),
        }],
    );
    let fns = vec![plain, kernel_calling(0)];
    assert!(
        inline_calls(&fns, &fns[1], 2).is_none(),
        "nothing exposed means the summary tier still applies"
    );
}
