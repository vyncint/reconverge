//! Learn-mode state machine — pure transitions, no I/O.
//!
//! Two screens: the lesson list, and a lesson page. A page that carries a
//! witness embeds the debugger's timeline semantics: `position` walks it
//! exactly like the witness view (0 = launch instant, k = after step k-1),
//! and turning a page resets time.

use super::lessons::Lesson;
use crate::witness::state::divergence_position;
use reconverge_artifacts::witness::WitnessArtifact;

/// Which screen the learner is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The lesson list.
    List,
    /// A page of the open lesson.
    Page,
}

/// Cursor state of the learn view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnState {
    /// Which screen is showing.
    pub screen: Screen,
    /// Selected (List) or open (Page) lesson.
    pub lesson: usize,
    /// Page index within the open lesson.
    pub page: usize,
    /// Replay position on pages that carry a witness.
    pub position: usize,
}

/// What a keypress asks the learn view to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Move down the lesson list.
    Down,
    /// Move up the lesson list.
    Up,
    /// Open the selected lesson.
    Open,
    /// Return to the lesson list.
    Back,
    /// Turn to the next page.
    NextPage,
    /// Turn back one page.
    PrevPage,
    /// Advance the page's witness one step.
    StepForward,
    /// Rewind the page's witness one step.
    StepBack,
    /// Jump to the step where the lanes diverge.
    JumpDivergence,
    /// Jump to the verdict.
    JumpVerdict,
}

impl LearnState {
    /// The initial state: the lesson list, first lesson selected.
    #[must_use]
    pub fn new() -> LearnState {
        LearnState {
            screen: Screen::List,
            lesson: 0,
            page: 0,
            position: 0,
        }
    }

    /// The witness the current page replays, if it has one.
    fn witness<'a>(&self, lessons: &'a [Lesson]) -> Option<&'a WitnessArtifact> {
        lessons[self.lesson].pages[self.page].witness.as_ref()
    }

    /// Apply one action; returns true when anything changed (the caller
    /// redraws only then — event-driven by construction).
    pub fn update(&mut self, action: KeyAction, lessons: &[Lesson]) -> bool {
        if lessons.is_empty() {
            return false;
        }
        let before = self.clone();
        match (self.screen, action) {
            (Screen::List, KeyAction::Down) => {
                self.lesson = (self.lesson + 1) % lessons.len();
            }
            (Screen::List, KeyAction::Up) => {
                self.lesson = (self.lesson + lessons.len() - 1) % lessons.len();
            }
            (Screen::List, KeyAction::Open) => {
                self.screen = Screen::Page;
                self.page = 0;
                self.position = 0;
            }
            (Screen::Page, KeyAction::Back) => {
                self.screen = Screen::List;
                self.page = 0;
                self.position = 0;
            }
            (Screen::Page, KeyAction::NextPage) => {
                let pages = lessons[self.lesson].pages.len();
                if self.page + 1 < pages {
                    self.page += 1;
                    self.position = 0;
                }
            }
            (Screen::Page, KeyAction::PrevPage) => {
                if self.page > 0 {
                    self.page -= 1;
                    self.position = 0;
                }
            }
            // The four replay actions on a page that has a witness. One arm
            // each rather than a nested match with a catch-all: the outer
            // pattern already says which actions arrive here, and a new
            // `KeyAction` then fails to compile instead of reaching an arm
            // that "cannot happen".
            (Screen::Page, KeyAction::StepForward) => {
                let Some(witness) = self.witness(lessons) else {
                    return false;
                };
                self.position = (self.position + 1).min(witness.steps.len());
            }
            (Screen::Page, KeyAction::StepBack) => {
                if self.witness(lessons).is_none() {
                    return false;
                }
                self.position = self.position.saturating_sub(1);
            }
            (Screen::Page, KeyAction::JumpDivergence) => {
                let Some(witness) = self.witness(lessons) else {
                    return false;
                };
                if let Some(position) = divergence_position(witness) {
                    self.position = position;
                }
            }
            (Screen::Page, KeyAction::JumpVerdict) => {
                let Some(witness) = self.witness(lessons) else {
                    return false;
                };
                let last = witness.steps.len();
                self.position = witness.verdict.step.map_or(last, |s| (s + 1).min(last));
            }
            _ => return false,
        }
        *self != before
    }
}

impl Default for LearnState {
    fn default() -> Self {
        LearnState::new()
    }
}
