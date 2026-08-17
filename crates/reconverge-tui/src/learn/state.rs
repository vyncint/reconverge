//! Learn-mode state machine — pure transitions, no I/O.
//!
//! Two screens: the lesson list, and a lesson page. A page that carries a
//! witness embeds the debugger's timeline semantics: `position` walks it
//! exactly like the witness view (0 = launch instant, k = after step k-1),
//! and turning a page resets time.

use super::lessons::Lesson;
use crate::witness::state::divergence_position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnState {
    pub screen: Screen,
    /// Selected (List) or open (Page) lesson.
    pub lesson: usize,
    pub page: usize,
    /// Replay position on pages that carry a witness.
    pub position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
    Open,
    Back,
    NextPage,
    PrevPage,
    StepForward,
    StepBack,
    JumpDivergence,
    JumpVerdict,
}

impl LearnState {
    #[must_use]
    pub fn new() -> LearnState {
        LearnState {
            screen: Screen::List,
            lesson: 0,
            page: 0,
            position: 0,
        }
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
            (
                Screen::Page,
                KeyAction::StepForward
                | KeyAction::StepBack
                | KeyAction::JumpDivergence
                | KeyAction::JumpVerdict,
            ) => {
                let Some(witness) = &lessons[self.lesson].pages[self.page].witness else {
                    return false;
                };
                let last = witness.steps.len();
                match action {
                    KeyAction::StepForward => self.position = (self.position + 1).min(last),
                    KeyAction::StepBack => self.position = self.position.saturating_sub(1),
                    KeyAction::JumpDivergence => {
                        if let Some(position) = divergence_position(witness) {
                            self.position = position;
                        }
                    }
                    KeyAction::JumpVerdict => {
                        self.position = witness.verdict.step.map_or(last, |s| (s + 1).min(last));
                    }
                    _ => unreachable!(),
                }
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
