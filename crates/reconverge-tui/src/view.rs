//! Pure view code: (model, area) → widgets. No I/O, no clock, no PID —
//! any frame is a function of the loaded artifacts alone, which is what
//! makes golden-frame testing reliable.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::load::LoadedArtifact;

/// Everything the shell renders. State = f(artifacts, key sequence);
/// the shell has no keys beyond quit, so state = f(artifacts).
#[derive(Debug, Clone, Default)]
pub struct ShellModel {
    /// Every artifact loaded for this run.
    pub artifacts: Vec<LoadedArtifact>,
    /// Load failures, shown in-frame (deterministically) rather than on
    /// stderr where they would tear the alternate screen.
    pub errors: Vec<String>,
    /// `--ascii`: box-drawing fallback for terminals without the glyphs.
    pub ascii: bool,
    /// False when `NO_COLOR` is set (any non-empty value).
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

/// The shell's body content as plain rows — the unit-testable layer under
/// the widget tree.
#[must_use]
pub fn shell_rows(model: &ShellModel) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if model.artifacts.is_empty() && model.errors.is_empty() {
        rows.push((String::new(), "no artifacts loaded".to_string()));
        rows.push((
            String::new(),
            "usage: reconverge-tui [--ascii] <artifact.json>...".to_string(),
        ));
    }
    for artifact in &model.artifacts {
        rows.push((
            artifact.schema.clone(),
            format!("{}  {}", artifact.name, artifact.summary),
        ));
    }
    for error in &model.errors {
        rows.push(("error".to_string(), error.clone()));
    }
    rows
}

/// Grapheme-safe truncation to a display width, with an ellipsis that
/// matches the glyph budget (`…` normally, `...` under `--ascii`). In
/// ASCII mode the content itself is transliterated first, so no non-ASCII
/// glyph the shell draws survives.
#[must_use]
pub fn fit(s: &str, width: usize, ascii: bool) -> String {
    let s = if ascii { asciify(s) } else { s.to_string() };
    if s.width() <= width {
        return s;
    }
    let ellipsis = if ascii { "..." } else { "\u{2026}" };
    let budget = width.saturating_sub(ellipsis.width());
    let mut out = String::new();
    let mut used = 0;
    for grapheme in s.graphemes(true) {
        let w = grapheme.width();
        if used + w > budget {
            break;
        }
        out.push_str(grapheme);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

/// Greedy word wrap to a display width, for text that must arrive whole
/// rather than end in [`fit`]'s ellipsis: breaks between words, and a word
/// wider than a line is split across lines rather than cut. Transliterated
/// under `--ascii` first, like [`fit`], so the widths are the drawn ones.
/// Always at least one line, so an empty text still holds its row.
#[must_use]
pub fn wrap(s: &str, width: usize, ascii: bool) -> Vec<String> {
    let s = if ascii { asciify(s) } else { s.to_string() };
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in s.split_whitespace() {
        let w = word.width();
        if used > 0 && used + 1 + w <= width {
            line.push(' ');
            line.push_str(word);
            used += 1 + w;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        for grapheme in word.graphemes(true) {
            let g = grapheme.width();
            if used > 0 && used + g > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push_str(grapheme);
            used += g;
        }
    }
    if used > 0 || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `--ascii` transliteration: the shell's own punctuation gets a faithful
/// spelling, anything else non-ASCII degrades to `?` rather than leaking.
fn asciify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2014}' => out.push_str("--"), // em dash
            '\u{00b7}' => out.push('*'),      // middle dot
            '\u{2026}' => out.push_str("..."),
            c if c.is_ascii() => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

/// Draw the whole shell.
pub fn render(frame: &mut Frame<'_>, model: &ShellModel) {
    let area = frame.area();
    let accent = |style: Style| {
        if model.color { style } else { Style::default() }
    };

    let block = Block::bordered()
        .border_set(if model.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        })
        .title(Span::styled(
            " reconverge ",
            accent(Style::default().add_modifier(Modifier::BOLD)),
        ))
        .title_bottom(Span::styled(
            " q quit ",
            accent(Style::default().add_modifier(Modifier::DIM)),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let mut lines = vec![
        Line::from(fit("artifact shell", width, model.ascii)),
        Line::default(),
    ];
    for (tag, text) in shell_rows(model) {
        if tag.is_empty() {
            lines.push(Line::from(fit(&text, width, model.ascii)));
            continue;
        }
        let tag_style = accent(Style::default().fg(if tag == "error" {
            Color::Red
        } else {
            Color::Cyan
        }));
        let body = fit(&text, width.saturating_sub(tag.len() + 2), model.ascii);
        lines.push(Line::from(vec![
            Span::styled(format!("{tag}  "), tag_style),
            Span::raw(body),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(fit(
        "views: inspect · witness · learn · triage",
        width,
        model.ascii,
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::LoadedArtifact;

    #[test]
    fn empty_model_shows_usage() {
        let rows = shell_rows(&ShellModel::default());
        assert_eq!(rows[0].1, "no artifacts loaded");
        assert!(rows[1].1.starts_with("usage:"));
    }

    #[test]
    fn artifacts_and_errors_become_tagged_rows() {
        let model = ShellModel {
            artifacts: vec![LoadedArtifact {
                name: "a.json".into(),
                schema: "findings.v1".into(),
                summary: "crate k — 0 findings: 0 deny, 0 confirmed, 0 warning".into(),
            }],
            errors: vec!["b.json: not JSON: oops".into()],
            ascii: false,
            color: true,
        };
        let rows = shell_rows(&model);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "findings.v1");
        assert!(rows[0].1.starts_with("a.json  "));
        assert_eq!(rows[1].0, "error");
    }

    #[test]
    fn ascii_mode_transliterates_content() {
        assert_eq!(fit("a — b · c…", 40, true), "a -- b * c...");
        assert_eq!(fit("ph\u{e2}n k\u{1ef3}", 40, true), "ph?n k?");
    }

    #[test]
    fn wrap_breaks_between_words_and_never_drops_text() {
        assert_eq!(wrap("", 10, false), vec![""]);
        assert_eq!(wrap("short", 10, false), vec!["short"]);
        // Exactly the width stays on one line.
        assert_eq!(wrap("12345 789", 9, false), vec!["12345 789"]);
        assert_eq!(wrap("12345 7890", 9, false), vec!["12345", "7890"]);
        // A word wider than the line is split, not cut.
        assert_eq!(
            wrap("abcdefgh ij", 3, false),
            vec!["abc", "def", "gh", "ij"]
        );
        // Widths are display widths, after transliteration.
        assert_eq!(wrap("a \u{2014} b", 4, true), vec!["a --", "b"]);
        assert_eq!(
            wrap("\u{6f22}\u{5b57} ab", 4, false),
            vec!["\u{6f22}\u{5b57}", "ab"]
        );
        assert_eq!(wrap("anything", 0, false).concat(), "anything");
    }

    #[test]
    fn fit_is_grapheme_safe_and_width_aware() {
        assert_eq!(fit("hello", 10, false), "hello");
        assert_eq!(fit("hello world", 6, false), "hello\u{2026}");
        assert_eq!(fit("hello world", 6, true), "hel...");
        // Text with combining-capable letters truncates on grapheme
        // boundaries, never mid-cluster.
        let vi = "ph\u{e2}n k\u{1ef3} d\u{1ecb}"; // "phân kỳ dị"
        let cut = fit(vi, 6, false);
        assert!(cut.width() <= 6, "{cut:?} is wider than 6");
        assert!(cut.ends_with('\u{2026}'));
    }
}
