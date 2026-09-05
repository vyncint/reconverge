//! Inspector rendering: (data, state, area) → widgets. No I/O, no clock;
//! scrolling derives from the selection, so frames are pure functions of
//! (artifacts, key sequence).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use reconverge_artifacts::unimap::{Uniformity, Value, ValueSource};

use super::data::{FunctionData, InspectorData};
use super::state::InspectorState;
use crate::view::fit;
use reconverge_artifacts::plural;

pub struct InspectorView<'a> {
    pub data: &'a InspectorData,
    pub state: &'a InspectorState,
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

impl InspectorView<'_> {
    fn accent(&self, style: Style) -> Style {
        if self.color { style } else { Style::default() }
    }

    fn glyph(&self, uniformity: Uniformity) -> &'static str {
        match (uniformity, self.ascii) {
            (Uniformity::Divergent, false) => "\u{25c6}", // ◆
            (Uniformity::Divergent, true) => "D",
            (Uniformity::Uniform, false) => "\u{00b7}", // ·
            (Uniformity::Uniform, true) => ".",
        }
    }

    fn label(value: &Value) -> String {
        value
            .name
            .clone()
            .map_or_else(|| value.id.clone(), |name| format!("`{name}`"))
    }

    fn describe(value: &Value) -> &'static str {
        match value.source {
            Some(ValueSource::ThreadIndex) => "thread-index",
            Some(ValueSource::KernelParam) => "kernel-param",
            Some(ValueSource::BlockIndex) => "block-index",
            Some(ValueSource::Constant) => "constant",
            Some(ValueSource::DivergentLoad) => "divergent-load",
            Some(ValueSource::AtomicReturn) => "atomic-return",
            Some(ValueSource::DivergentPhi) => "divergent-phi",
            Some(ValueSource::Derived) => "derived",
            None => match value.uniformity {
                Uniformity::Uniform => "uniform",
                Uniformity::Divergent => "divergent",
            },
        }
    }
}

/// The first visible source line for a window of `height` centered on
/// `focus` (1-based), clamped to the file.
#[must_use]
pub fn source_window(total_lines: usize, height: usize, focus_line: usize) -> usize {
    if total_lines <= height || height == 0 {
        return 1;
    }
    let half = height / 2;
    let start = focus_line.saturating_sub(half).max(1);
    start.min(total_lines - height + 1)
}

pub fn render(frame: &mut Frame<'_>, view: &InspectorView<'_>) {
    let area = frame.area();
    let block = Block::bordered()
        .border_set(if view.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        })
        .title(Span::styled(
            " reconverge inspect ",
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        ))
        .title_bottom(Span::styled(
            " j/k value  p walk  u back  n/N finding  f fn  q quit ",
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(function) = view.data.functions.get(view.state.function) else {
        let mut lines = vec![
            Line::from("no unimap loaded"),
            Line::from("usage: reconverge-tui inspect [--ascii] <unimap.json> [findings.json]"),
        ];
        for error in &view.data.errors {
            lines.push(Line::from(fit(error, inner.width as usize, view.ascii)));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    };

    // `data.errors` was written at four sites in this mode's loader and read
    // at none, so a truncated findings file, a nonexistent path and a valid
    // file with zero findings all rendered as a working inspector with
    // nothing to show — the state that means "you passed no findings file".
    // Nothing was printed, nothing logged, and the exit code was a success.
    let error_rows = u16::try_from(view.data.errors.len()).unwrap_or(u16::MAX);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // header
            Constraint::Length(error_rows), // load errors, when there are any
            Constraint::Min(3),             // body
            Constraint::Length(1),          // finding bar
        ])
        .split(inner);

    render_header(frame, view, function, rows[0]);
    if error_rows > 0 {
        let width = rows[1].width as usize;
        frame.render_widget(
            Paragraph::new(
                view.data
                    .errors
                    .iter()
                    .map(|error| {
                        Line::from(Span::styled(
                            fit(error, width, view.ascii),
                            view.accent(Style::default().fg(Color::Red)),
                        ))
                    })
                    .collect::<Vec<_>>(),
            ),
            rows[1],
        );
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[2]);
    render_source(frame, view, function, body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(body[1]);
    render_values(frame, view, function, right[0]);
    render_provenance(frame, view, function, right[1]);

    render_finding_bar(frame, view, rows[3]);
}

fn render_header(
    frame: &mut Frame<'_>,
    view: &InspectorView<'_>,
    function: &FunctionData,
    area: Rect,
) {
    let f = &function.function;
    let coverage = f.coverage.map_or_else(String::new, |c| {
        let total = c.analyzed_statements + c.opaque_statements;
        format!(" — {}/{total} statements analyzed", c.analyzed_statements)
    });
    let text = fit(
        &format!(
            "kernel {} ({}/{} functions){coverage}",
            f.name,
            view.state.function + 1,
            view.data.functions.len()
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

fn render_source(
    frame: &mut Frame<'_>,
    view: &InspectorView<'_>,
    function: &FunctionData,
    area: Rect,
) {
    let width = area.width as usize;
    let mut lines = Vec::new();
    match &function.source {
        None => {
            lines.push(Line::from(fit(
                &format!("source unavailable: {}", function.function.span.file),
                width,
                view.ascii,
            )));
        }
        Some(source) => {
            lines.push(Line::from(Span::styled(
                fit(&source.name, width, view.ascii),
                view.accent(Style::default().fg(Color::Cyan)),
            )));
            let height = (area.height as usize).saturating_sub(1);
            let focus = selected_span_line(view, function);
            let start = source_window(source.lines.len(), height, focus);
            for (offset, text) in source.lines.iter().skip(start - 1).take(height).enumerate() {
                let line_no = start + offset;
                let marker = if line_no == focus { '>' } else { ' ' };
                let row = fit(&format!("{marker}{line_no:>4} {text}"), width, view.ascii);
                let style = if line_no == focus {
                    view.accent(Style::default().add_modifier(Modifier::BOLD))
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(row, style)));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn selected_span_line(view: &InspectorView<'_>, function: &FunctionData) -> usize {
    function
        .value_index(&view.state.selected)
        .map_or(1, |i| function.function.values[i].span.line_start)
}

fn render_values(
    frame: &mut Frame<'_>,
    view: &InspectorView<'_>,
    function: &FunctionData,
    area: Rect,
) {
    let width = area.width as usize;
    let mut lines = vec![Line::from(Span::styled(
        fit("values", width, view.ascii),
        view.accent(Style::default().fg(Color::Cyan)),
    ))];
    for &index in &function.listed {
        let value = &function.function.values[index];
        let marker = if value.id == view.state.selected {
            '>'
        } else {
            ' '
        };
        let row = fit(
            &format!(
                "{marker}{} {} \u{2014} {}",
                InspectorView::glyph(view, value.uniformity),
                InspectorView::label(value),
                InspectorView::describe(value),
            ),
            width,
            view.ascii,
        );
        let style = match value.uniformity {
            Uniformity::Divergent => view.accent(Style::default().fg(Color::Yellow)),
            Uniformity::Uniform => Style::default(),
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_provenance(
    frame: &mut Frame<'_>,
    view: &InspectorView<'_>,
    function: &FunctionData,
    area: Rect,
) {
    let width = area.width as usize;
    let selected_label = function.value_index(&view.state.selected).map_or_else(
        || view.state.selected.clone(),
        |i| {
            let value = &function.function.values[i];
            format!(
                "{} ({})",
                InspectorView::label(value),
                InspectorView::describe(value)
            )
        },
    );
    let mut lines = vec![Line::from(Span::styled(
        fit(
            &format!("provenance of {selected_label}"),
            width,
            view.ascii,
        ),
        view.accent(Style::default().fg(Color::Cyan)),
    ))];
    let chain = function.chain_from(&view.state.selected);
    if chain.is_empty() {
        lines.push(Line::from(fit(
            "(a source: no incoming provenance)",
            width,
            view.ascii,
        )));
    }
    for (i, (what, _)) in chain.iter().enumerate() {
        lines.push(Line::from(fit(
            &format!("{:>2}. \u{2190} {what}", i + 1),
            width,
            view.ascii,
        )));
    }
    // Name where the chain bottoms out, when it is a recognized source.
    if let Some((_, last_from)) = chain.last()
        && let Some(index) = function.value_index(last_from)
    {
        let value = &function.function.values[index];
        if matches!(
            value.source,
            Some(ValueSource::ThreadIndex) | Some(ValueSource::AtomicReturn)
        ) {
            lines.push(Line::from(Span::styled(
                fit(
                    &format!(
                        "source: {} \u{2014} {}",
                        InspectorView::label(value),
                        InspectorView::describe(value)
                    ),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().fg(Color::Yellow)),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_finding_bar(frame: &mut Frame<'_>, view: &InspectorView<'_>, area: Rect) {
    let width = area.width as usize;
    let text = match view.state.finding {
        Some(index) => {
            let finding = &view.data.findings[index];
            format!(
                "finding {}/{} [{}] {}",
                index + 1,
                view.data.findings.len(),
                finding.code,
                finding.message
            )
        }
        None if view.data.findings.is_empty() => "no findings loaded".to_string(),
        None => format!(
            "{} {} loaded \u{2014} press n to visit {}",
            view.data.findings.len(),
            plural(view.data.findings.len(), "finding", "findings"),
            plural(view.data.findings.len(), "it", "them")
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            fit(&text, width, view.ascii),
            view.accent(Style::default().fg(Color::Magenta)),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::source_window;

    #[test]
    fn source_window_clamps_at_both_ends() {
        assert_eq!(source_window(100, 10, 1), 1);
        assert_eq!(source_window(100, 10, 50), 45);
        assert_eq!(source_window(100, 10, 100), 91);
        assert_eq!(source_window(5, 10, 3), 1, "short files never scroll");
        assert_eq!(source_window(10, 0, 3), 1);
    }
}
