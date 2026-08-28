//! rustc-style text rendering of findings.

use std::fs;
use std::path::Path;

use reconverge_artifacts::findings::{Confidence, Finding};

use crate::review::{Item, Review};

/// Print findings in rustc-like text form to stdout, honoring `--strict`
/// and `--show-suppressed`, then a summary line.
///
/// `workspace_root` anchors the source snippets: spans are stored relative
/// to it, so it — not the process cwd — is where their files are read from.
pub fn render_text(review: &Review, workspace_root: &Path, strict: bool, show_suppressed: bool) {
    for item in review.items() {
        if !item.shown(strict, show_suppressed) {
            continue;
        }
        print!("{}", render_item(&item, workspace_root));
        println!();
    }
    println!("{}", review.counts().summary_line(strict));
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
    out.push_str(&format!(
        "{severity}[{}]: {}\n",
        finding.code, finding.message
    ));
    out.push_str(&format!(
        "  --> {}:{}:{}\n",
        finding.span.file, finding.span.line_start, finding.span.column_start
    ));
    if let Some(snippet) = source_snippet(finding, workspace_root) {
        out.push_str(&snippet);
    }
    if let Some(entry) = item.suppression {
        out.push_str(&format!("   = baseline: {}\n", entry.reason));
    }
    for note in &finding.notes {
        out.push_str(&format!("   = note: {note}\n"));
    }
    for step in &finding.provenance {
        out.push_str(&format!(
            "   = provenance: {} ({}:{})\n",
            step.what, step.span.file, step.span.line_start
        ));
    }
    if let Some(help) = &finding.help {
        out.push_str(&format!("   = help: {help}\n"));
    }
    out
}

/// The offending source line with a caret run, single-line spans only.
/// Rendering is best-effort: an unreadable file just omits the snippet.
///
/// The span's file is workspace-root-relative, so it is resolved against
/// `workspace_root` rather than the process cwd — otherwise running from a
/// member directory reads the wrong path and silently drops every snippet.
fn source_snippet(finding: &Finding, workspace_root: &Path) -> Option<String> {
    let span = &finding.span;
    if span.line_start != span.line_end {
        return None;
    }
    let text = fs::read_to_string(workspace_root.join(&span.file)).ok()?;
    let line = text.lines().nth(span.line_start - 1)?;
    let gutter = span.line_start.to_string();
    let pad = " ".repeat(gutter.len());
    let caret_offset = " ".repeat(span.column_start.saturating_sub(1));
    let caret_len = span.column_end.saturating_sub(span.column_start).max(1);
    let carets = "^".repeat(caret_len);
    Some(format!(
        "{pad} |\n{gutter} | {line}\n{pad} | {caret_offset}{carets}\n"
    ))
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
            suppressed.contains("   = baseline: reviewed: the host half owns this buffer\n"),
            "{suppressed}"
        );
        // The rest of the diagnostic is unchanged: a suppression hides the
        // finding by default, it never edits it.
        assert!(suppressed.contains("= note: every thread gets"));
        assert!(suppressed.contains("= help: use `DisjointSlice<f32>`"));
    }
}
