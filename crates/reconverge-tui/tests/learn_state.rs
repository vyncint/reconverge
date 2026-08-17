//! Learn-mode state-machine tests (§9 layer 1): pure transitions over the
//! embedded lessons — no PTY, no files.

use reconverge_tui::learn::{KeyAction, LearnState, Screen, lessons};

#[test]
fn list_selection_wraps_and_open_enters_the_lesson() {
    let all = lessons();
    let mut state = LearnState::new();
    assert_eq!(state.screen, Screen::List);

    assert!(state.update(KeyAction::Up, &all));
    assert_eq!(state.lesson, 3, "k wraps to the last lesson");
    assert!(state.update(KeyAction::Down, &all));
    assert_eq!(state.lesson, 0);

    assert!(state.update(KeyAction::Down, &all));
    assert!(state.update(KeyAction::Open, &all));
    assert_eq!(
        (state.screen, state.lesson, state.page),
        (Screen::Page, 1, 0)
    );
}

#[test]
fn page_turns_clamp_and_reset_the_replay_position() {
    let all = lessons();
    let mut state = LearnState::new();
    state.update(KeyAction::Open, &all);
    assert!(
        !state.update(KeyAction::PrevPage, &all),
        "clamped at page 1"
    );

    // Page 2 carries the witness; step into it, then turning resets time.
    assert!(state.update(KeyAction::NextPage, &all));
    assert!(state.update(KeyAction::StepForward, &all));
    assert!(state.update(KeyAction::StepForward, &all));
    assert_eq!(state.position, 2);
    assert!(state.update(KeyAction::NextPage, &all));
    assert_eq!((state.page, state.position), (2, 0));
    assert!(
        !state.update(KeyAction::NextPage, &all),
        "clamped at the end"
    );

    assert!(state.update(KeyAction::Back, &all));
    assert_eq!((state.screen, state.page), (Screen::List, 0));
}

#[test]
fn replay_keys_only_act_on_witness_pages() {
    let all = lessons();
    let mut state = LearnState::new();
    state.update(KeyAction::Open, &all);
    // Page 1 is prose-only: stepping is a no-op.
    for action in [
        KeyAction::StepForward,
        KeyAction::StepBack,
        KeyAction::JumpDivergence,
        KeyAction::JumpVerdict,
    ] {
        assert!(!state.update(action, &all), "{action:?} on a prose page");
    }

    state.update(KeyAction::NextPage, &all);
    let steps = all[0].pages[1].witness.as_ref().unwrap().steps.len();
    assert!(state.update(KeyAction::JumpVerdict, &all));
    assert_eq!(state.position, steps, "v lands after the verdict step");
    assert!(state.update(KeyAction::JumpDivergence, &all));
    assert!(state.position > 0 && state.position < steps);
    assert!(state.update(KeyAction::StepBack, &all));
}

#[test]
fn list_actions_are_inert_on_pages_and_vice_versa() {
    let all = lessons();
    let mut state = LearnState::new();
    assert!(!state.update(KeyAction::NextPage, &all), "no page in list");
    assert!(!state.update(KeyAction::Back, &all), "already at the list");
    state.update(KeyAction::Open, &all);
    assert!(!state.update(KeyAction::Down, &all), "no list on a page");
    assert!(!state.update(KeyAction::Open, &all));
}
