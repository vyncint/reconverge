//! rustc-style text rendering of findings.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use reconverge_artifacts::findings::{Confidence, Finding};
use unicode_width::UnicodeWidthChar;

use crate::review::{Item, Review};

/// Widest source line the snippet will print, in terminal cells.
///
/// Past this the line is trimmed around the span, rustc-style, with an
/// ellipsis on whichever side was cut. An 830-column source line used to
/// render as an 830-cell snippet row plus a caret row to match — on an 80x24
/// terminal that is ten wrapped rows each, and the diagnostic's own header
/// had scrolled off by the time the summary landed.
const MAX_SNIPPET_WIDTH: usize = 120;

/// Cells to keep before the span when a line has to be trimmed.
const TRIM_CONTEXT: usize = 20;

/// Tab stop used when expanding tabs in a printed source line.
///
/// Tabs are expanded rather than passed through so the rendered block does
/// not depend on the reader's terminal settings *or* on the gutter width —
/// a tab-indented line drifted by a different amount at each line-number
/// width, which is what makes the expansion the testable choice.
const TAB_WIDTH: usize = 4;

/// Print findings in rustc-like text form to stdout, honoring `--strict`
/// and `--show-suppressed`, then a summary line.
///
/// `workspace_root` anchors the source snippets: spans are stored relative
/// to it, so it — not the process cwd — is where their files are read from.
///
/// # Errors
///
/// Propagates the io error from writing to stdout. A reader that closed
/// early surfaces as [`io::ErrorKind::BrokenPipe`]; [`crate::out::finish`]
/// is what decides that it is not a failure.
pub fn render_text(
    review: &Review,
    workspace_root: &Path,
    strict: bool,
    show_suppressed: bool,
) -> io::Result<()> {
    // One locked handle for the whole report: `println!` panics on a closed
    // reader, and the lock also spares us a re-lock per line.
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for item in review.items() {
        if !item.shown(strict, show_suppressed) {
            continue;
        }
        out.write_all(render_item(&item, workspace_root).as_bytes())?;
        out.write_all(b"\n")?;
    }
    writeln!(
        out,
        "{}",
        review.counts().summary_line(strict, show_suppressed)
    )?;
    out.flush()
}

fn render_item(item: &Item<'_>, workspace_root: &Path) -> String {
    let finding = item.finding;
    let mut out = String::new();
    let severity = match (item.suppression, finding.confidence) {
        // A suppressed finding is neither an error nor a warning: it is a
        // decision, and it reads like one.
        (Some(_), _) => "suppressed",
        (None, Confidence::Deny | Confidence::Confirmed) => "error",
        (None, Confidence::Warning) => "warning",
    };
    // rustc indents the whole block by the width of the line number, so a
    // three-digit line and a one-digit line both line up under their own
    // gutter. Computed once here and threaded through, rather than pinned at
    // the width that happens to be right for a two-digit line.
    let pad = " ".repeat(finding.span.line_start.to_string().len());
    out.push_str(&format!(
        "{severity}[{}]: {}\n",
        finding.code, finding.message
    ));
    out.push_str(&format!(
        "{pad}--> {}:{}:{}\n",
        finding.span.file, finding.span.line_start, finding.span.column_start
    ));
    if let Some(snippet) = source_snippet(finding, workspace_root, &pad) {
        out.push_str(&snippet);
    }
    if let Some(entry) = item.suppression {
        out.push_str(&format!("{pad} = baseline: {}\n", entry.reason));
    }
    for note in &finding.notes {
        out.push_str(&format!("{pad} = note: {note}\n"));
    }
    for step in &finding.provenance {
        out.push_str(&format!(
            "{pad} = provenance: {} ({}:{})\n",
            step.what, step.span.file, step.span.line_start
        ));
    }
    if let Some(help) = &finding.help {
        out.push_str(&format!("{pad} = help: {help}\n"));
    }
    out
}

/// The offending source line with a caret run, single-line spans only.
/// Rendering is best-effort: an unreadable file just omits the snippet.
///
/// The span's file is workspace-root-relative, so it is resolved against
/// `workspace_root` rather than the process cwd — otherwise running from a
/// member directory reads the wrong path and silently drops every snippet.
///
/// The line is *prepared* before it is printed: this is foreign text, and
/// the snippet is the only thing standing between the analyzed file's bytes
/// and the reader's terminal.
fn source_snippet(finding: &Finding, workspace_root: &Path, pad: &str) -> Option<String> {
    let span = &finding.span;
    if span.line_start != span.line_end {
        return None;
    }
    let text = fs::read_to_string(workspace_root.join(&span.file)).ok()?;
    let raw = text.lines().nth(span.line_start - 1)?;
    let prepared = PreparedLine::new(raw, span.column_start, span.column_end);
    let gutter = span.line_start.to_string();
    let caret_offset = " ".repeat(prepared.caret_col);
    let carets = "^".repeat(prepared.caret_width);
    Some(format!(
        "{pad} |\n{gutter} | {}\n{pad} | {caret_offset}{carets}\n",
        prepared.text
    ))
}

/// A source line ready for a terminal, with the span located in cells.
///
/// Three things happen here, and each closes a way the old rendering could
/// mislead:
///
/// * **Control bytes are made visible.** A comment holding two real `ESC`
///   bytes repainted the terminal from the analyzed file, erasing the
///   diagnostics printed above it while the summary went on counting them.
///   Nothing below `0x20`, no `DEL` and no C1 byte reaches stdout now.
/// * **Tabs are expanded**, so the block does not depend on the reader's
///   tab stops or on the gutter width.
/// * **The caret is measured in cells, not characters.** A terminal
///   advances two cells for a wide character and to the next tab stop for a
///   tab, so a character count put the caret under the wrong column on any
///   line with either before the span — and gave a four-character CJK
///   parameter four carets for the eight cells it occupies.
///
/// The span itself is untouched: `findings.v1`, SARIF and the baseline keep
/// character columns, by rustc convention, so every consumer keyed on them
/// is unaffected.
struct PreparedLine {
    text: String,
    /// Cells before the first caret.
    caret_col: usize,
    /// Cells the caret run covers.
    caret_width: usize,
}

impl PreparedLine {
    fn new(raw: &str, column_start: usize, column_end: usize) -> PreparedLine {
        // One entry per source character, so the span's character columns
        // can be converted to cells and the line trimmed on a character
        // boundary rather than inside a glyph.
        let mut pieces: Vec<(String, usize)> = Vec::new();
        let mut width = 0usize;
        // Cells before each source character, indexed the way the span's
        // 1-based columns are; one extra entry so a span ending at the end
        // of the line has somewhere to land.
        let mut columns: Vec<usize> = Vec::with_capacity(raw.chars().count() + 1);
        for ch in raw.chars() {
            columns.push(width);
            let (glyph, cells) = match ch {
                '\t' => {
                    let stop = (width / TAB_WIDTH + 1) * TAB_WIDTH;
                    (" ".repeat(stop - width), stop - width)
                }
                // C0 and DEL become their Unicode control pictures: one cell
                // each, so the caret arithmetic is untouched, and visible, so
                // `ESC` reads as `␛` rather than acting.
                '\u{0}'..='\u{1f}' => (control_picture(ch), 1),
                '\u{7f}' => ('\u{2421}'.to_string(), 1),
                // C1 has no picture block; the replacement character says
                // "a byte was here" without pretending to name it.
                '\u{80}'..='\u{9f}' => ('\u{fffd}'.to_string(), 1),
                _ => {
                    // Zero-width characters (combining marks) contribute no
                    // cells but are kept, exactly as the terminal draws them.
                    let cells = ch.width().unwrap_or(0);
                    (ch.to_string(), cells)
                }
            };
            width += cells;
            pieces.push((glyph, cells));
        }
        columns.push(width);

        // A span column is 1-based and `column_end` is exclusive, so both
        // index `columns` directly after the 1-based-to-0-based shift. A
        // column past the end of the line clamps to its end rather than
        // panicking on a span that disagrees with the file on disk.
        let at = |column: usize| columns[column.saturating_sub(1).min(columns.len() - 1)];
        let caret_col = at(column_start);
        let caret_width = at(column_end).saturating_sub(caret_col).max(1);

        if width <= MAX_SNIPPET_WIDTH {
            let text = pieces.into_iter().map(|(glyph, _)| glyph).collect();
            return PreparedLine {
                text,
                caret_col,
                caret_width,
            };
        }
        Self::trimmed(&pieces, caret_col, caret_width, width)
    }

    /// A window of at most [`MAX_SNIPPET_WIDTH`] cells around the span, with
    /// an ellipsis marking whichever side was cut.
    fn trimmed(
        pieces: &[(String, usize)],
        caret_col: usize,
        caret_width: usize,
        width: usize,
    ) -> PreparedLine {
        const ELLIPSIS: &str = "...";
        // Keep a little context before the span, and slide back from the end
        // of the line when the span sits near it, so the window is always
        // full.
        let start_cell = caret_col
            .saturating_sub(TRIM_CONTEXT)
            .min(width.saturating_sub(MAX_SNIPPET_WIDTH));
        let end_cell = start_cell + MAX_SNIPPET_WIDTH;

        let mut text = String::new();
        let mut cell = 0usize;
        let mut kept_start = None;
        for (glyph, cells) in pieces {
            let next = cell + cells;
            // A character is kept when it lies wholly inside the window; a
            // wide one straddling an edge is dropped rather than half drawn.
            if cell >= start_cell && next <= end_cell {
                if kept_start.is_none() {
                    kept_start = Some(cell);
                }
                text.push_str(glyph);
            }
            cell = next;
        }
        let kept_start = kept_start.unwrap_or(start_cell);

        let head = if kept_start > 0 { ELLIPSIS } else { "" };
        let tail = if end_cell < width { ELLIPSIS } else { "" };
        PreparedLine {
            // The caret moves with the window, and past the leading
            // ellipsis, so it still sits under the text it names.
            caret_col: caret_col.saturating_sub(kept_start) + head.len(),
            // A span running off the trimmed edge underlines what is shown.
            caret_width: caret_width.min(MAX_SNIPPET_WIDTH),
            text: format!("{head}{text}{tail}"),
        }
    }
}

/// The Unicode control picture for a C0 byte: `ESC` becomes `␛`.
fn control_picture(ch: char) -> String {
    char::from_u32(0x2400 + ch as u32)
        .unwrap_or('\u{fffd}')
        .to_string()
}

#[cfg(test)]
mod tests {
    use reconverge_artifacts::baseline::Entry;
    use reconverge_artifacts::findings::SourceSpan;

    use super::*;

    fn finding() -> Finding {
        Finding {
            code: "RC003".into(),
            confidence: Confidence::Deny,
            message: "kernel `scale` takes `&mut [f32]` as a parameter".into(),
            kernel: Some("scale".into()),
            span: SourceSpan {
                file: "no/such/file.rs".into(),
                line_start: 7,
                column_start: 1,
                line_end: 7,
                column_end: 9,
            },
            notes: vec!["every thread gets the same exclusive reference".into()],
            help: Some("use `DisjointSlice<f32>`".into()),
            explain: "RC003".into(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn suppressed_findings_render_as_decisions_with_their_reason() {
        let finding = finding();
        let entry = Entry {
            krate: "k".into(),
            kernel: Some("scale".into()),
            code: "RC003".into(),
            reason: "reviewed: the host half owns this buffer".into(),
        };

        let open = render_item(
            &Item {
                finding: &finding,
                suppression: None,
            },
            Path::new("."),
        );
        assert!(open.starts_with("error[RC003]: "), "{open}");
        assert!(!open.contains("= baseline:"));

        let suppressed = render_item(
            &Item {
                finding: &finding,
                suppression: Some(&entry),
            },
            Path::new("."),
        );
        assert!(
            suppressed.starts_with("suppressed[RC003]: "),
            "{suppressed}"
        );
        assert!(
            suppressed.contains(" = baseline: reviewed: the host half owns this buffer\n"),
            "{suppressed}"
        );
        // The rest of the diagnostic is unchanged: a suppression hides the
        // finding by default, it never edits it.
        assert!(suppressed.contains("= note: every thread gets"));
        assert!(suppressed.contains("= help: use `DisjointSlice<f32>`"));
    }

    /// Write `line` to a temp file and render a finding whose span points at
    /// it, so `source_snippet` runs against a real file rather than the
    /// `no/such/file.rs` fixture that made it return `None` and left the
    /// whole snippet path untested.
    fn rendered(line: &str, line_no: usize, column_start: usize, column_end: usize) -> String {
        // A counter, not the span: several cases share a line and a column,
        // and the suite runs them in parallel — two tests sharing a scratch
        // directory means one deletes the file the other is reading.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "reconverge-render-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        // Pad the file so `line_no` exists.
        let mut text = String::new();
        for _ in 1..line_no {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
        fs::write(dir.join("src/lib.rs"), text).unwrap();

        let mut finding = finding();
        finding.span = SourceSpan {
            file: "src/lib.rs".into(),
            line_start: line_no,
            column_start,
            line_end: line_no,
            column_end,
        };
        let out = render_item(
            &Item {
                finding: &finding,
                suppression: None,
            },
            &dir,
        );
        let _ = fs::remove_dir_all(&dir);
        out
    }

    /// The whole block, byte for byte. `contains` assertions are what let the
    /// gutter and the caret drift: both are pure layout.
    #[test]
    fn the_rendered_block_is_aligned_under_its_own_gutter() {
        // Written out with no line continuations: `\<newline>` eats the
        // leading spaces, which is exactly the layout under test.
        let expected = concat!(
            "error[RC003]: kernel `scale` takes `&mut [f32]` as a parameter\n",
            " --> src/lib.rs:6:12\n",
            "  |\n",
            "6 |     if x { sync(); }\n",
            "  |            ^^^^^^\n",
            "  = note: every thread gets the same exclusive reference\n",
            "  = help: use `DisjointSlice<f32>`\n",
        );
        assert_eq!(rendered("    if x { sync(); }", 6, 12, 18), expected);
    }

    /// The same source line at four line numbers: only the gutter moves, and
    /// the caret stays under the span. Before, the `= ` lines were pinned at
    /// three spaces — right for a two-digit line and wrong everywhere else.
    #[test]
    fn the_frame_does_not_drift_with_the_line_numbers_digit_count() {
        for (line_no, pad) in [(6, " "), (22, "  "), (100, "   "), (1001, "    ")] {
            let out = rendered("\tif x { sync(); }", line_no, 9, 15);
            assert!(
                out.contains(&format!("\n{pad}--> src/lib.rs:{line_no}:9\n")),
                "line {line_no}: {out}"
            );
            assert!(
                out.contains(&format!("\n{pad} = note: ")),
                "line {line_no}: {out}"
            );
            // The tab expands to four cells, so the span starts at cell 11
            // whatever the gutter is.
            assert!(
                out.contains(&format!("\n{pad} | {}^^^^^^\n", " ".repeat(11))),
                "line {line_no}: {out}"
            );
        }
    }

    /// A terminal advances two cells for a wide character, so a character
    /// count put the caret eight cells left of the span it names.
    #[test]
    fn the_caret_is_measured_in_cells_not_characters() {
        // Eight wide characters and a space are 17 cells, not 9 characters.
        let out = rendered("幅幅幅幅幅幅幅幅 sync();", 6, 10, 16);
        let caret_row = out.lines().find(|l| l.contains('^')).expect("a caret row");
        assert_eq!(caret_row, format!("  | {}^^^^^^", " ".repeat(17)), "{out}");

        // And the run is as wide as what it underlines: four wide characters
        // are eight cells, so eight carets rather than four.
        let out = rendered("pub fn k(幅幅幅幅: &mut [f32]) {}", 6, 10, 14);
        let caret_row = out.lines().find(|l| l.contains('^')).expect("a caret row");
        assert_eq!(caret_row, format!("  | {}^^^^^^^^", " ".repeat(9)), "{out}");
    }

    /// A comment holding two real `ESC` bytes used to repaint the terminal
    /// from the analyzed file, erasing every diagnostic above it.
    #[test]
    fn control_bytes_in_the_source_are_shown_rather_than_obeyed() {
        let out = rendered("    if x { sync(); } // \u{1b}[2J\u{1b}[H", 6, 12, 18);
        assert!(!out.contains('\u{1b}'), "an ESC reached stdout: {out:?}");
        assert!(out.contains("␛[2J␛[H"), "{out}");
        // Every C0 byte, DEL and C1 byte, not just ESC.
        let out = rendered("a\u{0}b\u{7}c\u{7f}d\u{9b}e", 6, 1, 2);
        assert!(
            !out.lines().any(|l| l.chars().any(|c| (c < ' ' && c != '\t')
                || c == '\u{7f}'
                || ('\u{80}'..='\u{9f}').contains(&c))),
            "{out:?}"
        );
        assert!(out.contains("a␀b␇c␡d\u{fffd}e"), "{out}");
    }

    /// An 830-column line rendered as an 830-cell row plus a caret row to
    /// match; on an 80x24 terminal the diagnostic's own header had scrolled
    /// off before the summary landed.
    #[test]
    fn an_over_long_line_is_trimmed_around_its_span() {
        let long = format!("{}sync();{}", "x".repeat(400), "y".repeat(400));
        let out = rendered(&long, 6, 401, 407);
        let snippet = out
            .lines()
            .find(|l| l.starts_with("6 | "))
            .expect("a snippet row");
        let caret_row = out.lines().find(|l| l.contains('^')).expect("a caret row");
        // Both rows fit the bound: the window, two ellipses and the gutter.
        assert!(
            snippet.chars().count() <= MAX_SNIPPET_WIDTH + 10,
            "{} cells: {snippet}",
            snippet.chars().count()
        );
        assert!(
            caret_row.chars().count() <= MAX_SNIPPET_WIDTH + 10,
            "{caret_row}"
        );
        // Cut on both sides, with the span still in view.
        assert!(snippet.contains("...x"), "{snippet}");
        assert!(snippet.ends_with("..."), "{snippet}");
        assert!(snippet.contains("sync();"), "{snippet}");
        // `6 | ` and `  | ` are the same width, so a column in one row is the
        // same column in the other: the caret sits under the span.
        let carets = caret_row.find('^').expect("carets");
        let under: String = snippet.chars().skip(carets).take(6).collect();
        assert_eq!(under, "sync()", "the caret must sit under the span:\n{out}");
    }

    /// A line that fits is printed whole, so the ordinary case is untouched.
    #[test]
    fn a_line_within_the_bound_is_not_trimmed() {
        let out = rendered("    if x { sync(); }", 6, 12, 18);
        assert!(!out.contains("..."), "{out}");
    }
}
