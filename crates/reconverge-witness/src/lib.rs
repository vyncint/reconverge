//! Warp-accurate replay of divergence findings — never a general-purpose
//! kernel runtime.
//!
//! Given a function model (the same one the uniformity engine analyzes), a
//! finding site, and a launch shape, the interpreter runs each of the 32
//! lanes through the kernel-subset semantics the adapter captured
//! (integer arithmetic, copies, branches with known guard values, dialect
//! calls). Interpretation is **witness-directed**: a lane stops the moment
//! it either arrives at the site or enters a block from which the site is
//! unreachable ("never arrives") — so unknown values elsewhere in the
//! kernel cannot spoil a replay that never needed them.
//!
//! Anything genuinely unknown on the way — a branch on a parameter, a
//! loop past the step budget, an unmodeled operation — aborts the replay:
//! **no witness, the static result stands**. One sound
//! exception: an unknown branch whose arms all rejoin before the site — the
//! short-circuit `&&` / `if let` diamonds of everyday bounds checks — is
//! skipped whole, with every effect it could have had erased rather than
//! guessed (see `skip_unknowable_diamond` for the exact conditions). A
//! successful replay is a concrete thread configuration plus a lane
//! timeline, and it promotes the finding to `confirmed`.
//!
//! Verdict wording is calibrated: hardware behavior is "usually" a hang,
//! never "always" (the project docs).

#![forbid(unsafe_code)]

use reconverge_artifacts::witness::{LaneState, VerdictKind};
use reconverge_core::dialect::CallKind;
use reconverge_core::graph::{Cfg, post_dominators};
use reconverge_core::model::{BinOp, BlockId, Eval, FnModel, Operand, SpanRef, TermKind, UnOp};

/// Lanes per warp on every supported target.
pub const LANES: u32 = 32;

/// Identifier of the witness artifact schema this crate's replays become.
#[must_use]
pub fn emitted_schema() -> &'static str {
    reconverge_artifacts::schema::WITNESS
}

/// Per-lane execution step budget; loops beyond this bail out.
const STEP_BUDGET: usize = 4096;

/// What kind of site is being replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Barrier,
    /// A warp collective with its constant participation mask, when known.
    Collective {
        mask: Option<u64>,
    },
}

/// One timeline step of a successful replay. Spans are the model's opaque
/// references; the driver maps them to real source spans.
#[derive(Debug, Clone)]
pub struct ReplayStep {
    pub statement: String,
    pub span: Option<SpanRef>,
    pub lane_changes: Vec<(u8, LaneState)>,
    /// (arrived, expected) for barrier steps.
    pub barrier: Option<(u32, u32)>,
    /// (op, mask, active) for collective steps.
    pub warp_op: Option<(String, u32, u32)>,
}

/// A concrete replay of one finding: launch shape, timeline, verdict.
#[derive(Debug, Clone)]
pub struct Replay {
    pub block: [u32; 3],
    pub grid: [u32; 3],
    pub steps: Vec<ReplayStep>,
    pub verdict_kind: VerdictKind,
    pub verdict_message: String,
    pub verdict_step: usize,
    /// Bitmask of lanes that arrived at the site (bit per lane; the block
    /// may be more than one warp, so this is wider than a warp mask).
    pub arrived: u128,
    /// Bitmask of lanes that can never arrive.
    pub never_arrives: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneStop {
    Arrived,
    NeverArrives,
    AtBarrier(BlockId),
    Bailed,
}

struct Lane {
    store: Vec<Option<u128>>,
    block: BlockId,
    /// Resume after this many statements of the current block (used when a
    /// lane is released from a non-site barrier, whose call is the block
    /// terminator).
    resume_past_terminator: bool,
    steps: usize,
    stop: Option<LaneStop>,
}

/// Try to replay a hang for the finding at `site_block` under a
/// single-warp launch (block `[32,1,1]`, grid `[1,1,1]`).
///
/// Returns `None` — no witness — whenever any lane's path to a conclusion
/// depends on something the interpreter cannot know.
#[must_use]
pub fn replay_hang(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
) -> Option<Replay> {
    replay_hang_at(f, site_block, site, cause_span, LANES)
}

/// Like [`replay_hang`], under a block of `lanes` threads (`[lanes,1,1]`,
/// grid `[1,1,1]`) — the shape a kernel's launch contract declares.
///
/// `lanes` must be a multiple of 32 between 32 and 128. Beyond one warp,
/// only barrier sites are replayed, and any warp collective on any lane's
/// path aborts: a collective synchronizes *within* each warp, and modeling
/// that per-warp release choreography wrongly could fabricate a witness.
#[must_use]
pub fn replay_hang_at(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
    lanes: u32,
) -> Option<Replay> {
    if !(LANES..=128).contains(&lanes) || !lanes.is_multiple_of(LANES) {
        return None;
    }
    if lanes > LANES && !matches!(site, SiteKind::Barrier) {
        return None;
    }
    let cfg = Cfg::build(f);
    let can_reach = reaches(&cfg, site_block);
    if !can_reach[0] {
        return None; // the entry cannot reach the site at all
    }
    // Post-dominators with every escape counted as an exit — returns,
    // aborts, unmodeled jumps, and diverging calls alike. When the site
    // post-dominates a block, every modeled path from that block reaches
    // the site before leaving the function.
    let is_exit: Vec<bool> = f
        .blocks
        .iter()
        .map(|b| {
            matches!(
                b.term.kind,
                TermKind::Return
                    | TermKind::Halt
                    | TermKind::Jump { .. }
                    | TermKind::Opaque { target: None, .. }
                    | TermKind::Call { target: None, .. }
            )
        })
        .collect();
    let ipdom = post_dominators(&cfg, &is_exit);
    let n_lanes = lanes;
    let ctx = ReplayCtx {
        f,
        cfg: &cfg,
        can_reach: &can_reach,
        ipdom: &ipdom,
        site_block,
        site,
        lanes: n_lanes,
    };

    let mut lanes: Vec<Lane> = (0..n_lanes)
        .map(|_| Lane {
            store: vec![None; f.local_count],
            block: 0,
            resume_past_terminator: false,
            steps: 0,
            stop: None,
        })
        .collect();

    // Round-based execution: run every unfinished lane to a stop; when all
    // still-running lanes are parked at the same non-site barrier, release
    // them together and continue.
    let mut released_syncs = 0usize;
    loop {
        for (lane_id, lane) in lanes.iter_mut().enumerate() {
            if lane.stop.is_none() || lane.stop == Some(LaneStop::AtBarrier(usize::MAX)) {
                lane.stop = Some(run_lane(&ctx, lane, lane_id as u32));
            }
        }
        if lanes.iter().any(|l| l.stop == Some(LaneStop::Bailed)) {
            return None;
        }
        let parked: Vec<BlockId> = lanes
            .iter()
            .filter_map(|l| match l.stop {
                Some(LaneStop::AtBarrier(b)) => Some(b),
                _ => None,
            })
            .collect();
        if parked.is_empty() {
            break;
        }
        if lanes.iter().any(|l| l.stop == Some(LaneStop::Arrived)) {
            // Lanes are split between the site and one or more earlier
            // barriers: a mutual deadlock. The arrived lanes hold the site
            // forever — a barrier site waits for the whole block, and a
            // collective site is only ever emitted when its mask names a
            // lane that is absent (`build_replay`) — so no parked barrier
            // can be satisfied either: it waits for the arrived lanes, who
            // wait for it. The parked lanes therefore never arrive.
            for lane in &mut lanes {
                if matches!(lane.stop, Some(LaneStop::AtBarrier(_))) {
                    lane.stop = Some(LaneStop::NeverArrives);
                }
            }
            continue;
        }
        let first = parked[0];
        let all_running_parked_together = parked.len()
            == lanes
                .iter()
                .filter(|l| {
                    !matches!(
                        l.stop,
                        Some(LaneStop::Arrived) | Some(LaneStop::NeverArrives)
                    )
                })
                .count()
            && parked.iter().all(|&b| b == first);
        if !all_running_parked_together {
            // Lanes are parked across different barriers with none at the
            // site: everyone is stuck upstream and nobody arrives — nothing
            // for this site's replay to witness.
            return None;
        }
        // Release: everyone passes the intervening barrier together.
        released_syncs += 1;
        if released_syncs > 64 {
            return None;
        }
        for lane in &mut lanes {
            lane.resume_past_terminator = true;
            lane.stop = Some(LaneStop::AtBarrier(usize::MAX)); // marker: resume
        }
    }

    let mut arrived: u128 = 0;
    let mut never: u128 = 0;
    for (lane_id, lane) in lanes.iter().enumerate() {
        match lane.stop {
            Some(LaneStop::Arrived) => arrived |= 1 << lane_id,
            Some(LaneStop::NeverArrives) => never |= 1 << lane_id,
            _ => return None,
        }
    }
    if arrived == 0 || never == 0 {
        return None; // uniform behavior: nothing to witness
    }

    build_replay(f, site_block, site, cause_span, arrived, never, n_lanes)
}

/// Blocks from which `site` is reachable (including the site itself).
fn reaches(cfg: &Cfg, site: BlockId) -> Vec<bool> {
    let mut seen = vec![false; cfg.len()];
    let mut stack = vec![site];
    seen[site] = true;
    while let Some(b) = stack.pop() {
        for &p in &cfg.preds[b] {
            if !seen[p] {
                seen[p] = true;
                stack.push(p);
            }
        }
    }
    seen
}

fn operand_value(store: &[Option<u128>], operand: Operand) -> Option<u128> {
    match operand {
        Operand::Local(local) => store[local],
        Operand::Const(value) => Some(value),
    }
}

/// Low `bits` set, saturating at the store's own width.
fn width_mask(bits: u32) -> u128 {
    if bits >= u128::BITS {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn eval(store: &[Option<u128>], e: Eval) -> Option<u128> {
    match e {
        Eval::Use(op) => operand_value(store, op),
        Eval::Unary(op, a, bits) => {
            let a = operand_value(store, a)?;
            // Both operations are exact at a known width: the store holds
            // an `n`-bit value zero-extended, so complementing or negating
            // within `n` bits and masking back is the value the program
            // itself holds. At `n` = 1 the complement is boolean negation,
            // which is what a condition needs.
            let mask = width_mask(bits);
            Some(match op {
                UnOp::Not => !a & mask,
                UnOp::Neg => a.wrapping_neg() & mask,
            })
        }
        Eval::Cast(a, bits) => Some(operand_value(store, a)? & width_mask(bits)),
        Eval::CheckedBinary(op, a, b, bits) => {
            // The checked form panics the thread on overflow: a value past
            // the width never exists in the real program, so the result is
            // the exact in-range value or nothing. (Underflow wraps to the
            // top of the 128-bit range and fails the same width test.)
            let value = eval(store, Eval::Binary(op, a, b))?;
            (value >> bits == 0).then_some(value)
        }
        Eval::Binary(op, a, b) => {
            let a = operand_value(store, a)?;
            let b = operand_value(store, b)?;
            Some(match op {
                BinOp::Add => a.wrapping_add(b),
                BinOp::Sub => a.wrapping_sub(b),
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::Div => a.checked_div(b)?,
                BinOp::Rem => a.checked_rem(b)?,
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                BinOp::BitXor => a ^ b,
                BinOp::Shl => a.checked_shl(u32::try_from(b).ok()?)?,
                BinOp::Shr => a.checked_shr(u32::try_from(b).ok()?)?,
                BinOp::Eq => u128::from(a == b),
                BinOp::Ne => u128::from(a != b),
                BinOp::Lt => u128::from(a < b),
                BinOp::Le => u128::from(a <= b),
                BinOp::Gt => u128::from(a > b),
                BinOp::Ge => u128::from(a >= b),
            })
        }
    }
}

/// Handle a branch whose condition the interpreter cannot know.
///
/// The decision provably cannot change who arrives at the site when the
/// whole diamond rejoins: every path from the branch reaches its immediate
/// post-dominator (aborts, unmodeled jumps, and returns all count as exits
/// when the post-dominators are computed, so none hides in the region), and
/// the region between them contains no site, no barrier or collective (their
/// release choreography must be simulated, not skipped), and no cycle (a
/// loop could refuse to terminate in some world). In that case the region's
/// every possible effect is erased — each destination it could assign
/// becomes unknown, fabricating nothing — and the lane lands at the join.
///
/// Returns the join block, or `None`: no witness, the static result stands.
fn skip_unknowable_diamond(
    f: &FnModel,
    cfg: &Cfg,
    ipdom: &[Option<BlockId>],
    branch: BlockId,
    site_block: BlockId,
    store: &mut [Option<u128>],
) -> Option<BlockId> {
    let join = ipdom[branch]?;

    // The region strictly between the branch and the join.
    let mut in_region = vec![false; cfg.len()];
    let mut region = Vec::new();
    let mut stack: Vec<BlockId> = cfg.succs[branch].to_vec();
    while let Some(b) = stack.pop() {
        if b == join || in_region[b] {
            continue;
        }
        in_region[b] = true;
        region.push(b);
        stack.extend(cfg.succs[b].iter().copied());
    }

    for &b in &region {
        if b == site_block {
            return None; // arrival genuinely depends on the unknown
        }
        // A region block whose post-dominator chain is the virtual exit
        // either escapes without the join (contradiction) or cannot reach
        // any exit at all (an infinite loop): bail either way.
        ipdom[b]?;
        match &f.blocks[b].term.kind {
            TermKind::Call { callee, .. }
                if matches!(callee.kind, CallKind::Barrier | CallKind::WarpCollective) =>
            {
                return None;
            }
            TermKind::Return | TermKind::Halt | TermKind::Jump { .. } => return None,
            _ => {}
        }
    }

    // No cycles inside the region: Kahn's algorithm on the region subgraph.
    let mut indegree = vec![0usize; cfg.len()];
    for &b in &region {
        for &s in &cfg.succs[b] {
            if s != join && in_region[s] {
                indegree[s] += 1;
            }
        }
    }
    let mut ready: Vec<BlockId> = region
        .iter()
        .copied()
        .filter(|&b| indegree[b] == 0)
        .collect();
    let mut removed = 0usize;
    while let Some(b) = ready.pop() {
        removed += 1;
        for &s in &cfg.succs[b] {
            if s != join && in_region[s] {
                indegree[s] -= 1;
                if indegree[s] == 0 {
                    ready.push(s);
                }
            }
        }
    }
    if removed != region.len() {
        return None; // a cycle: some world may never leave it
    }

    // Erase every effect the region could have had.
    for &b in &region {
        let block = &f.blocks[b];
        for stmt in &block.stmts {
            if let Some(dest) = stmt.dest {
                store[dest] = None;
            }
        }
        match &block.term.kind {
            TermKind::Call { dest: Some(d), .. } | TermKind::Opaque { dest: Some(d), .. } => {
                store[*d] = None;
            }
            _ => {}
        }
    }
    Some(join)
}

/// Immutable per-replay context shared by every lane.
struct ReplayCtx<'a> {
    f: &'a FnModel,
    cfg: &'a Cfg,
    can_reach: &'a [bool],
    ipdom: &'a [Option<BlockId>],
    site_block: BlockId,
    site: SiteKind,
    /// Threads in the replayed block (a multiple of 32, at most 128).
    lanes: u32,
}

fn run_lane(ctx: &ReplayCtx<'_>, lane: &mut Lane, lane_id: u32) -> LaneStop {
    loop {
        lane.steps += 1;
        if lane.steps > STEP_BUDGET {
            return LaneStop::Bailed;
        }
        if !ctx.can_reach[lane.block] {
            return LaneStop::NeverArrives;
        }

        let block = &ctx.f.blocks[lane.block];
        if !lane.resume_past_terminator {
            for stmt in &block.stmts {
                if let Some(dest) = stmt.dest {
                    lane.store[dest] = stmt.eval.and_then(|e| eval(&lane.store, e));
                }
            }
        }
        let resuming = lane.resume_past_terminator;
        lane.resume_past_terminator = false;

        match &block.term.kind {
            TermKind::Goto { target } => lane.block = *target,
            TermKind::Jump { .. } => return LaneStop::Bailed,
            TermKind::Return | TermKind::Halt => return LaneStop::NeverArrives,
            TermKind::Branch {
                cond,
                targets,
                values,
            } => {
                if values.len() != targets.len() {
                    return LaneStop::Bailed;
                }
                let chosen = match lane.store[*cond] {
                    Some(v) => {
                        let mut chosen = None;
                        for (value, target) in values.iter().zip(targets) {
                            match value {
                                Some(guard) if *guard == v => {
                                    chosen = Some(*target);
                                    break;
                                }
                                None => chosen = Some(*target), // otherwise edge
                                _ => {}
                            }
                        }
                        chosen
                    }
                    // An unknown decision that cannot matter: when every
                    // path from this branch rejoins at its immediate
                    // post-dominator without touching the site, a
                    // synchronization point, or a cycle, the choice cannot
                    // change who arrives — skip the whole diamond, erase
                    // everything it could assign (no value is ever
                    // fabricated), and land at the join. Anything else
                    // stays a bail: no witness, the static result stands.
                    None => skip_unknowable_diamond(
                        ctx.f,
                        ctx.cfg,
                        ctx.ipdom,
                        lane.block,
                        ctx.site_block,
                        &mut lane.store,
                    ),
                };
                match chosen {
                    Some(target) => lane.block = target,
                    None => return LaneStop::Bailed,
                }
            }
            TermKind::Opaque { dest, target, .. } => {
                if let Some(dest) = dest {
                    lane.store[*dest] = None;
                }
                match target {
                    Some(t) => lane.block = *t,
                    None => return LaneStop::NeverArrives,
                }
            }
            TermKind::Call {
                callee,
                arg_operands,
                dest,
                target,
                ..
            } => {
                if lane.block == ctx.site_block && !resuming {
                    return LaneStop::Arrived;
                }
                let result: Option<u128> = if resuming {
                    None // the released barrier's unit result
                } else {
                    match callee.kind {
                        CallKind::Barrier => return LaneStop::AtBarrier(lane.block),
                        CallKind::WarpCollective if ctx.lanes > LANES => {
                            // A collective synchronizes within each warp;
                            // the multi-warp replay does not model that
                            // per-warp choreography, and guessing it could
                            // fabricate a witness.
                            return LaneStop::Bailed;
                        }
                        CallKind::WarpCollective => {
                            // A non-site collective synchronizes too; treat
                            // it like a barrier for the release logic.
                            let _ = ctx.site;
                            return LaneStop::AtBarrier(lane.block);
                        }
                        CallKind::ThreadIndexWitness => {
                            thread_index_value(&callee.display, lane_id)
                        }
                        CallKind::BlockUniform => block_uniform_value(&callee.display, ctx.lanes),
                        CallKind::DivergentEnvRead => {
                            lane_env_value(&callee.display, lane_id, ctx.lanes)
                        }
                        CallKind::WitnessRead => arg_operands
                            .first()
                            .copied()
                            .flatten()
                            .and_then(|op| operand_value(&lane.store, op)),
                        _ => None,
                    }
                };
                if let Some(dest) = dest {
                    lane.store[*dest] = result;
                }
                match target {
                    Some(t) => lane.block = *t,
                    None => return LaneStop::NeverArrives,
                }
            }
        }
    }
}

/// Values of the thread-index witnesses under the replay's launch shape
/// (block `[lanes,1,1]`, grid `[1,1,1]`), by name — verified against
/// cuda-device's formulas at the pinned rev. Every formula below reduces to
/// a function of the in-block thread index when the grid is 1 and the block
/// is one-dimensional; a name not listed evaluates to unknown, never to a
/// guess (an unlisted witness with the wrong value could fabricate a
/// confirmation — `threadIdx_y` is 0 under this launch, not the lane id).
fn thread_index_value(display: &str, idx: u32) -> Option<u128> {
    let last = display.rsplit("::").next().unwrap_or(display);
    match last {
        // blockIdx.x * blockDim.x + threadIdx.x = idx; 2D columns and the
        // flattened 2D indices reduce the same way with row 0.
        "threadIdx_x" | "index_1d" | "index_1d_u32" | "index_2d" | "index_2d_runtime"
        | "index_2d_col" => Some(u128::from(idx)),
        // The y/z axes of a one-dimensional block are 0 — as is the 2D row.
        "threadIdx_y" | "threadIdx_z" | "index_2d_row" => Some(0),
        "lane_id" => Some(u128::from(idx % LANES)),
        // blockIdx.x * warps_per_block + threadIdx.x / 32 = idx / 32.
        "warp_index" => Some(u128::from(idx / LANES)),
        _ => None,
    }
}

/// Values of the divergent environment reads that are exact under the
/// replay's launch shape: `warp_id` is the warp of the thread index and
/// `live_lanes_1d` counts the warp's launched lanes. The per-lane
/// registers (`lanemask_*`) and the path-dependent `active_mask` stay
/// unknown — their 32-bit mask values would flow into evaluation that is
/// not width-typed (integer `!` is modeled boolean-only), and a wrong
/// value here could fabricate a confirmation.
fn lane_env_value(display: &str, idx: u32, lanes: u32) -> Option<u128> {
    if display.contains("warp_id") {
        Some(u128::from(idx / LANES))
    } else if display.contains("live_lanes_1d") {
        Some(u128::from((lanes - (idx / LANES) * LANES).min(LANES)))
    } else {
        None
    }
}

/// Values of the block-uniform built-ins under the replay's launch shape
/// (block `[lanes,1,1]`, grid `[1,1,1]`).
fn block_uniform_value(display: &str, lanes: u32) -> Option<u128> {
    if display.contains("blockDim_x") {
        Some(u128::from(lanes))
    } else if display.contains("blockDim") || display.contains("gridDim") {
        Some(1)
    } else if display.contains("blockIdx") {
        Some(0)
    } else {
        None
    }
}

fn lanes_of(mask: u128, lanes: u32) -> Vec<u8> {
    (0..lanes as u8).filter(|l| mask & (1 << l) != 0).collect()
}

fn build_replay(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
    arrived: u128,
    never: u128,
    lanes: u32,
) -> Option<Replay> {
    let site_span = f.blocks[site_block].term.span;
    let site_display = match &f.blocks[site_block].term.kind {
        TermKind::Call { callee, .. } => callee.display.clone(),
        _ => return None,
    };
    let n_arrived = arrived.count_ones();
    let n_never = never.count_ones();

    let mut steps = vec![ReplayStep {
        statement: format!(
            "lanes evaluate the guarding branch — {n_arrived} continue toward \
             `{site_display}`, {n_never} never will"
        ),
        span: Some(cause_span),
        lane_changes: Vec::new(),
        barrier: None,
        warp_op: None,
    }];

    let (verdict_kind, verdict_message) = match site {
        SiteKind::Barrier => {
            steps.push(ReplayStep {
                statement: format!(
                    "{site_display}() — {n_arrived} of {lanes} lanes arrive and wait"
                ),
                span: Some(site_span),
                lane_changes: lanes_of(arrived, lanes)
                    .into_iter()
                    .map(|l| (l, LaneState::Waiting))
                    .collect(),
                barrier: Some((n_arrived, lanes)),
                warp_op: None,
            });
            steps.push(ReplayStep {
                statement: format!(
                    "the other {n_never} lanes exit or move past reconvergence without \
                     ever reaching the barrier"
                ),
                span: Some(cause_span),
                lane_changes: lanes_of(never, lanes)
                    .into_iter()
                    .map(|l| (l, LaneState::Exited))
                    .collect(),
                barrier: None,
                warp_op: None,
            });
            (
                VerdictKind::UndefinedBehavior,
                format!(
                    "{n_arrived} of {lanes} lanes wait at `{site_display}()` while {n_never} \
                     never arrive; the barrier cannot be satisfied — undefined behavior \
                     on hardware, usually a permanent hang"
                ),
            )
        }
        SiteKind::Collective { mask } => {
            // Collectives are replayed under one warp only (the entry gate
            // guarantees it), so the lane set fits the 32-bit mask domain.
            let arrived = u32::try_from(arrived).ok()?;
            // Without a known constant mask there is nothing to check the
            // arrivals against — claiming a mismatch would overreach.
            let mask = u32::try_from(mask?).ok()?;
            let named_absent = mask & !arrived;
            if named_absent == 0 {
                // The mask names only lanes that actually arrive: the
                // guarded partial-warp idiom. No witness.
                return None;
            }
            steps.push(ReplayStep {
                statement: format!(
                    "{site_display}() — active lanes {arrived:#010x}, mask {mask:#010x}"
                ),
                span: Some(site_span),
                lane_changes: Vec::new(),
                barrier: None,
                warp_op: Some((site_display.clone(), mask, arrived)),
            });
            steps.push(ReplayStep {
                statement: format!(
                    "{} lane(s) named by the mask never reach the call",
                    named_absent.count_ones()
                ),
                span: Some(cause_span),
                lane_changes: lanes_of(never, lanes)
                    .into_iter()
                    .map(|l| (l, LaneState::Exited))
                    .collect(),
                barrier: None,
                warp_op: None,
            });
            (
                VerdictKind::UndefinedBehavior,
                format!(
                    "`{site_display}()` executes with active lanes {arrived:#010x} but its \
                     mask {mask:#010x} names {} lane(s) that never arrive — undefined \
                     behavior on hardware, usually a kernel that never finishes",
                    named_absent.count_ones()
                ),
            )
        }
    };

    let verdict_step = steps.len() - 1;
    Some(Replay {
        block: [lanes, 1, 1],
        grid: [1, 1, 1],
        steps,
        verdict_kind,
        verdict_message,
        verdict_step,
        arrived,
        never_arrives: never,
    })
}

/// The pure-ASCII warp diagram for text diagnostics and SARIF (§7): lane
/// states at the failure point, eight lanes per group, one row per warp.
#[must_use]
pub fn ascii_warp_diagram(
    arrived: u128,
    never: u128,
    lanes: u32,
    arrived_glyph: char,
) -> Vec<String> {
    let mut out = Vec::new();
    for warp in 0..lanes.div_ceil(LANES) {
        let mut row = String::new();
        for offset in 0..LANES {
            let lane = warp * LANES + offset;
            if offset > 0 && offset % 8 == 0 {
                row.push(' ');
            }
            row.push(if arrived & (1 << lane) != 0 {
                arrived_glyph
            } else if never & (1 << lane) != 0 {
                '.'
            } else {
                '?'
            });
        }
        out.push(format!(
            "lanes {}..{} at the failure point: {row}",
            warp * LANES,
            warp * LANES + LANES - 1
        ));
    }
    out.push(format!(
        "({arrived_glyph} = reaches the call, . = never arrives)"
    ));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagram_groups_lanes_by_eight() {
        let lines = super::ascii_warp_diagram(0x5555_5555, 0xaaaa_aaaa, 32, 'W');
        assert_eq!(
            lines[0],
            "lanes 0..31 at the failure point: W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W."
        );
        assert!(lines[1].contains("W = reaches the call"));
    }

    #[test]
    fn diagram_prints_one_row_per_warp() {
        // Warp 0 arrives whole, warp 1 never does: the multi-warp shape.
        let lines = super::ascii_warp_diagram(0xffff_ffff, 0xffff_ffff_0000_0000, 64, 'W');
        assert_eq!(
            lines[0],
            "lanes 0..31 at the failure point: WWWWWWWW WWWWWWWW WWWWWWWW WWWWWWWW"
        );
        assert_eq!(
            lines[1],
            "lanes 32..63 at the failure point: ........ ........ ........ ........"
        );
        assert!(lines[2].contains("W = reaches the call"));
    }
}
