//! Debugger state-machine tests (§9 layer 1): pure transitions on the
//! checked-in canonical fixtures — state is f(artifacts, key sequence),
//! so these run headless with no PTY.

use std::path::{Path, PathBuf};

use reconverge_artifacts::witness::LaneState;
use reconverge_tui::witness::state::divergence_position;
use reconverge_tui::witness::{KeyAction, WitnessData, WitnessState, data};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/witness")
        .join(rel)
}

fn canonical() -> WitnessData {
    let data = data::load(&[
        fixture("rc001-divergent-barrier.json"),
        fixture("rc002-partial-mask.json"),
    ]);
    assert!(data.errors.is_empty(), "{:?}", data.errors);
    data
}

#[test]
fn stepping_clamps_at_both_ends_of_the_timeline() {
    let data = canonical();
    let mut state = WitnessState::new(&data);
    assert_eq!(state.position, 0, "opens at the launch instant");
    assert!(!state.update(KeyAction::StepBack, &data), "already at 0");

    let steps = data.witnesses[0].steps.len();
    for expected in 1..=steps {
        assert!(state.update(KeyAction::StepForward, &data));
        assert_eq!(state.position, expected);
    }
    assert!(
        !state.update(KeyAction::StepForward, &data),
        "clamped at the end — wrapping would lie about time"
    );

    assert!(state.update(KeyAction::First, &data));
    assert_eq!(state.position, 0);
    assert!(state.update(KeyAction::Last, &data));
    assert_eq!(state.position, steps);
}

#[test]
fn jump_to_divergence_lands_on_the_first_warp_split() {
    let data = canonical();
    let mut state = WitnessState::new(&data);

    // rc001: the barrier step (index 1) is the first that changes lanes.
    // Three steps, not five: the fixture is recorded from a real `check`
    // since 0.5.0 rather than hand-written around MIR the driver has never
    // emitted.
    assert!(state.update(KeyAction::JumpDivergence, &data));
    assert_eq!(state.position, 2);
    let states = data.witnesses[0].lane_states_at(Some(1));
    assert_eq!(
        states.iter().filter(|s| **s == LaneState::Waiting).count(),
        16,
        "the divergence moment shows 16 lanes parked at the barrier"
    );

    // rc002 (witness 2): the switch (index 1) splits the warp.
    assert!(state.update(KeyAction::NextWitness, &data));
    assert_eq!(
        (state.witness, state.position),
        (1, 0),
        "switch resets time"
    );
    assert!(state.update(KeyAction::JumpDivergence, &data));
    assert_eq!(state.position, 2);
    assert_eq!(divergence_position(&data.witnesses[1]), Some(2));
}

#[test]
fn jump_to_verdict_lands_after_the_verdict_step() {
    let data = canonical();
    let mut state = WitnessState::new(&data);
    assert!(state.update(KeyAction::JumpVerdict, &data));
    // rc001's verdict.step is 2 (the last of 3 steps) -> position 3.
    assert_eq!(state.position, 3);
}

#[test]
fn witness_cycling_wraps_in_both_directions() {
    let data = canonical();
    let mut state = WitnessState::new(&data);
    assert!(state.update(KeyAction::PrevWitness, &data));
    assert_eq!(state.witness, 1, "N wraps backward");
    assert!(state.update(KeyAction::NextWitness, &data));
    assert_eq!(state.witness, 0);
}

#[test]
fn empty_data_never_transitions() {
    let data = WitnessData::default();
    let mut state = WitnessState::new(&data);
    for action in [
        KeyAction::StepForward,
        KeyAction::StepBack,
        KeyAction::Last,
        KeyAction::JumpDivergence,
        KeyAction::JumpVerdict,
        KeyAction::NextWitness,
    ] {
        assert!(!state.update(action, &data), "{action:?} on empty data");
    }
}
