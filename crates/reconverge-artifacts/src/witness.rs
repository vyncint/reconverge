//! `witness.v1` — the witness artifact (`schemas/witness.v1.json`).
//!
//! A concrete 32-lane replay of one finding under one launch
//! configuration. Lane states are delta-encoded per step — only changes
//! are recorded — so long traces stay small. The witness debugger is
//! a pure reader of this. Additive-only within v1.

use serde::{Deserialize, Serialize};

use crate::findings::{SourceSpan, ToolInfo};

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
    /// Always 32.
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
    /// The MIR statement or terminator executed at this step.
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

impl WitnessArtifact {
    /// Replay the delta-encoded timeline up to and including `step`,
    /// returning the 32 lane states at that point. `step = None` returns
    /// the initial states.
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
            assert_eq!(parsed.lanes, 32);
            assert_eq!(parsed.initial_lane_states.len(), 32);
            serde_json::to_value(&parsed)
        });
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
