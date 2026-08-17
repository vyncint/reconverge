//! Debugger state machine — pure transitions, no I/O.
//!
//! `position` walks the timeline: `0` is the launch instant (every lane in
//! its initial state, no event executed yet) and `k` is "after step
//! `k-1`". So a witness with N steps has N+1 positions, and stepping is
//! clamped at both ends — hitting a wall is visible, wrapping would lie
//! about time.

use reconverge_artifacts::witness::WitnessArtifact;

use super::data::WitnessData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessState {
    /// Index into `data.witnesses`.
    pub witness: usize,
    /// Timeline position: `0..=steps.len()`.
    pub position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    StepForward,
    StepBack,
    First,
    Last,
    /// Jump to the moment the warp first splits (the first step that
    /// changes any lane state).
    JumpDivergence,
    /// Jump to the verdict step.
    JumpVerdict,
    NextWitness,
    PrevWitness,
}

/// The position at which the warp first splits, if it ever does.
#[must_use]
pub fn divergence_position(witness: &WitnessArtifact) -> Option<usize> {
    witness
        .steps
        .iter()
        .position(|s| !s.lane_changes.is_empty())
        .map(|i| i + 1)
}

impl WitnessState {
    #[must_use]
    pub fn new(_data: &WitnessData) -> WitnessState {
        WitnessState {
            witness: 0,
            position: 0,
        }
    }

    /// Apply one action; returns true when anything changed (the caller
    /// redraws only then — event-driven by construction).
    pub fn update(&mut self, action: KeyAction, data: &WitnessData) -> bool {
        let Some(witness) = data.witnesses.get(self.witness) else {
            return false;
        };
        let last = witness.steps.len();
        let before = self.clone();
        match action {
            KeyAction::StepForward => self.position = (self.position + 1).min(last),
            KeyAction::StepBack => self.position = self.position.saturating_sub(1),
            KeyAction::First => self.position = 0,
            KeyAction::Last => self.position = last,
            KeyAction::JumpDivergence => {
                if let Some(position) = divergence_position(witness) {
                    self.position = position;
                }
            }
            KeyAction::JumpVerdict => {
                self.position = witness.verdict.step.map_or(last, |s| (s + 1).min(last));
            }
            KeyAction::NextWitness | KeyAction::PrevWitness => {
                let n = data.witnesses.len() as isize;
                let step: isize = if action == KeyAction::NextWitness {
                    1
                } else {
                    -1
                };
                self.witness = (self.witness as isize + step).rem_euclid(n) as usize;
                self.position = 0;
            }
        }
        *self != before
    }
}
