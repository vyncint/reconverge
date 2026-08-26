//! Triage rendering: (data, state, area) → widgets. No I/O, no clock;
//! every frame is a pure function of (artifacts, key sequence).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use reconverge_artifacts::findings::Confidence;

use super::data::TriageData;
use super::state::{Status, TriageState};
use crate::view::fit;
use reconverge_artifacts::plural;

pub struct TriageView<'a> {
    pub data: &'a TriageData,
    pub state: &'a TriageState,
    pub ascii: bool,
    pub color: bool,
}

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Keys shown along the bottom border: reviewing, then editing a reason.
const KEYS: &str = " j/k move  s suppress  u unsuppress  w write  q quit ";
const EDITING_KEYS: &str = " type a reason  Enter save  Esc cancel ";

/// A suppression with no reason is the silent debt the baseline exists to
/// prevent, so the editor says so instead of accepting one.
const REASON_REQUIRED: &str =
    "a reason is required — a silent suppression is the debt this file prevents";
const CONFIRM_QUIT: &str = "unsaved edits — press w to write, or Q to discard and quit";

impl TriageView<'_> {
    fn accent(&self, style: Style) -> Style {
        if self.color { style } else { Style::default() }
    }
}

/// Severity word for a finding's confidence tier (ASCII in both modes).
fn tier(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Deny => "deny",
        Confidence::Confirmed => "confirmed",
        Confidence::Warning => "warning",
    }
}

pub fn render(frame: &mut Frame<'_>, view: &TriageView<'_>) {
    let area = frame.area();
    let keys = if view.state.editing.is_some() {
        EDITING_KEYS
    } else {
        KEYS
    };
    let block = Block::bordered()
        .border_set(if view.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        })
        .title(Span::styled(
            " reconverge triage ",
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        ))
        .title_bottom(Span::styled(
            fit(keys, area.width.saturating_sub(2) as usize, view.ascii),
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if view.data.items.is_empty() {
        let mut lines = vec![Line::from(fit(
            "no findings to triage",
            inner.width as usize,
            view.ascii,
        ))];
        for error in &view.data.errors {
            lines.push(Line::from(fit(error, inner.width as usize, view.ascii)));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header + blank
            Constraint::Min(3),    // finding list
            Constraint::Length(5), // detail + editor
            Constraint::Length(1), // status
        ])
        .split(inner);

    render_header(frame, view, rows[0]);
    render_list(frame, view, rows[1]);
    render_detail(frame, view, rows[2]);
    render_status(frame, view, rows[3]);
}

fn render_header(frame: &mut Frame<'_>, view: &TriageView<'_>, area: Rect) {
    let suppressed = (0..view.data.items.len())
        .filter(|&i| view.state.suppression_of(view.data, i).is_some())
        .count();
    let baseline_name = view
        .data
        .baseline_path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let dirty = if view.state.dirty {
        " (unsaved)".to_string()
    } else {
        String::new()
    };
    let text = fit(
        &format!(
            "{} {} — {suppressed} suppressed — baseline: {baseline_name}{dirty}",
            view.data.items.len(),
            plural(view.data.items.len(), "finding", "findings"),
        ),
        area.width as usize,
        view.ascii,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        ))),
        area,
    );
}

/// The first visible row for a list window of `height` around `selected`.
#[must_use]
pub fn list_window(total: usize, height: usize, selected: usize) -> usize {
    if total <= height || height == 0 {
        return 0;
    }
    let half = height / 2;
    selected.saturating_sub(half).min(total - height)
}

fn render_list(frame: &mut Frame<'_>, view: &TriageView<'_>, area: Rect) {
    let width = area.width as usize;
    let height = area.height as usize;
    let start = list_window(view.data.items.len(), height, view.state.selected);
    let mut lines = Vec::new();
    for (index, item) in view.data.items.iter().enumerate().skip(start).take(height) {
        let selected = index == view.state.selected;
        let accepted = view.state.suppression_of(view.data, index).is_some();
        let marker = if selected { '>' } else { ' ' };
        let box_ = if accepted { 's' } else { ' ' };
        let kernel = item.finding.kernel.as_deref().unwrap_or("-");
        let row = fit(
            &format!(
                "{marker}[{box_}] {:<9} {} {kernel:<24} {}",
                tier(item.finding.confidence),
                item.finding.code,
                item.finding.message,
            ),
            width,
            view.ascii,
        );
        let style = match (selected, accepted) {
            (true, _) => view.accent(Style::default().add_modifier(Modifier::BOLD)),
            (false, true) => view.accent(Style::default().add_modifier(Modifier::DIM)),
            (false, false) => Style::default(),
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_detail(frame: &mut Frame<'_>, view: &TriageView<'_>, area: Rect) {
    let width = area.width as usize;
    let item = &view.data.items[view.state.selected];
    let accepted = view.state.suppression_of(view.data, view.state.selected);

    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled(
            fit(
                &format!(
                    "{} {} \u{2014} {}:{} \u{2014} {}",
                    item.krate,
                    item.finding.code,
                    item.finding.span.file,
                    item.finding.span.line_start,
                    if accepted.is_some() {
                        "accepted"
                    } else {
                        "open"
                    },
                ),
                width,
                view.ascii,
            ),
            view.accent(Style::default().fg(Color::Cyan)),
        )),
        Line::from(fit(&item.finding.message, width, view.ascii)),
    ];

    match (&view.state.editing, accepted) {
        (Some(buffer), _) => {
            lines.push(Line::from(Span::styled(
                fit("why is this acceptable?", width, view.ascii),
                view.accent(Style::default().add_modifier(Modifier::DIM)),
            )));
            // A block cursor makes the editor obvious without any timer.
            lines.push(Line::from(Span::styled(
                fit(&format!("reason: {buffer}\u{2588}"), width, view.ascii),
                view.accent(Style::default().fg(Color::Yellow)),
            )));
        }
        (None, Some(reason)) => {
            lines.push(Line::from(Span::styled(
                fit(&format!("reason: {reason}"), width, view.ascii),
                view.accent(Style::default().fg(Color::Green)),
            )));
        }
        (None, None) => {}
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_status(frame: &mut Frame<'_>, view: &TriageView<'_>, area: Rect) {
    let width = area.width as usize;
    let (text, color) = if view.state.confirm_quit {
        (CONFIRM_QUIT.to_string(), Color::Yellow)
    } else {
        match &view.state.status {
            Status::None => (String::new(), Color::Reset),
            Status::Wrote(entries) => (
                format!("baseline written — {entries} entry(ies)"),
                Color::Green,
            ),
            Status::WriteFailed(error) => (format!("write failed: {error}"), Color::Red),
            Status::ReasonRequired => (REASON_REQUIRED.to_string(), Color::Yellow),
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            fit(&text, width, view.ascii),
            view.accent(Style::default().fg(color)),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_window_keeps_the_selection_visible() {
        assert_eq!(list_window(3, 10, 2), 0, "short lists never scroll");
        assert_eq!(list_window(100, 10, 0), 0);
        assert_eq!(list_window(100, 10, 50), 45);
        assert_eq!(list_window(100, 10, 99), 90, "clamped at the end");
        assert_eq!(list_window(100, 0, 5), 0);
    }

    #[test]
    fn tier_words_are_the_confidence_tiers() {
        assert_eq!(tier(Confidence::Deny), "deny");
        assert_eq!(tier(Confidence::Confirmed), "confirmed");
        assert_eq!(tier(Confidence::Warning), "warning");
    }
}
