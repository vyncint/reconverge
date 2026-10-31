//! Warp-accurate replay of divergence findings — never a general-purpose
//! kernel runtime.
//!
//! Given a function model (the same one the uniformity engine analyzes), a
//! finding site, and a launch shape, the interpreter runs each thread of the
//! block — one warp, or the block a launch contract declares — through the
//! kernel-subset semantics the adapter captured
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
#![warn(missing_docs)]

use reconverge_artifacts::plural;
use reconverge_artifacts::witness::{LaneState, VerdictKind};
use reconverge_core::analysis::Summaries;
use reconverge_core::dialect::CallKind;
use reconverge_core::graph::{Cfg, post_dominators};
use reconverge_core::model::{
    BinOp, BlockId, Callee, Eval, FnModel, IntType, Operand, SpanRef, TermKind, UnOp,
};

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
    /// A block-wide barrier.
    Barrier,
    /// A warp collective with its constant participation mask, when known.
    Collective {
        /// The constant participation mask, when the call carried one.
        mask: Option<u64>,
    },
}

/// One timeline step of a successful replay. Spans are the model's opaque
/// references; the driver maps them to real source spans.
#[derive(Debug, Clone)]
pub struct ReplayStep {
    /// What happens at this step, in prose.
    pub statement: String,
    /// Where it happens, when known.
    pub span: Option<SpanRef>,
    /// Lanes whose state changed at this step, delta-encoded.
    pub lane_changes: Vec<(u8, LaneState)>,
    /// (arrived, expected) for barrier steps.
    pub barrier: Option<(u32, u32)>,
    /// (op, mask, active) for collective steps.
    pub warp_op: Option<(String, u32, u32)>,
}

/// A concrete replay of one finding: launch shape, timeline, verdict.
#[derive(Debug, Clone)]
pub struct Replay {
    /// Block dimensions replayed.
    pub block: [u32; 3],
    /// Grid dimensions replayed.
    pub grid: [u32; 3],
    /// The timeline.
    pub steps: Vec<ReplayStep>,
    /// How it ended.
    pub verdict_kind: VerdictKind,
    /// The verdict in calibrated prose.
    pub verdict_message: String,
    /// The step the verdict was reached at.
    pub verdict_step: usize,
    /// Bitmask of lanes that arrived at the site (bit per lane; the block
    /// may be more than one warp, so this is wider than a warp mask).
    pub arrived: u128,
    /// Bitmask of lanes that can never arrive.
    pub never_arrives: u128,
}

impl Replay {
    /// Threads replayed: the product of the block's dimensions. Lane `n`
    /// of the bitmasks is the thread at linear index `n`, x fastest — so a
    /// `(16, 2, 1)` block is one warp of 32 lanes, not 16.
    #[must_use]
    pub fn threads(&self) -> u32 {
        self.block.iter().product()
    }
}

/// What a replay may assume beyond the function model: the launch it runs
/// under, and what the crate's other functions may do.
#[derive(Debug, Clone, Copy)]
pub struct ReplayEnv<'a> {
    /// The block to replay, `(x, y, z)`. Its thread count must be a whole
    /// number of warps, at most 128; lane `n` is the thread at linear index
    /// `n`, x fastest, which is how the hardware forms warps.
    pub block: [u32; 3],
    /// The crate's interprocedural summaries, indexed like the model set
    /// the function's [`Callee::local_fn`] ids point into. A lane that
    /// reaches a call into a local function the summaries say may execute a
    /// barrier or a collective ends the replay — the callee was not inlined,
    /// so the interpreter cannot follow it. Without summaries every local
    /// callee is assumed to.
    pub summaries: Option<&'a Summaries>,
}

impl ReplayEnv<'_> {
    /// One warp, `(32, 1, 1)`: the launch an undeclared kernel is replayed
    /// under.
    pub const ONE_WARP: ReplayEnv<'static> = ReplayEnv {
        block: [LANES, 1, 1],
        summaries: None,
    };

    /// A replay under `block`, with no summaries.
    #[must_use]
    pub fn block(block: [u32; 3]) -> ReplayEnv<'static> {
        ReplayEnv {
            block,
            summaries: None,
        }
    }
}

/// Why a replay produced no witness.
///
/// The distinction matters to a reader and to anything consuming
/// `findings.v1`: "the replay checked and agreed" and "the replay could
/// not tell" are opposite results that otherwise look identical, because
/// both simply leave the finding at `warning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoWitness {
    /// No lane reaches the site under this launch shape, so there is
    /// nothing to confirm. Close to knowledge: under the declared shape
    /// this code does not run.
    UnreachableUnderLaunch,
    /// Every lane reaches the site — nothing diverges here.
    Uniform,
    /// A collective whose mask names exactly the lanes that arrive: the
    /// guarded partial-warp idiom, which is correct code. The replay
    /// verified it rather than failing to evaluate it.
    MaskMatchesArrivals {
        /// The mask the call named.
        mask: u32,
        /// The lanes that arrived.
        arrived: u32,
    },
    /// The replay could not determine what happens. An absence of
    /// knowledge, not a result.
    Indeterminate,
    /// The block is not one the replay models: not a whole number of warps,
    /// wider than 128 threads, or a collective site beyond one warp. Nothing
    /// was run, so nothing is known.
    UnsupportedLaunch,
    /// A lane reached a call into a local function that may execute a
    /// barrier or a collective, and the call was not inlined (recursion, or
    /// deeper than the inliner goes). Stepping over it would be stepping
    /// over a synchronization point.
    UnfollowedCall {
        /// The block whose terminator is that call.
        block: BlockId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneStop {
    Arrived,
    NeverArrives,
    /// Parked at a barrier, which synchronizes the whole block.
    AtBarrier(BlockId),
    /// Parked at a warp collective, which synchronizes only its own warp.
    /// The distinction is the whole of per-warp convergence: at one warp
    /// the two are the same thing, and beyond one warp they are not.
    AtCollective(BlockId),
    Bailed,
    /// Reached a local call that may synchronize and cannot be followed.
    Unfollowed(BlockId),
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
/// single-warp launch (block `[32,1,1]`, grid `[1,1,1]`), with no
/// interprocedural summaries ([`ReplayEnv::ONE_WARP`]).
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
    replay_outcome(f, site_block, site, cause_span).ok()
}

/// As [`replay_hang`], reporting *why* when there is no witness.
///
/// # Errors
/// Returns the [`NoWitness`] reason the replay produced no witness.
pub fn replay_outcome(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
) -> Result<Replay, NoWitness> {
    replay_outcome_at(f, site_block, site, cause_span, &ReplayEnv::ONE_WARP)
}

/// Like [`replay_hang`], under the block `env` names (grid `[1,1,1]`) —
/// the shape a kernel's launch contract declares.
///
/// The block's thread count must be a whole number of warps, at most 128.
/// Any shape of that size replays: lane `n` is the thread at linear index
/// `n`, x fastest, so `threadIdx_y()` under `(16, 2, 1)` is 1 on lanes
/// 16..31. Replaying a declared kernel at `(32, 1, 1)` instead put
/// `threadIdx.x` at 16..31 on a block whose contract says it stops at 15,
/// and reported the hang that shape has (#190).
///
/// A collective on a lane's path is modeled per warp: a warp whose
/// still-running lanes are all at the same collective passes it whatever
/// the other warps are doing, and a configuration that would need warps to
/// interact is declined rather than approximated.
///
/// The *site* itself must still be a barrier beyond one warp. A mask
/// comparison is a per-warp question and the witness artifact records a
/// single warp's view, so promoting a collective site at a multi-warp
/// block is a further step — and the one where a modeling error would
/// fail invisibly, producing a witness that looks exactly like a correct
/// one. It wants a computed oracle behind it, not a reading of the
/// artifact it produced.
#[must_use]
pub fn replay_hang_at(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
    env: &ReplayEnv<'_>,
) -> Option<Replay> {
    replay_outcome_at(f, site_block, site, cause_span, env).ok()
}

/// Whether `block` is a launch the replay can run, for a site of this kind:
/// a whole number of warps, at most 128 threads, and — for a collective
/// site — exactly one warp.
#[must_use]
pub fn replayable(block: [u32; 3], site: SiteKind) -> bool {
    let Some(threads) = block.iter().try_fold(1u32, |acc, &d| acc.checked_mul(d)) else {
        return false;
    };
    (LANES..=128).contains(&threads)
        && threads.is_multiple_of(LANES)
        && (threads == LANES || matches!(site, SiteKind::Barrier))
}

/// As [`replay_hang_at`], reporting *why* when there is no witness.
///
/// # Errors
/// Returns the [`NoWitness`] reason the replay produced no witness.
pub fn replay_outcome_at(
    f: &FnModel,
    site_block: BlockId,
    site: SiteKind,
    cause_span: SpanRef,
    env: &ReplayEnv<'_>,
) -> Result<Replay, NoWitness> {
    // A collective on the *path* is modeled per warp below; a collective
    // as the *site* is not replayed beyond one warp — see the note above.
    if !replayable(env.block, site) {
        return Err(NoWitness::UnsupportedLaunch);
    }
    let lanes: u32 = env.block.iter().product();
    let cfg = Cfg::build(f);
    let can_reach = reaches(&cfg, site_block);
    if !can_reach[0] {
        // The entry cannot reach the site at all: no lane arrives, which
        // is the same thing a reader needs told as the launch-shaped case.
        return Err(NoWitness::UnreachableUnderLaunch);
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
    let unfollowable = unfollowable_reach(f, &cfg, env.summaries);
    let n_lanes = lanes;
    let ctx = ReplayCtx {
        f,
        cfg: &cfg,
        can_reach: &can_reach,
        unfollowable: &unfollowable,
        ipdom: &ipdom,
        site_block,
        site,
        block: env.block,
        summaries: env.summaries,
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
        if let Some(block) = lanes.iter().find_map(|l| match l.stop {
            Some(LaneStop::Unfollowed(b)) => Some(b),
            _ => None,
        }) {
            return Err(NoWitness::UnfollowedCall { block });
        }
        if lanes.iter().any(|l| l.stop == Some(LaneStop::Bailed)) {
            return Err(NoWitness::Indeterminate);
        }

        // Warp-scoped releases first, because they are independent of what
        // the other warps are doing. A collective waits for its own warp;
        // only a barrier reaches across the block.
        match release_collectives(&mut lanes, n_lanes) {
            WarpRelease::Released => {
                released_syncs += 1;
                if released_syncs > 64 {
                    return Err(NoWitness::Indeterminate);
                }
                continue;
            }
            // Warps would have to interact for this to resolve. Declining
            // is the point: an approximation here still produces a witness
            // artifact, with a lane diagram and a concrete launch, and it
            // looks exactly like a correct one.
            WarpRelease::Interacting => return Err(NoWitness::Indeterminate),
            WarpRelease::Settled => {}
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
            return Err(NoWitness::Indeterminate);
        }
        // Release: everyone passes the intervening barrier together.
        released_syncs += 1;
        if released_syncs > 64 {
            return Err(NoWitness::Indeterminate);
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
            _ => return Err(NoWitness::Indeterminate),
        }
    }
    // Not a witness, but not the same non-answer either: no lane reaching
    // the site means the construct does not run under this shape, while
    // every lane reaching it means nothing diverges here.
    if arrived == 0 {
        return Err(NoWitness::UnreachableUnderLaunch);
    }
    if never == 0 {
        return Err(NoWitness::Uniform);
    }

    build_replay(f, site_block, site, cause_span, arrived, never, env.block)
}

/// What one round of warp-scoped release achieved.
enum WarpRelease {
    /// At least one warp passed its collective; run again.
    Released,
    /// No warp is waiting on a collective any more.
    Settled,
    /// A warp cannot resolve its collective without another warp moving
    /// first, which the model does not describe.
    Interacting,
}

/// Release each warp that has arrived at a collective together.
///
/// A collective synchronizes the lanes of one warp, so a warp whose
/// still-running lanes are all at the same collective passes it whatever
/// the other warps are doing. A warp that is split between a collective
/// and the site deadlocks within itself, exactly as the block-wide rule
/// says for barriers. Anything else — a warp split between a collective
/// and a barrier, or across two different collectives — would need warps
/// to interact, and is refused rather than guessed.
fn release_collectives(lanes: &mut [Lane], n_lanes: u32) -> WarpRelease {
    let mut released = false;
    for warp in 0..n_lanes.div_ceil(LANES) {
        let range = (warp * LANES) as usize..((warp + 1) * LANES).min(n_lanes) as usize;
        // Each lane parked at a collective, with the block it is parked at
        // — carried from the filter, so nothing downstream has to re-derive
        // it from the stop and reach for an arm that cannot happen.
        let at_collective: Vec<(usize, BlockId)> = range
            .clone()
            .filter_map(|i| match lanes[i].stop {
                Some(LaneStop::AtCollective(b)) => Some((i, b)),
                _ => None,
            })
            .collect();
        let Some(&(_, block)) = at_collective.first() else {
            continue;
        };
        let same_block = at_collective.iter().all(|&(_, b)| b == block);
        let others_running = range.clone().any(|i| {
            !matches!(
                lanes[i].stop,
                Some(LaneStop::Arrived | LaneStop::NeverArrives | LaneStop::AtCollective(_))
            )
        });
        let others_arrived = range
            .clone()
            .any(|i| lanes[i].stop == Some(LaneStop::Arrived));

        if same_block && !others_running && !others_arrived {
            for &(i, _) in &at_collective {
                lanes[i].resume_past_terminator = true;
                lanes[i].stop = Some(LaneStop::AtBarrier(usize::MAX)); // resume marker
            }
            released = true;
        } else if same_block && !others_running && others_arrived {
            // The rest of this warp is holding the site forever, so the
            // lanes at the collective wait on them and never arrive.
            for &(i, _) in &at_collective {
                lanes[i].stop = Some(LaneStop::NeverArrives);
            }
            released = true;
        } else {
            return WarpRelease::Interacting;
        }
    }
    if released {
        WarpRelease::Released
    } else {
        WarpRelease::Settled
    }
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

/// Population count of `v` read as a `bits`-wide integer, or `None` when
/// `v` carries bits the source type cannot hold.
///
/// The store is an untyped `u128` and unchecked arithmetic wraps there
/// rather than at the operand's width, so a wider value means the real
/// one was lost: `0u32.wrapping_sub(1)` is `u32::MAX` in the program and
/// `u128::MAX` in the store, 32 set bits against 128. Truncating cannot
/// recover it either — division, shifts and comparisons have already run
/// on the wide value — so an out-of-range operand yields no answer, and
/// the lane that branches on it ends without a verdict.
fn popcount_within(v: u128, bits: u32) -> Option<u128> {
    let in_range = bits >= u128::BITS || v >> bits == 0;
    in_range.then(|| u128::from(v.count_ones()))
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
            // the exact in-range value or nothing. Evaluated at 128 bits on
            // purpose — wrapping at `bits` would hide exactly the overflow
            // this form exists to catch. (Underflow wraps to the top of the
            // 128-bit range and fails the same width test.)
            let value = binary(
                op,
                operand_value(store, a)?,
                operand_value(store, b)?,
                IntType::unsigned(u128::BITS),
            )?;
            (value >> bits == 0).then_some(value)
        }
        Eval::Binary(op, a, b, ty) => {
            binary(op, operand_value(store, a)?, operand_value(store, b)?, ty)
        }
    }
}

/// `a op b` on `ty`, as the program computes it.
///
/// Both operands must already be values of `ty` — zero-extended from its
/// width, as the store holds everything. One that is not was lost upstream
/// (some arithmetic wrapped at the store's width rather than the program's),
/// and the answer is unknown rather than a guess built on it. The shift
/// amount is the exception: Rust lets it have its own type.
fn binary(op: BinOp, a: u128, b: u128, ty: IntType) -> Option<u128> {
    let bits = ty.bits;
    if !(1..=u128::BITS).contains(&bits) {
        return None;
    }
    let mask = width_mask(bits);
    let shifting = matches!(op, BinOp::Shl | BinOp::Shr);
    if a & !mask != 0 || (!shifting && b & !mask != 0) {
        return None;
    }
    // Two's complement read of a `bits`-wide pattern.
    let signed = |v: u128| -> i128 {
        let shift = u128::BITS - bits;
        ((v << shift) as i128) >> shift
    };
    let shift_amount = || u32::try_from(b).ok().filter(|&s| s < bits);
    let value = match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Eq => return Some(u128::from(a == b)),
        BinOp::Ne => return Some(u128::from(a != b)),
        // Past the width the program has panicked (debug) or masked the
        // amount (release); neither is a value to carry on with.
        BinOp::Shl => a << shift_amount()?,
        BinOp::Shr if ty.signed => (signed(a) >> shift_amount()?) as u128,
        BinOp::Shr => a >> shift_amount()?,
        BinOp::Div | BinOp::Rem | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if ty.signed => {
            let (a, b) = (signed(a), signed(b));
            // `MIN / -1` overflows the type, and the program panics there.
            if matches!(op, BinOp::Div | BinOp::Rem) && b == -1 && a == signed(1 << (bits - 1)) {
                return None;
            }
            match op {
                BinOp::Div => a.checked_div(b)? as u128,
                BinOp::Rem => a.checked_rem(b)? as u128,
                BinOp::Lt => return Some(u128::from(a < b)),
                BinOp::Le => return Some(u128::from(a <= b)),
                BinOp::Gt => return Some(u128::from(a > b)),
                _ => return Some(u128::from(a >= b)),
            }
        }
        BinOp::Div => a.checked_div(b)?,
        BinOp::Rem => a.checked_rem(b)?,
        BinOp::Lt => return Some(u128::from(a < b)),
        BinOp::Le => return Some(u128::from(a <= b)),
        BinOp::Gt => return Some(u128::from(a > b)),
        BinOp::Ge => return Some(u128::from(a >= b)),
    };
    Some(value & mask)
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
/// A call into a local function counts as a synchronization point when it
/// may reach one (see [`call_may_sync`]): stepping over it whole
/// is exactly what this function must not do to a barrier.
///
/// Returns the join block, or the stop that ends the lane: no witness, the
/// static result stands.
fn skip_unknowable_diamond(
    ctx: &ReplayCtx<'_>,
    branch: BlockId,
    store: &mut [Option<u128>],
) -> Result<BlockId, LaneStop> {
    let (f, cfg, ipdom, site_block) = (ctx.f, ctx.cfg, ctx.ipdom, ctx.site_block);
    let join = ipdom[branch].ok_or(LaneStop::Bailed)?;

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
            return Err(LaneStop::Bailed); // arrival genuinely depends on the unknown
        }
        // A region block whose post-dominator chain is the virtual exit
        // either escapes without the join (contradiction) or cannot reach
        // any exit at all (an infinite loop): bail either way.
        ipdom[b].ok_or(LaneStop::Bailed)?;
        match &f.blocks[b].term.kind {
            TermKind::Call { callee, .. }
                if matches!(
                    callee.kind,
                    CallKind::Barrier | CallKind::WarpCollective { .. }
                ) =>
            {
                return Err(LaneStop::Bailed);
            }
            TermKind::Call { callee, .. } if ctx.call_may_sync(callee) => {
                return Err(LaneStop::Unfollowed(b));
            }
            TermKind::Return | TermKind::Halt | TermKind::Jump { .. } => {
                return Err(LaneStop::Bailed);
            }
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
        return Err(LaneStop::Bailed); // a cycle: some world may never leave it
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
    Ok(join)
}

/// Immutable per-replay context shared by every lane.
struct ReplayCtx<'a> {
    f: &'a FnModel,
    cfg: &'a Cfg,
    can_reach: &'a [bool],
    /// Per block, an unfollowable call reachable from it.
    unfollowable: &'a [Option<BlockId>],
    ipdom: &'a [Option<BlockId>],
    site_block: BlockId,
    site: SiteKind,
    /// The replayed block, `(x, y, z)`; its thread count is a multiple of
    /// 32, at most 128.
    block: [u32; 3],
    summaries: Option<&'a Summaries>,
}

/// Can this call reach a barrier or a collective that the replay would not
/// see? A call into a local function the inliner did not splice in —
/// recursion, or past its depth — is stepped over as one statement, and if
/// the callee synchronizes, stepping over it is wrong: lanes that wait there
/// were reported as passing (#191). The summary bits answer; with none,
/// every local callee is assumed to.
fn call_may_sync(callee: &Callee, summaries: Option<&Summaries>) -> bool {
    let Some(id) = callee.local_fn else {
        return false;
    };
    // A barrier or collective is the construct itself, modeled where it is
    // met; only an ordinary call hides one.
    if matches!(
        callee.kind,
        CallKind::Barrier | CallKind::WarpCollective { .. }
    ) {
        return false;
    }
    summaries.is_none_or(|s| {
        s.may_contain_barrier.get(id).copied().unwrap_or(true)
            || s.may_contain_warp_op.get(id).copied().unwrap_or(true)
    })
}

/// Per block: a call the replay cannot follow (see [`call_may_sync`]) that
/// is reachable from it, if any.
///
/// A lane is "never arrives" when it enters code from which the site is
/// unreachable — but a lane whose remaining path runs into such a call may
/// wait at a barrier inside it, and that barrier can be the very one the
/// site names: `countdown(n)` recursing to the `sync_threads()` its first
/// frame holds. Calling that lane absent is the guess the witness exists
/// not to make.
fn unfollowable_reach(
    f: &FnModel,
    cfg: &Cfg,
    summaries: Option<&Summaries>,
) -> Vec<Option<BlockId>> {
    let mut reach = vec![None; cfg.len()];
    for (b, block) in f.blocks.iter().enumerate() {
        let TermKind::Call { callee, .. } = &block.term.kind else {
            continue;
        };
        if !call_may_sync(callee, summaries) || reach[b].is_some() {
            continue;
        }
        let mut stack = vec![b];
        reach[b] = Some(b);
        while let Some(x) = stack.pop() {
            for &p in &cfg.preds[x] {
                if reach[p].is_none() {
                    reach[p] = Some(b);
                    stack.push(p);
                }
            }
        }
    }
    reach
}

impl ReplayCtx<'_> {
    fn call_may_sync(&self, callee: &Callee) -> bool {
        call_may_sync(callee, self.summaries)
    }

    /// Where the thread at linear index `lane` sits in the block: x
    /// fastest, then y, then z — the order warps are formed in.
    fn thread(&self, lane: u32) -> Thread {
        let [x, y, _] = self.block;
        Thread {
            linear: lane,
            x: lane % x,
            y: (lane / x) % y,
            z: lane / (x * y),
        }
    }
}

/// One replayed thread's coordinates.
#[derive(Debug, Clone, Copy)]
struct Thread {
    linear: u32,
    x: u32,
    y: u32,
    z: u32,
}

fn run_lane(ctx: &ReplayCtx<'_>, lane: &mut Lane, lane_id: u32) -> LaneStop {
    loop {
        lane.steps += 1;
        if lane.steps > STEP_BUDGET {
            return LaneStop::Bailed;
        }
        if !ctx.can_reach[lane.block] {
            return match ctx.unfollowable[lane.block] {
                Some(call) => LaneStop::Unfollowed(call),
                None => LaneStop::NeverArrives,
            };
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
                    None => match skip_unknowable_diamond(ctx, lane.block, &mut lane.store) {
                        Ok(join) => Some(join),
                        Err(stop) => return stop,
                    },
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
                let thread = ctx.thread(lane_id);
                let result: Option<u128> = if resuming {
                    None // the released barrier's unit result
                } else if ctx.call_may_sync(callee) {
                    return LaneStop::Unfollowed(lane.block);
                } else {
                    match callee.kind {
                        CallKind::Barrier => return LaneStop::AtBarrier(lane.block),
                        CallKind::WarpCollective { .. } => {
                            // A non-site collective synchronizes its own
                            // warp. Parking it separately from a barrier is
                            // what lets several warps reach collectives
                            // independently.
                            let _ = ctx.site;
                            return LaneStop::AtCollective(lane.block);
                        }
                        CallKind::ThreadIndexWitness => {
                            thread_index_value(&callee.display, thread, ctx.block)
                        }
                        CallKind::ThreadIndexX { divisor } => {
                            thread.x.checked_div(divisor).map(u128::from)
                        }
                        CallKind::BlockUniform => block_uniform_value(&callee.display, ctx.block),
                        CallKind::DivergentEnvRead => {
                            lane_env_value(&callee.display, thread, ctx.block)
                        }
                        CallKind::WitnessRead => arg_operands
                            .first()
                            .copied()
                            .flatten()
                            .and_then(|op| operand_value(&lane.store, op)),
                        CallKind::CountOnes { bits } => arg_operands
                            .first()
                            .copied()
                            .flatten()
                            .and_then(|op| operand_value(&lane.store, op))
                            .and_then(|v| popcount_within(v, bits)),
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
/// (the replayed block, grid `[1,1,1]`), by name — verified against
/// cuda-device's formulas at the pinned rev. With one block every
/// `blockIdx` is 0, so each formula reduces to the thread's own
/// coordinates; a name not listed evaluates to unknown, never to a guess
/// (an unlisted witness with the wrong value could fabricate a
/// confirmation).
fn thread_index_value(display: &str, t: Thread, block: [u32; 3]) -> Option<u128> {
    let last = display.rsplit("::").next().unwrap_or(display);
    let one_dimensional = block[1] == 1 && block[2] == 1;
    match last {
        // blockIdx.x * blockDim.x + threadIdx.x = x.
        "threadIdx_x" | "index_1d" | "index_1d_u32" | "index_2d_col" => Some(u128::from(t.x)),
        "threadIdx_y" | "index_2d_row" => Some(u128::from(t.y)),
        "threadIdx_z" => Some(u128::from(t.z)),
        // `row * stride + col`, and the packed `(row, col)` of the runtime
        // form: the stride is a const generic or a slice's width, neither
        // of which the call names, so only row 0 is known — it is `col`.
        "index_2d" | "index_2d_runtime" => (t.y == 0).then_some(u128::from(t.x)),
        "lane_id" => Some(u128::from(t.linear % LANES)),
        // blockIdx.x * warps_per_block + threadIdx.x / 32, and only in a
        // 1-D launch; upstream returns `None` in any other.
        "warp_index" => one_dimensional.then_some(u128::from(t.x / LANES)),
        _ => None,
    }
}

/// Values of the divergent environment reads that are exact under the
/// replay's launch shape: `warp_id` is the warp of the thread index,
/// `live_lanes_1d` counts the warp's launched lanes, and the five
/// `lanemask_*` registers are closed forms of the lane's own ordinal.
///
/// The positional masks are pure functions of where the lane sits in its
/// warp — `lanemask_lt` is every lane below it, and so on — so they do not
/// depend on which lanes are still running, and the replay knows the
/// ordinal already. They were withheld while integer `!` was modeled
/// boolean-only and casts were the identity, because a 32-bit mask flowing
/// into that arithmetic could fabricate a confirmation; evaluation is
/// width-typed now, so the reason has gone.
///
/// `active_mask` stays unknown, and not for the same reason: its value is
/// the set of lanes still live at that point, which the replay tracks but
/// which changes as lanes diverge. That is a path-dependent question, not
/// a positional one.
fn lane_env_value(display: &str, t: Thread, block: [u32; 3]) -> Option<u128> {
    // The lane's ordinal within its own warp, which is what every
    // positional register is expressed in.
    let lane = t.linear % LANES;
    let below = (1u128 << lane) - 1;
    let up_to = (1u128 << (lane + 1)) - 1;
    let warp_wide = u128::from(u32::MAX);
    // Both are written over `threadIdx_x()` upstream, not over the linear
    // index: `warp_id()` is `threadIdx_x() / 32`, whatever the block's
    // other axes are.
    if display.contains("warp_id") {
        Some(u128::from(t.x / LANES))
    } else if display.contains("live_lanes_1d") {
        Some(u128::from((block[0] - (t.x / LANES) * LANES).min(LANES)))
    } else if display.contains("lanemask_lt") {
        Some(below)
    } else if display.contains("lanemask_le") {
        Some(up_to)
    } else if display.contains("lanemask_eq") {
        Some(1u128 << lane)
    } else if display.contains("lanemask_gt") {
        Some(!up_to & warp_wide)
    } else if display.contains("lanemask_ge") {
        Some(!below & warp_wide)
    } else {
        None
    }
}

/// Values of the block-uniform built-ins under the replay's launch shape
/// (the replayed block, grid `[1,1,1]`).
fn block_uniform_value(display: &str, block: [u32; 3]) -> Option<u128> {
    if display.contains("blockDim_x") {
        Some(u128::from(block[0]))
    } else if display.contains("blockDim_y") {
        Some(u128::from(block[1]))
    } else if display.contains("blockDim_z") {
        Some(u128::from(block[2]))
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
    block: [u32; 3],
) -> Result<Replay, NoWitness> {
    let lanes: u32 = block.iter().product();
    let site_span = f.blocks[site_block].term.span;
    let site_display = match &f.blocks[site_block].term.kind {
        TermKind::Call { callee, .. } => callee.display.clone(),
        _ => return Err(NoWitness::Indeterminate),
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
            let arrived = u32::try_from(arrived).map_err(|_| NoWitness::Indeterminate)?;
            // Without a known constant mask there is nothing to check the
            // arrivals against — claiming a mismatch would overreach.
            let mask = mask.ok_or(NoWitness::Indeterminate)?;
            let mask = u32::try_from(mask).map_err(|_| NoWitness::Indeterminate)?;
            let named_absent = mask & !arrived;
            if named_absent == 0 {
                // The mask names only lanes that actually arrive: the
                // guarded partial-warp idiom, which is correct code. This
                // is a verified result, not a failure to evaluate, and the
                // caller is told which so it can say so.
                return Err(NoWitness::MaskMatchesArrivals { mask, arrived });
            }
            steps.push(ReplayStep {
                statement: format!(
                    "{site_display}() — active lanes {arrived:#010x}, mask {mask:#010x}"
                ),
                span: Some(site_span),
                // The deltas belong *at* the call, not one step later.
                // Deferring them left the lane strip reading `oooooooo …`
                // for all 32 lanes two rows above an `active 0x55555555`
                // saying sixteen — the panel a user opens precisely because
                // they do not yet believe the finding, arguing against it.
                //
                // A barrier additionally turns its arriving lanes `Waiting`
                // at the site, which is why that arm looked right while
                // deferring its `Exited` deltas identically. A collective
                // has no such state — it does not block the lanes that did
                // arrive — so `Exited` is what the absent lanes get, which
                // matches the text diagnostic's own legend (`.` = never
                // arrives) and what the driver already wrote one step on.
                lane_changes: lanes_of(never, lanes)
                    .into_iter()
                    .map(|l| (l, LaneState::Exited))
                    .collect(),
                barrier: None,
                warp_op: Some((site_display.clone(), mask, arrived)),
            });
            steps.push(ReplayStep {
                statement: format!(
                    "{} {} named by the mask never reach the call",
                    named_absent.count_ones(),
                    plural(named_absent.count_ones(), "lane", "lanes")
                ),
                span: Some(cause_span),
                // Pure narration: the departures are recorded above.
                lane_changes: Vec::new(),
                barrier: None,
                warp_op: None,
            });
            (
                VerdictKind::UndefinedBehavior,
                format!(
                    "`{site_display}()` executes with active lanes {arrived:#010x} but its \
                     mask {mask:#010x} names {} {} that never arrive — undefined \
                     behavior on hardware, usually a kernel that never finishes",
                    named_absent.count_ones(),
                    plural(named_absent.count_ones(), "lane", "lanes")
                ),
            )
        }
    };

    let verdict_step = steps.len() - 1;
    Ok(Replay {
        block,
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
