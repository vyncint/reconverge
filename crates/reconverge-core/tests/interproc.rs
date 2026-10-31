//! Divergence across function boundaries (#188, #189), on hand-built
//! models: a helper's parameters, its return value, and the closures a
//! library call is handed.

use reconverge_core::analysis::{ReasonKind, Summaries};
use reconverge_core::dialect::CallKind;
use reconverge_core::inline::inline_all;
use reconverge_core::interproc::{analyze_kernel, provenance, reachable};
use reconverge_core::model::{Block, Callee, FnId, FnModel, Local, Stmt, Term, TermKind};
use reconverge_core::{LaunchScope, Uniformity};

fn term(kind: TermKind) -> Term {
    Term { kind, span: 0 }
}

fn stmt(dest: Local, uses: &[Local]) -> Stmt {
    Stmt {
        dest: Some(dest),
        uses: uses.to_vec(),
        eval: None,
        opaque: false,
        span: 0,
    }
}

fn block(stmts: Vec<Stmt>, kind: TermKind) -> Block {
    Block {
        stmts,
        term: term(kind),
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
        declared_cluster: None,
    }
}

/// A call. `args` are positional: one local per argument.
fn call(
    kind: CallKind,
    display: &str,
    local_fn: Option<FnId>,
    may_call: Vec<FnId>,
    args: &[Local],
    dest: Option<Local>,
    target: usize,
) -> TermKind {
    TermKind::Call {
        callee: Callee {
            kind,
            scope: LaunchScope::Block,
            display: display.into(),
            local_fn,
            may_call,
        },
        args: args.to_vec(),
        arg_uses: args.iter().map(|&a| vec![a]).collect(),
        const_args: vec![None; args.len()],
        arg_operands: vec![None; args.len()],
        dest,
        target: Some(target),
    }
}

fn branch(cond: Local, then: usize, otherwise: usize) -> TermKind {
    TermKind::Branch {
        cond,
        targets: vec![then, otherwise],
        values: vec![],
    }
}

/// `fn step(tid: u32) { if tid % 2 == 0 { sync_threads() } }`
/// locals: 0 ret, 1 tid, 2 cond
fn param_guarded_barrier() -> FnModel {
    model(
        "step",
        3,
        1,
        vec![
            block(vec![stmt(2, &[1])], branch(2, 1, 2)),
            block(
                vec![],
                call(
                    CallKind::Barrier,
                    "sync_threads",
                    None,
                    vec![],
                    &[],
                    None,
                    2,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    )
}

/// A kernel calling `callee(<value>)`, the value read from `source`.
/// locals: 0 ret, 1 value
fn kernel_passing(name: &str, source: CallKind, callee: FnId) -> FnModel {
    model(
        name,
        2,
        0,
        vec![
            block(
                vec![],
                call(source, "source", None, vec![], &[], Some(1), 1),
            ),
            block(
                vec![],
                call(CallKind::Other, "step", Some(callee), vec![], &[1], None, 2),
            ),
            block(vec![], TermKind::Return),
        ],
    )
}

#[test]
fn a_helper_parameter_is_divergent_when_a_call_passes_a_thread_index() {
    // 0 `step`, 1 the kernel passing `threadIdx_x()`.
    let fns = vec![
        param_guarded_barrier(),
        kernel_passing("k", CallKind::ThreadIndexWitness, 0),
    ];
    let summaries = Summaries::compute(&fns);
    let ctx = analyze_kernel(&fns, &summaries, 1);

    let step = ctx.get(0).expect("the kernel reaches its helper");
    assert_eq!(step.analysis.locals[1], Uniformity::Divergent, "tid");
    assert!(matches!(
        step.analysis.reasons[1].as_ref().map(|r| &r.kind),
        Some(ReasonKind::Parameter)
    ));
    let site = &step.analysis.barriers[0];
    let cause = site
        .divergent_cause
        .expect("the barrier inside the helper is under a divergent guard");

    // The chain crosses back into the kernel and ends at the source.
    let chain = provenance(&fns, &summaries, &ctx, 0, cause.cond);
    assert!(
        chain
            .last()
            .is_some_and(|s| s.detail.contains("thread-index witness")),
        "the chain must end at the kernel's thread index: {chain:?}"
    );
    assert!(
        chain
            .iter()
            .any(|s| s.detail.contains("parameter receives")),
        "and say where it crossed: {chain:?}"
    );
}

#[test]
fn the_join_is_per_kernel_so_a_uniform_caller_is_not_blamed() {
    // 0 `step`; 1 passes a thread index, 2 passes a block index.
    let fns = vec![
        param_guarded_barrier(),
        kernel_passing("divergent", CallKind::ThreadIndexWitness, 0),
        kernel_passing("uniform", CallKind::BlockUniform, 0),
    ];
    let summaries = Summaries::compute(&fns);

    let divergent = analyze_kernel(&fns, &summaries, 1);
    assert!(
        divergent.get(0).unwrap().analysis.barriers[0]
            .divergent_cause
            .is_some()
    );

    let uniform = analyze_kernel(&fns, &summaries, 2);
    let step = uniform.get(0).unwrap();
    assert_eq!(step.analysis.locals[1], Uniformity::Uniform);
    assert!(
        step.analysis.barriers[0].divergent_cause.is_none(),
        "a block-uniform argument is a uniform guard"
    );
    // ... but it is only block-constant, which the helper is told too.
    assert_eq!(step.analysis.local_scopes[1], LaunchScope::Block);
}

/// `fn tid() -> u32 { threadIdx_x() }` — locals: 0 ret
fn returns_thread_index() -> FnModel {
    model(
        "tid",
        1,
        0,
        vec![
            block(
                vec![],
                call(
                    CallKind::ThreadIndexWitness,
                    "threadIdx_x",
                    None,
                    vec![],
                    &[],
                    Some(0),
                    1,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    )
}

/// `if callee() == 0 { sync_threads() }` — locals: 0 ret, 1 result, 2 cond
fn kernel_branching_on(callee: FnId) -> FnModel {
    model(
        "k",
        3,
        0,
        vec![
            block(
                vec![],
                call(
                    CallKind::Other,
                    "tid",
                    Some(callee),
                    vec![],
                    &[],
                    Some(1),
                    1,
                ),
            ),
            block(vec![stmt(2, &[1])], branch(2, 2, 3)),
            block(
                vec![],
                call(
                    CallKind::Barrier,
                    "sync_threads",
                    None,
                    vec![],
                    &[],
                    None,
                    3,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    )
}

#[test]
fn a_divergent_return_value_reaches_the_caller() {
    let fns = vec![returns_thread_index(), kernel_branching_on(0)];
    let summaries = Summaries::compute(&fns);
    assert!(summaries.returns_divergent[0]);

    let ctx = analyze_kernel(&fns, &summaries, 1);
    let k = ctx.root();
    assert_eq!(k.analysis.locals[1], Uniformity::Divergent);
    let cause = k.analysis.barriers[0]
        .divergent_cause
        .expect("a branch on a helper's thread index is divergent");
    let chain = provenance(&fns, &summaries, &ctx, 1, cause.cond);
    assert!(
        chain
            .last()
            .is_some_and(|s| s.detail.contains("thread-index witness `threadIdx_x()`")),
        "the chain follows the return value into the helper: {chain:?}"
    );
}

#[test]
fn a_returned_block_index_is_block_scoped_at_the_call() {
    // `fn block_of() -> u32 { blockIdx_x() }`
    let block_of = model(
        "block_of",
        1,
        0,
        vec![
            block(
                vec![],
                call(
                    CallKind::BlockUniform,
                    "blockIdx_x",
                    None,
                    vec![],
                    &[],
                    Some(0),
                    1,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    );
    let fns = vec![block_of, kernel_branching_on(0)];
    let summaries = Summaries::compute(&fns);
    assert!(!summaries.returns_divergent[0]);
    assert_eq!(summaries.return_scope[0], LaunchScope::Block);

    let ctx = analyze_kernel(&fns, &summaries, 1);
    assert_eq!(ctx.root().analysis.locals[1], Uniformity::Uniform);
    assert_eq!(ctx.root().analysis.local_scopes[1], LaunchScope::Block);
}

#[test]
fn a_closure_handed_to_a_library_call_counts_toward_its_summary() {
    // 0: `|_| sync_threads()`; 1: `if tid < 4 { (0..2).for_each(<0>) }`
    // kernel locals: 0 ret, 1 tid, 2 cond, 3 range
    let closure = model(
        "closure",
        2,
        1,
        vec![
            block(
                vec![],
                call(
                    CallKind::Barrier,
                    "sync_threads",
                    None,
                    vec![],
                    &[],
                    None,
                    1,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    );
    let k = model(
        "k",
        4,
        0,
        vec![
            block(
                vec![],
                call(
                    CallKind::ThreadIndexWitness,
                    "threadIdx_x",
                    None,
                    vec![],
                    &[],
                    Some(1),
                    1,
                ),
            ),
            block(vec![stmt(2, &[1])], branch(2, 2, 3)),
            block(
                vec![],
                call(
                    CallKind::Other,
                    "Iterator::for_each",
                    None,
                    vec![0],
                    &[3],
                    None,
                    3,
                ),
            ),
            block(vec![], TermKind::Return),
        ],
    );
    let fns = vec![closure, k];
    let summaries = Summaries::compute(&fns);
    assert!(summaries.may_contain_barrier[1], "through the closure");
    assert_eq!(reachable(&fns, 1), vec![1, 0]);

    let ctx = analyze_kernel(&fns, &summaries, 1);
    let site = &ctx.root().analysis.barriers[0];
    assert!(site.interprocedural);
    assert!(site.divergent_cause.is_some());

    // Not a concrete path: nothing says how often `for_each` runs it.
    assert!(
        inline_all(&fns, &fns[1], 2).is_none(),
        "a closure reached through a library call is never spliced"
    );
}

#[test]
fn splicing_records_which_helper_block_each_copy_came_from() {
    // The kernel calls `step` twice; each call splices its own copy.
    // kernel locals: 0 ret, 1 value
    let k = model(
        "k",
        2,
        0,
        vec![
            block(
                vec![],
                call(
                    CallKind::ThreadIndexWitness,
                    "threadIdx_x",
                    None,
                    vec![],
                    &[],
                    Some(1),
                    1,
                ),
            ),
            block(
                vec![],
                call(CallKind::Other, "step", Some(0), vec![], &[1], None, 2),
            ),
            block(
                vec![],
                call(CallKind::Other, "step", Some(0), vec![], &[1], None, 3),
            ),
            block(vec![], TermKind::Return),
        ],
    );
    let fns = vec![param_guarded_barrier(), k];
    let inlined = inline_all(&fns, &fns[1], 2).expect("two calls to splice");
    assert_eq!(inlined.origins.len(), inlined.model.blocks.len());
    assert!(
        inlined.origins[..4].iter().all(Option::is_none),
        "the kernel's own blocks keep their ids and have no origin"
    );
    let copies = inlined
        .origins
        .iter()
        .filter(|o| **o == Some((0, 1)))
        .count();
    assert_eq!(copies, 2, "one copy of the barrier block per call");
}
