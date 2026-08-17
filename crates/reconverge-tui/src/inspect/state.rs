//! Inspector state machine — pure transitions, no I/O.

use super::data::InspectorData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorState {
    /// Index into `data.functions`.
    pub function: usize,
    /// Selected value id within the current function.
    pub selected: String,
    /// Provenance-walk history for `u`.
    pub history: Vec<String>,
    /// Selected finding (index into `data.findings`).
    pub finding: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    NextValue,
    PrevValue,
    WalkProvenance,
    WalkBack,
    NextFinding,
    PrevFinding,
    NextFunction,
}

impl InspectorState {
    /// Initial state: first function, first listed value, no finding.
    #[must_use]
    pub fn new(data: &InspectorData) -> InspectorState {
        let selected = data
            .functions
            .first()
            .and_then(|f| f.listed.first().map(|&i| f.function.values[i].id.clone()))
            .unwrap_or_default();
        InspectorState {
            function: 0,
            selected,
            history: Vec::new(),
            finding: None,
        }
    }

    /// Apply one action; returns true when anything changed (the caller
    /// redraws only then — event-driven by construction).
    pub fn update(&mut self, action: KeyAction, data: &InspectorData) -> bool {
        if data.functions.is_empty() {
            return false;
        }
        let before = self.clone();
        match action {
            KeyAction::NextValue => self.move_selection(data, 1),
            KeyAction::PrevValue => self.move_selection(data, -1),
            KeyAction::WalkProvenance => self.walk(data),
            KeyAction::WalkBack => {
                if let Some(previous) = self.history.pop() {
                    self.selected = previous;
                }
            }
            KeyAction::NextFinding => self.jump_finding(data, 1),
            KeyAction::PrevFinding => self.jump_finding(data, -1),
            KeyAction::NextFunction => {
                self.function = (self.function + 1) % data.functions.len();
                self.reset_selection(data);
            }
        }
        *self != before
    }

    fn reset_selection(&mut self, data: &InspectorData) {
        let f = &data.functions[self.function];
        self.selected = f
            .listed
            .first()
            .map(|&i| f.function.values[i].id.clone())
            .unwrap_or_default();
        self.history.clear();
    }

    fn move_selection(&mut self, data: &InspectorData, step: isize) {
        let f = &data.functions[self.function];
        if f.listed.is_empty() {
            return;
        }
        let ids: Vec<&str> = f
            .listed
            .iter()
            .map(|&i| f.function.values[i].id.as_str())
            .collect();
        let position = ids.iter().position(|&id| id == self.selected);
        let next = match position {
            // A selection outside the list (mid provenance walk) restarts
            // the list from the top.
            None => 0,
            Some(current) => (current as isize + step).rem_euclid(ids.len() as isize) as usize,
        };
        self.selected = ids[next].to_string();
        self.history.clear();
    }

    fn walk(&mut self, data: &InspectorData) {
        let f = &data.functions[self.function];
        let chain = f.chain_from(&self.selected);
        if let Some((_, from)) = chain.first() {
            self.history.push(self.selected.clone());
            self.selected = from.clone();
        }
    }

    fn jump_finding(&mut self, data: &InspectorData, step: isize) {
        if data.findings.is_empty() {
            return;
        }
        let n = data.findings.len() as isize;
        let next = match self.finding {
            None => {
                if step > 0 {
                    0
                } else {
                    (n - 1) as usize
                }
            }
            Some(current) => (current as isize + step).rem_euclid(n) as usize,
        };
        self.finding = Some(next);
        let finding = &data.findings[next];

        // Land in the finding's function, selecting the value at its first
        // provenance step (RC001: the divergent branch condition).
        if let Some(kernel) = &finding.kernel
            && let Some(function) = data
                .functions
                .iter()
                .position(|f| &f.function.name == kernel)
        {
            self.function = function;
            self.history.clear();
            let f = &data.functions[function];
            let target = finding
                .provenance
                .first()
                .and_then(|step| f.value_at(&step.span))
                .map(str::to_string);
            match target {
                Some(id) => self.selected = id,
                None => self.reset_selection(data),
            }
        }
    }
}
