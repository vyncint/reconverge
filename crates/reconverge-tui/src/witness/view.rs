//! Debugger rendering: (data, state, area) → widgets. No I/O, no clock;
//! every frame is a pure function of (artifacts, key sequence).
//!
//! The lane strip uses the same glyph language as the text diagnostics'
//! ASCII warp diagram — `o` active, `W` waiting, `.` exited, in groups of
//! eight — so the diagram a CI log prints is literally a frame of this
//! view. The glyphs are ASCII in both modes by design; `--ascii` only
//! changes borders, ellipses, and punctuation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use reconverge_artifacts::witness::{LaneState, Step, VerdictKind, WitnessArtifact};

use super::data::WitnessData;
use super::state::WitnessState;
use crate::view::fit;

pub struct WitnessView<'a> {
    pub data: &'a WitnessData,
    pub state: &'a WitnessState,
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

/// Width of the row-label column, so the lane strips line up under it.
const LABEL_WIDTH: usize = 10;

/// Keys shown along the bottom border.
const KEYS: &str = " h/l step  g/G ends  d split  v verdict  n/N witness  q quit ";

/// What a verdict kind is called on screen (learn mode shows the same
/// words over the same replays).
#[must_use]
pub fn verdict_word(kind: VerdictKind) -> &'static str {
    match kind {
        VerdictKind::Hang => "hang",
        VerdictKind::UndefinedBehavior => "undefined behavior",
        VerdictKind::Completed => "completed",
        VerdictKind::NoWitness => "no witness",
    }
}

/// One lane-state glyph — deliberately the diagnostics' diagram alphabet.
#[must_use]
pub fn lane_glyph(state: LaneState) -> char {
    match state {
        LaneState::Active => 'o',
        LaneState::Waiting => 'W',
        LaneState::Exited => '.',
    }
}

/// 32 per-lane characters, grouped by eight: `"o.o.o.o. o.o.o.o. …"`.
#[must_use]
pub fn strip(cells: &[char]) -> String {
    let mut out = String::with_capacity(cells.len() + cells.len() / 8);
    for (i, c) in cells.iter().enumerate() {
        if i > 0 && i % 8 == 0 {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

/// The strip of a 32-bit lane mask: `#` named, `.` not.
#[must_use]
pub fn mask_strip(mask: u32) -> String {
    let cells: Vec<char> = (0..32)
        .map(|lane| if mask & (1 << lane) != 0 { '#' } else { '.' })
        .collect();
    strip(&cells)
}

/// Column header aligned with [`strip`]'s groups.
#[must_use]
pub fn lane_indices() -> String {
    let mut out = String::new();
    for group in 0..4 {
        let label = (group * 8).to_string();
        out.push_str(&label);
        if group < 3 {
            out.push_str(&" ".repeat(9 - label.len()));
        }
    }
    out
}

/// Parse the artifact's `0x` + 8-hex-digit mask encoding.
#[must_use]
pub fn parse_mask(hex: &str) -> Option<u32> {
    u32::from_str_radix(hex.strip_prefix("0x")?, 16).ok()
}

/// The timeline bar: one cell per step up to 40, proportional beyond.
/// `=` executed, `>` the event just executed, `.` still ahead.
#[must_use]
pub fn progress_bar(total_steps: usize, position: usize) -> String {
    if total_steps == 0 {
        return String::new();
    }
    let cells = total_steps.min(40);
    let scale = |value: usize| value * cells / total_steps;
    let done = scale(position);
    let mut bar = String::with_capacity(cells + 2);
    bar.push('[');
    for cell in 0..cells {
        bar.push(if position > 0 && cell + 1 == done {
            '>'
        } else if cell < done {
            '='
        } else {
            '.'
        });
    }
    bar.push(']');
    bar
}

/// The color channel of a lane state (the text channel is [`lane_glyph`]).
#[must_use]
pub fn lane_state_style(state: LaneState) -> Style {
    match state {
        LaneState::Active => Style::default().fg(Color::Green),
        LaneState::Waiting => Style::default().fg(Color::Yellow),
        LaneState::Exited => Style::default().add_modifier(Modifier::DIM),
    }
}

impl WitnessView<'_> {
    fn accent(&self, style: Style) -> Style {
        if self.color { style } else { Style::default() }
    }

    fn lane_style(&self, state: LaneState) -> Style {
        self.accent(lane_state_style(state))
    }
}

pub fn render(frame: &mut Frame<'_>, view: &WitnessView<'_>) {
    let area = frame.area();
    let block = Block::bordered()
        .border_set(if view.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        })
        .title(Span::styled(
            " reconverge witness ",
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        ))
        .title_bottom(Span::styled(
            fit(KEYS, area.width.saturating_sub(2) as usize, view.ascii),
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(witness) = view.data.witnesses.get(view.state.witness) else {
        let mut lines = vec![Line::from(fit(
            "no witness artifacts loaded",
            inner.width as usize,
            view.ascii,
        ))];
        lines.push(Line::from(fit(
            "usage: reconverge-tui witness [--ascii] <witness.json>...",
            inner.width as usize,
            view.ascii,
        )));
        for error in &view.data.errors {
            lines.push(Line::from(fit(error, inner.width as usize, view.ascii)));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header + blank
            Constraint::Length(5), // lanes: indices, lanes, mask, active, legend
            Constraint::Length(5), // event: blank, step, statement, at, event line
            Constraint::Min(2),    // verdict
        ])
        .split(inner);

    render_header(frame, view, witness, rows[0]);
    render_lanes(frame, view, witness, rows[1]);
    render_event(frame, view, witness, rows[2]);
    render_verdict(frame, view, witness, rows[3]);
}

/// The step the current position has just executed, if any.
fn executed_step(witness: &WitnessArtifact, position: usize) -> Option<&Step> {
    position.checked_sub(1).and_then(|i| witness.steps.get(i))
}

fn render_header(
    frame: &mut Frame<'_>,
    view: &WitnessView<'_>,
    witness: &WitnessArtifact,
    area: Rect,
) {
    let code = witness
        .finding
        .as_ref()
        .map_or_else(String::new, |f| format!(" — {}", f.code));
    let launch = &witness.launch;
    let warp = launch
        .warp
        .map_or_else(String::new, |w| format!(" warp {w}"));
    let text = fit(
        &format!(
            "witness {}/{} — kernel `{}`{code} — grid ({},{},{}) block ({},{},{}){warp}",
            view.state.witness + 1,
            view.data.witnesses.len(),
            witness.kernel,
            launch.grid[0],
            launch.grid[1],
            launch.grid[2],
            launch.block[0],
            launch.block[1],
            launch.block[2],
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

/// Width-aware left padding of a row label to the fixed label column —
/// shared with learn mode's compact replay panel so the strips line up
/// there too.
pub fn label_cell(text: &str) -> String {
    use unicode_width::UnicodeWidthStr;
    let pad = LABEL_WIDTH.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(pad))
}

fn render_lanes(
    frame: &mut Frame<'_>,
    view: &WitnessView<'_>,
    witness: &WitnessArtifact,
    area: Rect,
) {
    let width = area.width as usize;
    let states = witness.lane_states_at(view.state.position.checked_sub(1));

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        fit(
            &format!("{}{}", label_cell(""), lane_indices()),
            width,
            view.ascii,
        ),
        view.accent(Style::default().add_modifier(Modifier::DIM)),
    )));

    // The lane strip, one styled span per lane so states are color-coded
    // cell by cell (NO_COLOR renders the identical text).
    let mut spans = vec![Span::raw(label_cell("lanes"))];
    for (lane, state) in states.iter().enumerate() {
        if lane > 0 && lane % 8 == 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            lane_glyph(*state).to_string(),
            view.lane_style(*state),
        ));
    }
    lines.push(Line::from(spans));

    // Mask and active rows: real strips at a collective step, a quiet
    // placeholder elsewhere so the layout never jumps.
    let warp_op = executed_step(witness, view.state.position).and_then(|s| s.warp_op.as_ref());
    match warp_op {
        Some(op) => {
            let mask_text = parse_mask(&op.mask).map_or(op.mask.clone(), mask_strip);
            let active_text = parse_mask(&op.active).map_or(op.active.clone(), mask_strip);
            lines.push(Line::from(Span::styled(
                fit(
                    &format!("{}{mask_text}  {}", label_cell("mask"), op.mask),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().fg(Color::Magenta)),
            )));
            lines.push(Line::from(Span::styled(
                fit(
                    &format!("{}{active_text}  {}", label_cell("active"), op.active),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().fg(Color::Green)),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                fit(
                    &format!("{}(not at a warp collective)", label_cell("mask")),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().add_modifier(Modifier::DIM)),
            )));
            lines.push(Line::default());
        }
    }

    lines.push(Line::from(Span::styled(
        fit(
            &format!("{}o active   W waiting   . exited", label_cell("")),
            width,
            view.ascii,
        ),
        view.accent(Style::default().add_modifier(Modifier::DIM)),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_event(
    frame: &mut Frame<'_>,
    view: &WitnessView<'_>,
    witness: &WitnessArtifact,
    area: Rect,
) {
    let width = area.width as usize;
    let position = view.state.position;
    let total = witness.steps.len();

    let mut lines = vec![Line::default()];
    lines.push(Line::from(Span::styled(
        fit(
            &format!("step {position}/{total}  {}", progress_bar(total, position)),
            width,
            view.ascii,
        ),
        view.accent(Style::default().fg(Color::Cyan)),
    )));

    match executed_step(witness, position) {
        None => {
            lines.push(Line::from(fit(
                "launch — before the first event",
                width,
                view.ascii,
            )));
        }
        Some(step) => {
            lines.push(Line::from(fit(&step.statement, width, view.ascii)));
            if let Some(span) = &step.span {
                lines.push(Line::from(Span::styled(
                    fit(
                        &format!("at {}:{}", span.file, span.line_start),
                        width,
                        view.ascii,
                    ),
                    view.accent(Style::default().add_modifier(Modifier::DIM)),
                )));
            } else {
                lines.push(Line::default());
            }
            if let Some(barrier) = &step.barrier {
                lines.push(Line::from(Span::styled(
                    fit(
                        &format!(
                            "barrier: {} of {} threads arrived",
                            barrier.arrived, barrier.expected
                        ),
                        width,
                        view.ascii,
                    ),
                    view.accent(Style::default().fg(Color::Yellow)),
                )));
            } else if let Some(op) = &step.warp_op
                && let (Some(mask), Some(active)) = (parse_mask(&op.mask), parse_mask(&op.active))
            {
                let absent = mask & !active;
                lines.push(Line::from(Span::styled(
                    fit(
                        &format!(
                            "{} — named in the mask but not active: {absent:#010x}",
                            op.op
                        ),
                        width,
                        view.ascii,
                    ),
                    view.accent(Style::default().fg(Color::Yellow)),
                )));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_verdict(
    frame: &mut Frame<'_>,
    view: &WitnessView<'_>,
    witness: &WitnessArtifact,
    area: Rect,
) {
    let width = area.width as usize;
    let verdict = &witness.verdict;
    // The verdict "lands" once the timeline reaches its step.
    let reached = verdict
        .step
        .is_none_or(|s| view.state.position > s.min(witness.steps.len().saturating_sub(1)));
    let at = verdict
        .step
        .map_or_else(String::new, |s| format!(" (at step {})", s + 1));

    let title_style = if reached {
        view.accent(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        view.accent(Style::default().add_modifier(Modifier::DIM))
    };
    let mut lines = vec![Line::default()];
    lines.push(Line::from(Span::styled(
        fit(
            &format!("verdict: {}{at}", verdict_word(verdict.kind)),
            width,
            view.ascii,
        ),
        title_style,
    )));
    if reached {
        lines.push(Line::from(fit(
            &verdict.message,
            width.saturating_mul(area.height.saturating_sub(2) as usize),
            view.ascii,
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_group_lanes_by_eight() {
        let all_active = strip(&['o'; 32]);
        assert_eq!(all_active, "oooooooo oooooooo oooooooo oooooooo");
        assert_eq!(
            mask_strip(0xffff_ffff),
            "######## ######## ######## ########"
        );
        assert_eq!(
            mask_strip(0x5555_5555),
            "#.#.#.#. #.#.#.#. #.#.#.#. #.#.#.#."
        );
        assert_eq!(lane_indices(), "0        8        16       24");
    }

    #[test]
    fn progress_bar_marks_the_just_executed_event() {
        assert_eq!(progress_bar(5, 0), "[.....]");
        assert_eq!(progress_bar(5, 1), "[>....]");
        assert_eq!(progress_bar(5, 4), "[===>.]");
        assert_eq!(progress_bar(5, 5), "[====>]");
        assert_eq!(progress_bar(0, 0), "");
        assert_eq!(progress_bar(80, 40).len(), 42, "proportional past 40");
    }

    #[test]
    fn masks_parse_from_the_artifact_encoding() {
        assert_eq!(parse_mask("0xffffffff"), Some(0xffff_ffff));
        assert_eq!(parse_mask("0x55555555"), Some(0x5555_5555));
        assert_eq!(parse_mask("garbage"), None);
    }

    #[test]
    fn row_labels_fit_the_label_column() {
        use unicode_width::UnicodeWidthStr;
        for label in ["lanes", "mask", "active"] {
            assert!(label.width() < LABEL_WIDTH, "{label:?} does not fit");
        }
    }
}
