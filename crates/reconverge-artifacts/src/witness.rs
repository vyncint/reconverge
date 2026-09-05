//! `witness.v1` — the witness artifact (`schemas/witness.v1.json`).
//!
//! A concrete replay of one finding under one launch configuration: one
//! warp, or the declared block when a `#[launch_contract]` names several
//! whole warps. Lane states are delta-encoded per step — only changes are
//! recorded — so long traces stay small. The witness debugger is a pure
//! reader of this. Additive-only within v1.

use serde::{Deserialize, Serialize};

use crate::findings::{SourceSpan, ToolInfo};
use crate::read::Artifact;

/// Top-level witness artifact: one replay of one finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitnessArtifact {
    /// Always [`crate::schema::WITNESS`].
    pub schema: String,
    pub tool: ToolInfo,
    #[serde(rename = "crate")]
    pub krate: String,
    /// User-facing kernel name.
    pub kernel: String,
    /// The finding this witness replays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding: Option<FindingRef>,
    /// Concrete thread configuration the search found.
    pub launch: Launch,
    /// Lanes in the replay: 32 for the ordinary one-warp replay, or the
    /// declared block when the contract names several whole warps (64, 96,
    /// 128). Blocks that are 2D, not whole warps, or wider than 128 stay at
    /// the one-warp replay.
    ///
    /// This said "always 32" — and the schema pinned it there — while the
    /// driver had been writing 64, 96 and 128 since 0.1.12. The artifacts
    /// that broke the published bound were exactly the gating ones: the
    /// whole-warp deadlocks promoted to `confirmed`.
    pub lanes: u8,
    pub initial_lane_states: Vec<LaneState>,
    pub steps: Vec<Step>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRef {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub grid: [u32; 3],
    pub block: [u32; 3],
    /// Index of the replayed warp within its block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaneState {
    Active,
    Waiting,
    Exited,
}

/// One event in the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub index: usize,
    /// What happens at this step, in prose: "sync_threads() — 16 of 32
    /// lanes arrive and wait".
    ///
    /// Documented as a MIR statement until 0.5.0, which no released driver
    /// has ever emitted — the replay is built from the site rather than by
    /// walking statements, and `reconverge_core::model::Stmt` drops the
    /// printable form at extraction. The fixtures showed MIR, so a
    /// front-end author who validated a MIR parser against them had it
    /// confirmed by the project's own API tests. If the MIR line is ever
    /// carried through the model it belongs in a second field beside this
    /// one, additively.
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
    /// Delta encoding: only lanes whose state changed at this step. Always
    /// present (possibly empty) — the schema requires it.
    pub lane_changes: Vec<LaneChange>,
    /// Present when this step is a barrier interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barrier: Option<BarrierEvent>,
    /// Present when this step is a warp collective.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp_op: Option<WarpOpEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneChange {
    pub lane: u8,
    pub state: LaneState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarrierEvent {
    pub arrived: u32,
    pub expected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarpOpEvent {
    /// e.g. `shuffle_sync`, `ballot_sync`.
    pub op: String,
    /// Participation mask named by the call, `0x` + 8 hex digits.
    pub mask: String,
    /// Lanes actually active at the call site, `0x` + 8 hex digits.
    pub active: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub kind: VerdictKind,
    /// Calibrated wording; hardware behavior is "usually", never "always".
    pub message: String,
    /// Step index where the verdict was reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerdictKind {
    Hang,
    UndefinedBehavior,
    Completed,
    NoWitness,
}

impl Artifact for WitnessArtifact {
    const SCHEMA: &'static str = crate::schema::WITNESS;

    fn declared_schema(&self) -> &str {
        &self.schema
    }
}

impl WitnessArtifact {
    /// Replay the delta-encoded timeline up to and including `step`,
    /// returning the lane states at that point. `step = None` returns the
    /// initial states.
    ///
    /// This is the one piece of witness semantics every reader needs; it
    /// lives here so the TUI and tests cannot drift apart on it.
    #[must_use]
    pub fn lane_states_at(&self, step: Option<usize>) -> Vec<LaneState> {
        let mut states = self.initial_lane_states.clone();
        if let Some(step) = step {
            for s in self.steps.iter().take_while(|s| s.index <= step) {
                for change in &s.lane_changes {
                    if let Some(slot) = states.get_mut(change.lane as usize) {
                        *slot = change.state;
                    }
                }
            }
        }
        states
    }

    /// The invariant the lane strip is drawn from: **at any step carrying a
    /// `warp_op`, the lanes still present at that step are exactly the set
    /// bits of `warp_op.active`.**
    ///
    /// Nothing stated this before 0.5.0, so the collective panel was
    /// self-contradictory rather than in breach of a written contract: the
    /// strip read `oooooooo …` for all 32 lanes, two rows above an
    /// `active 0x55555555` saying sixteen, four rows above the panel naming
    /// the sixteen that never arrive. The departures were attached to the
    /// step *after* the call, so the site step carried no deltas at all.
    ///
    /// Returns the index of the first step that breaks it, if any.
    #[must_use]
    pub fn first_collective_disagreeing_with_its_mask(&self) -> Option<usize> {
        for step in &self.steps {
            let Some(op) = &step.warp_op else { continue };
            let Some(active) = parse_lane_mask(&op.active) else {
                continue;
            };
            let states = self.lane_states_at(Some(step.index));
            for (lane, state) in states.iter().enumerate() {
                // A mask is 32 bits wide and names lanes within a warp, so
                // beyond the first warp there is nothing to compare against.
                if lane >= 32 {
                    break;
                }
                let named = active & (1u32 << lane) != 0;
                let present = *state == LaneState::Active;
                if named != present {
                    return Some(step.index);
                }
            }
        }
        None
    }
}

/// A `0x` + 8 hex digit lane mask, as the artifact spells it.
fn parse_lane_mask(text: &str) -> Option<u32> {
    u32::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::round_trip_fixtures;

    #[test]
    fn witness_fixtures_round_trip() {
        round_trip_fixtures("witness", |text| {
            let parsed: WitnessArtifact = serde_json::from_str(text)?;
            assert_eq!(parsed.schema, crate::schema::WITNESS);
            // Not `== 32`: the multi-warp replay writes 64, 96 or 128, and
            // pinning every fixture at one warp is what kept the multi-warp
            // fixture — the one that would have caught the schema being
            // unsatisfiable for gating artifacts — out of the directory.
            assert!(
                (32..=128).contains(&parsed.lanes) && parsed.lanes.is_multiple_of(32),
                "lanes must be whole warps up to 128, got {}",
                parsed.lanes
            );
            assert_eq!(parsed.initial_lane_states.len(), usize::from(parsed.lanes));
            serde_json::to_value(&parsed)
        });
    }

    /// Every shipped fixture obeys the invariant the collective panel is
    /// drawn from. The hand-written `rc002-partial-mask.json` put its lane
    /// deltas on the branch step and so passed all along; the driver put
    /// none on the site step, which is why the golden frame was coherent
    /// and the shipping artifact was not.
    #[test]
    fn witness_fixtures_agree_with_their_own_masks() {
        round_trip_fixtures("witness", |text| {
            let parsed: WitnessArtifact = serde_json::from_str(text)?;
            assert_eq!(
                parsed.first_collective_disagreeing_with_its_mask(),
                None,
                "a collective step's lane strip disagrees with its own \
                 `active` mask"
            );
            serde_json::to_value(&parsed)
        });
    }

    /// No fixture may be stamped a version the workspace does not produce.
    /// All three were `0.0.0`, which is the tell that no released driver
    /// wrote them — and the reason the MIR they showed went unquestioned.
    #[test]
    fn witness_fixtures_are_stamped_a_version_this_workspace_produces() {
        round_trip_fixtures("witness", |text| {
            let parsed: WitnessArtifact = serde_json::from_str(text)?;
            assert_eq!(
                parsed.tool.version,
                env!("CARGO_PKG_VERSION"),
                "fixture tool.version must be a version this workspace \
                 produces; re-record it from a real `check`"
            );
            serde_json::to_value(&parsed)
        });
    }

    #[test]
    fn a_collective_whose_strip_contradicts_its_mask_is_named() {
        let mut artifact = artifact_with_collective(vec![]);
        // Deltas deferred to the step after the call: the shipped shape.
        assert_eq!(
            artifact.first_collective_disagreeing_with_its_mask(),
            Some(1),
            "all 32 lanes present under a mask naming 16 must be caught"
        );

        // The same call with the departures at the site.
        artifact = artifact_with_collective(
            (0..32)
                .filter(|lane| lane % 2 == 1)
                .map(|lane| LaneChange {
                    lane,
                    state: LaneState::Exited,
                })
                .collect(),
        );
        assert_eq!(artifact.first_collective_disagreeing_with_its_mask(), None);
    }

    fn artifact_with_collective(lane_changes: Vec<LaneChange>) -> WitnessArtifact {
        WitnessArtifact {
            schema: crate::schema::WITNESS.to_string(),
            tool: ToolInfo::current(),
            krate: "k".into(),
            kernel: "rc002_divergent_collective".into(),
            finding: None,
            launch: Launch {
                grid: [1, 1, 1],
                block: [32, 1, 1],
                warp: Some(0),
            },
            lanes: 32,
            initial_lane_states: vec![LaneState::Active; 32],
            steps: vec![
                Step {
                    index: 0,
                    statement: "lanes evaluate the guarding branch".into(),
                    span: None,
                    lane_changes: vec![],
                    barrier: None,
                    warp_op: None,
                },
                Step {
                    index: 1,
                    statement: "ballot_sync()".into(),
                    span: None,
                    lane_changes,
                    barrier: None,
                    warp_op: Some(WarpOpEvent {
                        op: "ballot_sync".into(),
                        mask: "0xffffffff".into(),
                        active: "0x55555555".into(),
                    }),
                },
            ],
            verdict: Verdict {
                kind: VerdictKind::UndefinedBehavior,
                message: "m".into(),
                step: Some(1),
            },
        }
    }

    #[test]
    fn lane_state_replay_applies_deltas_in_order() {
        let mut artifact = WitnessArtifact {
            schema: crate::schema::WITNESS.to_string(),
            tool: ToolInfo::current(),
            krate: "k".into(),
            kernel: "k".into(),
            finding: None,
            launch: Launch {
                grid: [1, 1, 1],
                block: [32, 1, 1],
                warp: Some(0),
            },
            lanes: 32,
            initial_lane_states: vec![LaneState::Active; 32],
            steps: vec![
                Step {
                    index: 0,
                    statement: "s0".into(),
                    span: None,
                    lane_changes: vec![LaneChange {
                        lane: 1,
                        state: LaneState::Exited,
                    }],
                    barrier: None,
                    warp_op: None,
                },
                Step {
                    index: 1,
                    statement: "s1".into(),
                    span: None,
                    lane_changes: vec![LaneChange {
                        lane: 0,
                        state: LaneState::Waiting,
                    }],
                    barrier: None,
                    warp_op: None,
                },
            ],
            verdict: Verdict {
                kind: VerdictKind::Completed,
                message: "done".into(),
                step: Some(1),
            },
        };

        assert_eq!(artifact.lane_states_at(None)[1], LaneState::Active);
        assert_eq!(artifact.lane_states_at(Some(0))[1], LaneState::Exited);
        assert_eq!(artifact.lane_states_at(Some(0))[0], LaneState::Active);
        assert_eq!(artifact.lane_states_at(Some(1))[0], LaneState::Waiting);

        // Steps beyond the last recorded index change nothing.
        artifact.verdict.step = None;
        assert_eq!(artifact.lane_states_at(Some(99))[0], LaneState::Waiting);
    }
}
