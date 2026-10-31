//! Learn-mode rendering: (lessons, state, area) → widgets. No I/O, no
//! clock; every frame is a pure function of (embedded lessons, key
//! sequence) — nothing is read from disk or network at draw time.
//!
//! The replay panel reuses the witness debugger's primitives (glyphs,
//! strips, progress bar), so what a lesson animates is
//! exactly what `cargo reconverge witness` shows on real findings.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use reconverge_artifacts::witness::{VerdictKind, WitnessArtifact};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::lessons::{Lesson, Page};
use super::state::{LearnState, Screen};
use crate::view::{fit, wrap};
use crate::witness::view as wview;

/// Everything one frame of the learn view needs.
pub struct LearnView<'a> {
    /// The lessons.
    pub lessons: &'a [Lesson],
    /// The cursor.
    pub state: &'a LearnState,
    /// Draw with ASCII only — no box drawing or block glyphs.
    pub ascii: bool,
    /// Use colour; false under `NO_COLOR` or when the terminal has none.
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

/// Keys shown along the bottom border, per screen.
const LIST_KEYS: &str = " j/k select  Enter open  q quit ";
const PAGE_KEYS: &str = " n/p page  h/l step  d/v jumps  Esc list  q quit ";

/// The one-line introduction above the lesson list.
const INTRO: &str = "four interactive lessons, replayed from recorded witnesses — no GPU required";

impl LearnView<'_> {
    fn accent(&self, style: Style) -> Style {
        if self.color { style } else { Style::default() }
    }
}

/// Draw one frame of the learn view.
pub fn render(frame: &mut Frame<'_>, view: &LearnView<'_>) {
    let area = frame.area();
    let keys = match view.state.screen {
        Screen::List => LIST_KEYS,
        Screen::Page => PAGE_KEYS,
    };
    let block = Block::bordered()
        .border_set(if view.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        })
        .title(Span::styled(
            " reconverge learn ",
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        ))
        .title_bottom(Span::styled(
            fit(keys, area.width.saturating_sub(2) as usize, view.ascii),
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match view.state.screen {
        Screen::List => render_list(frame, view, inner),
        Screen::Page => render_page(frame, view, inner),
    }
}

fn render_list(frame: &mut Frame<'_>, view: &LearnView<'_>, area: Rect) {
    let width = area.width as usize;
    let mut lines = vec![
        Line::from(Span::styled(
            fit(INTRO, width, view.ascii),
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        )),
        Line::default(),
    ];
    for (i, lesson) in view.lessons.iter().enumerate() {
        let marker = if i == view.state.lesson { '>' } else { ' ' };
        let row = fit(
            &format!("{marker} {}. {}", i + 1, lesson.title()),
            width,
            view.ascii,
        );
        let style = if i == view.state.lesson {
            view.accent(Style::default().add_modifier(Modifier::BOLD))
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_page(frame: &mut Frame<'_>, view: &LearnView<'_>, area: Rect) {
    let lesson = &view.lessons[view.state.lesson];
    let page = &lesson.pages[view.state.page];
    // Build the replay panel first, then size it to the rows its lines take:
    // every line is one row, the verdict already wrapped into as many as it
    // needs, so a long verdict is read in full instead of being cut at the
    // panel's edge — capped at the box so it cannot spill over.
    let mut replay = page
        .witness
        .as_ref()
        .map(|witness| replay_lines(view, witness, area.width as usize))
        .unwrap_or_default();
    let text = text_lines(view, lesson, page, area.width as usize);
    // The blank that opens the replay panel is the first row to give up when
    // the page is a row short: at 80x24 two pages lost the last line of their
    // kernel excerpt to it — in `reconvergence`, the `sync_threads()` the
    // page is about — without a mark.
    if !replay.is_empty() && text.len() + replay.len() > usize::from(area.height) {
        replay.remove(0);
    }
    let replay_rows = u16::try_from(replay.len())
        .unwrap_or(u16::MAX)
        .min(area.height);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(replay_rows)])
        .split(area);

    render_text(frame, view, text, rows[0]);
    frame.render_widget(Paragraph::new(replay), rows[1]);
}

/// A lesson's prose at `width`: each paragraph keeps the line breaks it was
/// written with while they fit, and is reflowed as a whole when one does not.
///
/// Cutting each authored line at the edge, as this did, lost the end of
/// every line at 46 columns ("the mask promis…"), which is most of a lesson.
/// Reflowing always would rewrap the prose at 120 columns into lines too long
/// to read; the authored breaks are the better layout wherever they fit.
fn prose_lines(body: &str, width: usize, ascii: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (i, paragraph) in body.split("\n\n").enumerate() {
        if i > 0 {
            out.push(String::new());
        }
        if paragraph
            .lines()
            .all(|line| line_width(line, ascii) <= width)
        {
            out.extend(paragraph.lines().map(|line| fit(line, width, ascii)));
        } else {
            out.extend(wrap(paragraph, width, ascii));
        }
    }
    out
}

/// The drawn width of `line`, after `--ascii` transliteration.
fn line_width(line: &str, ascii: bool) -> usize {
    fit(line, usize::MAX, ascii).width()
}

/// `fit`, but for a strip built from many styled spans — the lanes row, where
/// each lane carries its own state colour and so cannot be one `&str` through
/// `fit`. Clips the spans to `width` display columns, grapheme-safe, with the
/// same ellipsis `fit` uses, keeping each span's style. This keeps the strip a
/// single row (so it never pushes the verdict off the panel) and
/// column-aligned with the fitted strips above it.
fn fit_spans(spans: Vec<Span<'static>>, width: usize, ascii: bool) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|span| span.content.as_ref().width()).sum();
    if total <= width {
        return spans;
    }
    let ellipsis = if ascii { "..." } else { "\u{2026}" };
    let budget = width.saturating_sub(ellipsis.width());
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= budget {
            break;
        }
        let mut kept = String::new();
        for grapheme in span.content.as_ref().graphemes(true) {
            let w = grapheme.width();
            if used + w > budget {
                break;
            }
            kept.push_str(grapheme);
            used += w;
        }
        if !kept.is_empty() {
            out.push(Span::styled(kept, span.style));
        }
    }
    out.push(Span::raw(ellipsis));
    out
}

/// The page's text column: heading, prose, and the kernel excerpt.
fn text_lines(
    view: &LearnView<'_>,
    lesson: &Lesson,
    page: &Page,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            fit(
                &format!(
                    "lesson {}/{} — {} — page {}/{}",
                    view.state.lesson + 1,
                    view.lessons.len(),
                    lesson.title(),
                    view.state.page + 1,
                    lesson.pages.len(),
                ),
                width,
                view.ascii,
            ),
            view.accent(Style::default().add_modifier(Modifier::BOLD)),
        )),
        Line::default(),
    ];
    lines.extend(
        prose_lines(page.body(), width, view.ascii)
            .into_iter()
            .map(Line::from),
    );
    if let Some(code) = page.code {
        lines.push(Line::default());
        for text in code.lines() {
            lines.push(Line::from(Span::styled(
                fit(&format!("    {text}"), width, view.ascii),
                view.accent(Style::default().fg(Color::Cyan)),
            )));
        }
    }
    lines
}

fn render_text(frame: &mut Frame<'_>, view: &LearnView<'_>, mut lines: Vec<Line<'_>>, area: Rect) {
    // Prose that wraps can outgrow a short terminal. Say so on the last row
    // rather than letting the page end mid-paragraph as if that were all.
    let height = area.height as usize;
    if lines.len() > height && height > 0 {
        lines.truncate(height - 1);
        lines.push(Line::from(Span::styled(
            fit(
                "\u{2026} more below: enlarge the terminal",
                area.width as usize,
                view.ascii,
            ),
            view.accent(Style::default().add_modifier(Modifier::DIM)),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The compact replay panel's lines: the debugger's lane strip, mask panel,
/// and event line, driven by the page's embedded witness at the lesson's
/// current position. Every line is one row; the verdict, the one text
/// allowed to take several, arrives already wrapped.
fn replay_lines(
    view: &LearnView<'_>,
    witness: &WitnessArtifact,
    width: usize,
) -> Vec<Line<'static>> {
    let position = view.state.position;
    let total = witness.steps.len();
    let executed = position.checked_sub(1).and_then(|i| witness.steps.get(i));

    let mut lines = vec![Line::default()];
    lines.push(Line::from(Span::styled(
        fit(
            &format!(
                "replay — step {position}/{total}  {}",
                wview::progress_bar(total, position)
            ),
            width,
            view.ascii,
        ),
        view.accent(Style::default().fg(Color::Cyan)),
    )));
    lines.push(Line::from(Span::styled(
        fit(
            &format!("{}{}", wview::label_cell(""), wview::lane_indices()),
            width,
            view.ascii,
        ),
        view.accent(Style::default().add_modifier(Modifier::DIM)),
    )));

    let states = witness.lane_states_at(position.checked_sub(1));
    let mut spans = vec![Span::raw(wview::label_cell("lanes"))];
    for (lane, state) in states.iter().enumerate() {
        if lane > 0 && lane % 8 == 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            wview::lane_glyph(*state).to_string(),
            view.accent(wview::lane_state_style(*state)),
        ));
    }
    lines.push(Line::from(fit_spans(spans, width, view.ascii)));

    // Mask and active strips at a collective step; otherwise the legend.
    match executed.and_then(|s| s.warp_op.as_ref()) {
        Some(op) => {
            let mask_text = wview::parse_mask(&op.mask).map_or(op.mask.clone(), wview::mask_strip);
            let active_text =
                wview::parse_mask(&op.active).map_or(op.active.clone(), wview::mask_strip);
            lines.push(Line::from(Span::styled(
                fit(
                    &format!("{}{mask_text}  {}", wview::label_cell("mask"), op.mask),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().fg(Color::Magenta)),
            )));
            lines.push(Line::from(Span::styled(
                fit(
                    &format!(
                        "{}{active_text}  {}",
                        wview::label_cell("active"),
                        op.active
                    ),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().fg(Color::Green)),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                fit(
                    &format!("{}o active   W waiting   . exited", wview::label_cell("")),
                    width,
                    view.ascii,
                ),
                view.accent(Style::default().add_modifier(Modifier::DIM)),
            )));
            lines.push(Line::default());
        }
    }

    // The event just executed, then its consequence (or the verdict, once
    // the timeline reaches it).
    match executed {
        None => lines.push(Line::from(fit(
            "launch — before the first event",
            width,
            view.ascii,
        ))),
        Some(step) => lines.push(Line::from(fit(&step.statement, width, view.ascii))),
    }
    let reached = witness
        .verdict
        .step
        .is_none_or(|s| position > s.min(total.saturating_sub(1)));
    if reached {
        let verdict_style = view.accent(match witness.verdict.kind {
            VerdictKind::Completed => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            _ => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        });
        // The verdict is the line the lesson exists to deliver, so it wraps
        // rather than truncates, into the rows the panel is sized for above.
        let verdict = format!(
            "verdict: {} — {}",
            wview::verdict_word(witness.verdict.kind),
            witness.verdict.message
        );
        lines.extend(
            wrap(&verdict, width, view.ascii)
                .into_iter()
                .map(|text| Line::from(Span::styled(text, verdict_style))),
        );
    } else if let Some(barrier) = executed.and_then(|s| s.barrier.as_ref()) {
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
    } else if let Some(op) = executed.and_then(|s| s.warp_op.as_ref())
        && let (Some(mask), Some(active)) =
            (wview::parse_mask(&op.mask), wview::parse_mask(&op.active))
    {
        lines.push(Line::from(Span::styled(
            fit(
                &format!(
                    "{} — named in the mask but not active: {:#010x}",
                    op.op,
                    mask & !active
                ),
                width,
                view.ascii,
            ),
            view.accent(Style::default().fg(Color::Yellow)),
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_line_matches_the_screen() {
        assert!(LIST_KEYS.contains("Enter open"));
        assert!(PAGE_KEYS.contains("n/p page") && PAGE_KEYS.contains("h/l step"));
    }
}
