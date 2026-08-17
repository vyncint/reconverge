//! Triage state-machine tests (§9 layer 1): pure transitions over the
//! checked-in fixtures — no PTY, and no file is ever written (the state
//! only *requests* a write).

use std::path::{Path, PathBuf};

use reconverge_tui::triage::{KeyAction, Status, TriageData, TriageState, data};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// Two findings, no suppressions yet — the state a first triage run sees.
fn fresh() -> (TriageData, TriageState) {
    let (data, baseline) = data::load(
        &[fixture("findings/rc003-minimal.json")],
        &fixture("baseline/does-not-exist.json"),
    );
    assert_eq!(data.items.len(), 2, "fixture shape changed");
    let state = TriageState::new(baseline);
    (data, state)
}

#[test]
fn accepting_a_finding_requires_a_reason_and_records_it() {
    let (data, mut state) = fresh();
    assert!(state.suppression_of(&data, 0).is_none());

    assert!(state.update(KeyAction::BeginSuppress, &data));
    assert_eq!(state.editing.as_deref(), Some(""));

    // Committing an empty reason keeps the editor open and says why.
    assert!(state.update(KeyAction::ReasonCommit, &data));
    assert_eq!(state.status, Status::ReasonRequired);
    assert!(state.editing.is_some(), "still editing");
    assert!(!state.dirty, "nothing was recorded");

    for c in "ok: host owns it".chars() {
        state.update(KeyAction::ReasonChar(c), &data);
    }
    assert!(state.update(KeyAction::ReasonCommit, &data));
    assert!(state.editing.is_none());
    assert!(state.dirty);
    assert_eq!(state.suppression_of(&data, 0), Some("ok: host owns it"));
    assert_eq!(state.baseline.entries.len(), 1);

    // `s` on an already-accepted finding does nothing; `u` withdraws it.
    assert!(!state.update(KeyAction::BeginSuppress, &data));
    assert!(state.update(KeyAction::Unsuppress, &data));
    assert!(state.suppression_of(&data, 0).is_none());
    assert!(state.baseline.entries.is_empty());
}

#[test]
fn the_reason_editor_is_grapheme_aware() {
    let (data, mut state) = fresh();
    state.update(KeyAction::BeginSuppress, &data);
    // "phân kỳ" in NFD: the combining marks must not be deleted alone.
    for c in "pha\u{302}n k\u{79}\u{300}".chars() {
        state.update(KeyAction::ReasonChar(c), &data);
    }
    let before = state.editing.clone().unwrap();
    assert!(state.update(KeyAction::ReasonBackspace, &data));
    let after = state.editing.clone().unwrap();
    // `ỳ` is "y" plus a combining grave: one keystroke removes the whole
    // letter. Popping `char`s would have left a bare "y" behind.
    assert_eq!(after, "pha\u{302}n k");
    assert!(!after.ends_with('y'), "combining mark orphaned: {after:?}");
    assert!(before.starts_with(&after));

    // Backspacing an empty buffer is harmless.
    for _ in 0..20 {
        state.update(KeyAction::ReasonBackspace, &data);
    }
    assert_eq!(state.editing.as_deref(), Some(""));
}

#[test]
fn cancelling_leaves_the_baseline_untouched() {
    let (data, mut state) = fresh();
    state.update(KeyAction::BeginSuppress, &data);
    for c in "half-typed".chars() {
        state.update(KeyAction::ReasonChar(c), &data);
    }
    assert!(state.update(KeyAction::ReasonCancel, &data));
    assert!(state.editing.is_none());
    assert!(!state.dirty);
    assert!(state.baseline.entries.is_empty());
}

#[test]
fn navigation_wraps_and_is_disabled_mid_reason() {
    let (data, mut state) = fresh();
    assert!(state.update(KeyAction::Next, &data));
    assert_eq!(state.selected, 1);
    assert!(state.update(KeyAction::Next, &data));
    assert_eq!(state.selected, 0, "wraps");
    assert!(state.update(KeyAction::Prev, &data));
    assert_eq!(state.selected, 1, "wraps backwards");

    state.update(KeyAction::BeginSuppress, &data);
    let selected = state.selected;
    // While typing, j/k are letters, not motions — the mapping in the
    // binary sends ReasonChar; the state machine refuses motion either way.
    assert!(!state.update(KeyAction::Next, &data));
    assert_eq!(state.selected, selected);
}

#[test]
fn writing_is_requested_by_the_state_and_performed_by_the_caller() {
    let (data, mut state) = fresh();
    assert!(!state.take_write_request(), "nothing requested yet");

    state.update(KeyAction::BeginSuppress, &data);
    for c in "reviewed".chars() {
        state.update(KeyAction::ReasonChar(c), &data);
    }
    state.update(KeyAction::ReasonCommit, &data);
    assert!(state.dirty);

    assert!(state.update(KeyAction::RequestWrite, &data));
    assert!(state.take_write_request());
    assert!(!state.take_write_request(), "requests are taken once");

    state.record_write(Ok(()));
    assert!(!state.dirty, "written edits are no longer unsaved");
    assert_eq!(state.status, Status::Wrote(1));

    state.record_write(Err("read-only filesystem".into()));
    assert_eq!(
        state.status,
        Status::WriteFailed("read-only filesystem".into())
    );
}

#[test]
fn quitting_with_unsaved_edits_asks_once() {
    let (data, mut state) = fresh();
    assert!(state.request_quit(), "clean state quits immediately");

    state.update(KeyAction::BeginSuppress, &data);
    for c in "reviewed".chars() {
        state.update(KeyAction::ReasonChar(c), &data);
    }
    state.update(KeyAction::ReasonCommit, &data);

    assert!(!state.request_quit(), "first q asks");
    assert!(state.confirm_quit);
    assert!(state.request_quit(), "second q leaves");

    // Any deliberate action clears the prompt again.
    state.update(KeyAction::Next, &data);
    assert!(!state.confirm_quit);
    assert!(!state.request_quit());
}

#[test]
fn an_existing_baseline_is_loaded_and_shown_as_accepted() {
    let (data, baseline) = data::load(
        &[fixture("findings/rc003-minimal.json")],
        &fixture("baseline/minimal.json"),
    );
    let state = TriageState::new(baseline);
    let accepted: Vec<usize> = (0..data.items.len())
        .filter(|&i| state.suppression_of(&data, i).is_some())
        .collect();
    assert_eq!(accepted.len(), 1, "the fixture accepts exactly one finding");
    assert!(
        state
            .suppression_of(&data, accepted[0])
            .unwrap()
            .contains("demonstrate the &mut [T] hazard")
    );
    assert!(!state.dirty, "loading is not an edit");
}

#[test]
fn empty_data_never_transitions() {
    let data = TriageData::default();
    let mut state = TriageState::new(reconverge_artifacts::baseline::BaselineArtifact::empty());
    for action in [
        KeyAction::Next,
        KeyAction::Prev,
        KeyAction::BeginSuppress,
        KeyAction::Unsuppress,
        KeyAction::ReasonCommit,
    ] {
        assert!(!state.update(action.clone(), &data), "{action:?}");
    }
}
