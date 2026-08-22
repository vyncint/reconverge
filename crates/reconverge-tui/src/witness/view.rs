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

/// Rows reserved for the verdict, pinned to the bottom: its heading plus two
/// wrapped lines of explanation, which is what the longest verdict needs.
const VERDICT_ROWS: u16 = 4;

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

    // The lane strip is one row per warp; a one-warp witness keeps the
    // historical five-row block exactly.
    let warp_rows = u16::from(witness.lanes.max(1)).div_ceil(32);
    // One row per step plus the launch, clamped to what is left once the
    // fixed blocks and the verdict have taken theirs — the verdict is the
    // conclusion and must never be the thing that scrolls away.
    let fixed = 2 + (4 + warp_rows) + 5 + VERDICT_ROWS;
    let timeline_rows = u16::try_from(witness.steps.len() + 1)
        .unwrap_or(u16::MAX)
        .min(inner.height.saturating_sub(fixed));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header + blank
            // lanes: indices, one strip per warp, mask, active, legend
            Constraint::Length(4 + warp_rows),
            Constraint::Length(5), // event: blank, step, statement, at, event line
            // The timeline takes as much as it has to say and no more, so the
            // verdict follows it directly and any slack falls off the bottom.
            // Sized here rather than by a `Min`, because a `Min` would leave
            // the gap in the middle of the screen — which reads worse than a
            // short page, and was half the reason this area looked unfinished.
            Constraint::Length(timeline_rows),
            Constraint::Length(VERDICT_ROWS),
            Constraint::Min(0),
        ])
        .split(inner);

    render_header(frame, view, witness, rows[0]);
    render_lanes(frame, view, witness, rows[1]);
    render_event(frame, view, witness, rows[2]);
    render_timeline(frame, view, witness, rows[3]);
    render_verdict(frame, view, witness, rows[4]);
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
    // cell by cell (NO_COLOR renders the identical text). Blocks wider
    // than a warp get one row per warp, labeled by its warp index; the
    // indices header above applies to every row.
    for (warp, chunk) in states.chunks(32).enumerate() {
        let label = if states.len() > 32 {
            format!("w{warp}")
        } else {
            "lanes".to_string()
        };
        let mut spans = vec![Span::raw(label_cell(&label))];
        for (lane, state) in chunk.iter().enumerate() {
            if lane > 0 && lane % 8 == 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                lane_glyph(*state).to_string(),
                view.lane_style(*state),
            ));
        }
        lines.push(Line::from(spans));
    }

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

/// Every step at once, with the current one marked.
///
/// The block above says what is happening *now*. This says what the whole run
/// does, which `h`/`l` otherwise answer only by paging blindly and
/// remembering — and remembering is the thing a reader of a divergence bug has
/// least to spare. Seeing that step 3 splits the warp and step 4 is the
/// barrier those lanes never reach is the entire explanation, and it fits on
/// one screen.
///
/// The lane column is a delta, exactly as the artifact stores it: how many
/// lanes changed state at that step and to what. That is the fact the prose
/// on the event line summarises, stated per step so the shape of the run is
/// visible without reading six sentences.
fn render_timeline(
    frame: &mut Frame<'_>,
    view: &WitnessView<'_>,
    witness: &WitnessArtifact,
    area: Rect,
) {
    if area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let position = view.state.position;
    let total = witness.steps.len();

    // Step 0 is the launch, so there are `total + 1` positions to show.
    let rows = area.height as usize;
    let first = window_start(position, total + 1, rows);
    let mut lines = Vec::with_capacity(rows);

    for index in first..(first + rows).min(total + 1) {
        let current = index == position;
        let marker = if current {
            if view.ascii { "> " } else { "\u{25b8} " }
        } else {
            "  "
        };
        let (statement, change) = match index.checked_sub(1).and_then(|i| witness.steps.get(i)) {
            None => ("launch".to_string(), String::new()),
            Some(step) => (step.statement.clone(), lane_delta(step)),
        };

        // The change column is right-aligned against the width so the shape of
        // the run reads down the edge; a narrow terminal drops it rather than
        // wrapping, since a wrapped timeline is no longer a timeline.
        let left = format!("{marker}{index:>2}  {statement}");
        let text = match width.checked_sub(change.chars().count() + 2) {
            Some(room) if !change.is_empty() && left.chars().count() < room => {
                let pad = room - left.chars().count();
                format!("{left}{}{change}", " ".repeat(pad))
            }
            _ => left,
        };

        let style = if current {
            view.accent(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            view.accent(Style::default().add_modifier(Modifier::DIM))
        };
        lines.push(Line::from(Span::styled(
            fit(&text, width, view.ascii),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Which slice of the timeline to show, keeping the current step visible and
/// the window still whenever the whole run already fits.
fn window_start(position: usize, count: usize, rows: usize) -> usize {
    if count <= rows {
        return 0;
    }
    // Centre the current step, then clamp so the last screen is full rather
    // than trailing off into blank rows.
    position.saturating_sub(rows / 2).min(count - rows)
}

/// How many lanes changed state at this step, and to what — the artifact's
/// own delta, counted.
fn lane_delta(step: &Step) -> String {
    let (mut active, mut waiting, mut exited) = (0usize, 0usize, 0usize);
    for change in &step.lane_changes {
        match change.state {
            LaneState::Active => active += 1,
            LaneState::Waiting => waiting += 1,
            LaneState::Exited => exited += 1,
        }
    }
    let mut parts = Vec::new();
    for (count, name) in [(waiting, "waiting"), (exited, "exited"), (active, "active")] {
        if count > 0 {
            parts.push(format!("{count} {name}"));
        }
    }
    parts.join(", ")
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

    /// The window keeps the current step on screen without moving when it
    /// does not have to — a timeline that scrolls under a reader who pressed
    /// `l` once is harder to follow than one that does not move at all.
    #[test]
    fn the_timeline_window_holds_still_until_it_must_scroll() {
        // Everything fits: never scroll, wherever the cursor is.
        for position in 0..6 {
            assert_eq!(window_start(position, 6, 10), 0, "position {position}");
        }
        // Exactly fits is still no scroll.
        assert_eq!(window_start(5, 6, 6), 0);

        // Longer than the window: centre the cursor…
        assert_eq!(window_start(10, 40, 6), 7);
        // …but never scroll past the end, so the last page is full rather
        // than trailing off into blank rows.
        assert_eq!(window_start(39, 40, 6), 34);
        assert_eq!(window_start(0, 40, 6), 0);
    }

    /// The delta column counts the artifact's own `lane_changes`, and says
    /// nothing when a step changed nothing — an empty column is quieter than
    /// "0 lanes" and means the same.
    #[test]
    fn the_delta_column_counts_what_the_step_changed() {
        use reconverge_artifacts::witness::LaneChange;

        let step = |changes: Vec<LaneChange>| Step {
            index: 0,
            statement: String::new(),
            span: None,
            lane_changes: changes,
            barrier: None,
            warp_op: None,
        };
        let change = |lane, state| LaneChange { lane, state };

        assert_eq!(lane_delta(&step(vec![])), "");
        assert_eq!(
            lane_delta(&step(vec![
                change(0, LaneState::Waiting),
                change(2, LaneState::Waiting),
            ])),
            "2 waiting"
        );
        // Order is fixed rather than following the artifact, so two runs of
        // the same shape read the same way.
        assert_eq!(
            lane_delta(&step(vec![
                change(1, LaneState::Exited),
                change(0, LaneState::Waiting),
            ])),
            "1 waiting, 1 exited"
        );
    }
}
