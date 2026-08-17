//! Triage state machine — pure transitions, no I/O.
//!
//! The baseline lives here because it is exactly what the key sequence
//! produces. Writing it to disk is the caller's job: the state only raises
//! a request (`w`) and records the outcome, so every transition stays pure
//! and testable, and the one place that touches the filesystem is the one
//! place that had to.

use reconverge_artifacts::baseline::BaselineArtifact;
use unicode_segmentation::UnicodeSegmentation;

use super::data::TriageData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageState {
    /// Index into `data.items`.
    pub selected: usize,
    /// The reviewed baseline as edited so far.
    pub baseline: BaselineArtifact,
    /// Reason buffer while typing a suppression; `None` when not editing.
    pub editing: Option<String>,
    /// Edits not yet written to disk.
    pub dirty: bool,
    /// `q` was pressed with unsaved edits; the view asks for confirmation.
    pub confirm_quit: bool,
    /// Feedback from the last action, rendered in the status bar.
    pub status: Status,
    write_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    None,
    /// Baseline written, with the number of entries.
    Wrote(usize),
    /// Write failed; the message is the underlying error, verbatim.
    WriteFailed(String),
    /// A suppression was attempted with a blank reason.
    ReasonRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    Next,
    Prev,
    BeginSuppress,
    Unsuppress,
    ReasonChar(char),
    ReasonBackspace,
    ReasonCommit,
    ReasonCancel,
    RequestWrite,
}

impl TriageState {
    #[must_use]
    pub fn new(baseline: BaselineArtifact) -> TriageState {
        TriageState {
            selected: 0,
            baseline,
            editing: None,
            dirty: false,
            confirm_quit: false,
            status: Status::None,
            write_requested: false,
        }
    }

    /// The suppression reason for the selected finding, if it has one.
    #[must_use]
    pub fn suppression_of(&self, data: &TriageData, index: usize) -> Option<&str> {
        let item = data.items.get(index)?;
        self.baseline
            .suppression_of(&item.krate, &item.finding)
            .map(|entry| entry.reason.as_str())
    }

    /// Apply one action; returns true when anything changed (the caller
    /// redraws only then — event-driven by construction).
    pub fn update(&mut self, action: KeyAction, data: &TriageData) -> bool {
        let before = self.clone();
        // Any deliberate action answers a pending quit prompt.
        self.confirm_quit = false;

        match action {
            _ if data.items.is_empty() => {}
            KeyAction::Next | KeyAction::Prev if self.editing.is_none() => {
                let n = data.items.len() as isize;
                let step = if action == KeyAction::Next { 1 } else { -1 };
                self.selected = (self.selected as isize + step).rem_euclid(n) as usize;
                self.status = Status::None;
            }
            KeyAction::BeginSuppress
                if self.editing.is_none() && self.suppression_of(data, self.selected).is_none() =>
            {
                self.editing = Some(String::new());
                self.status = Status::None;
            }
            KeyAction::Unsuppress if self.editing.is_none() => {
                let item = &data.items[self.selected];
                if self.baseline.unsuppress(&item.krate, &item.finding) {
                    self.dirty = true;
                    self.status = Status::None;
                }
            }
            KeyAction::ReasonChar(c) => {
                if let Some(buffer) = &mut self.editing {
                    buffer.push(c);
                }
            }
            KeyAction::ReasonBackspace => {
                if let Some(buffer) = &mut self.editing {
                    // Grapheme-aware: one keystroke removes one visible
                    // character, including letters built from combining marks.
                    let mut graphemes: Vec<&str> = buffer.graphemes(true).collect();
                    graphemes.pop();
                    *buffer = graphemes.concat();
                }
            }
            KeyAction::ReasonCommit => {
                if let Some(buffer) = self.editing.clone() {
                    let reason = buffer.trim();
                    if reason.is_empty() {
                        // A suppression without a reason is the debt this
                        // whole file exists to prevent: stay in the editor.
                        self.status = Status::ReasonRequired;
                    } else {
                        let item = &data.items[self.selected];
                        if self.baseline.suppress(&item.krate, &item.finding, reason) {
                            self.dirty = true;
                        }
                        self.editing = None;
                        self.status = Status::None;
                    }
                }
            }
            KeyAction::ReasonCancel if self.editing.is_some() => {
                self.editing = None;
                self.status = Status::None;
            }
            KeyAction::RequestWrite if self.editing.is_none() => {
                self.write_requested = true;
            }
            _ => {}
        }
        *self != before
    }

    /// Take a pending write request (the caller performs the I/O).
    pub fn take_write_request(&mut self) -> bool {
        std::mem::take(&mut self.write_requested)
    }

    /// Record what the caller's write actually did.
    pub fn record_write(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.dirty = false;
                self.status = Status::Wrote(self.baseline.entries.len());
            }
            Err(message) => self.status = Status::WriteFailed(message),
        }
    }

    /// Whether `q` may exit now. Unsaved edits get one confirmation first —
    /// losing a review pass to a stray keystroke would be its own bug.
    pub fn request_quit(&mut self) -> bool {
        if self.dirty && !self.confirm_quit {
            self.confirm_quit = true;
            return false;
        }
        true
    }
}
