//! SARIF 2.1.0 report generation (`--sarif <path>`).
//!
//! Every finding is included regardless of `--strict`: SARIF consumers
//! filter by level themselves. `deny`/`confirmed` map to `error`,
//! `warning` to `warning`.
//!
//! Baseline-accepted findings are reported the standard way rather than
//! dropped: SARIF 2.1.0 has a `suppressions` property for exactly this, so
//! a code-scanning UI shows them as suppressed-with-justification instead
//! of silently losing them.
//!
//! `--sarif` is how the GitHub Action delivers findings, so for a CI user
//! SARIF *is* the output. Three things the text diagnostic carries used to
//! stop at the file: the provenance walk (dropped entirely, though SARIF
//! has `relatedLocations` for exactly its shape), the explain page (no
//! `helpUri`, so the "Learn more" link had nothing behind it), and a rule's
//! default severity, which was sampled from whichever finding of that code
//! happened to come first — RC001 and RC002 span two tiers, so the same
//! binary published contradictory defaults for the same rule across two
//! projects.
//!
//! Two more things decide whether an alert is any use once uploaded. Its
//! location: GitHub reads a relative URI from the *repository* root, while
//! the driver reports paths relative to the *Cargo workspace* root, so a
//! workspace in a subdirectory put every alert on a file that does not
//! exist. And its rule text: code scanning shows `fullDescription` and
//! `help` next to an alert, and a rule carrying neither showed a bare id.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use reconverge_artifacts::findings::{Confidence, SourceSpan};
use serde_json::json;

use crate::review::Review;

/// One-line rule descriptions from the diagnostic registry (README.md).
///
/// RC001 used to say `sync_threads()`, which is one of the barriers it
/// covers; a cluster-wide or grid-wide one is the same bug, and an alert
/// on `grid::sync()` titled after a different call reads as a misfire.
fn rule_description(code: &str) -> &'static str {
    match code {
        "RC001" => "an all-threads barrier under thread-divergent control",
        "RC002" => "warp collective at a non-convergent point or mask–lane mismatch",
        "RC003" => "&mut [T] as a #[kernel] parameter",
        "RC004" => "static shared memory exceeds the target's limit",
        "RC005" => "launch-contract inconsistency",
        _ => "reconverge finding",
    }
}

fn level(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Deny | Confidence::Confirmed => "error",
        Confidence::Warning => "warning",
    }
}

/// A rule's default severity, as a property of the code rather than of
/// whichever result came first.
///
/// The tier ladder in the README already fixes this per code: RC003, RC004
/// and RC005 are structural and always gate; RC001 and RC002 span
/// `confirmed` and `warning`, and the *default* for a rule that can gate is
/// the gating level — a consumer reading the rule metadata should be told
/// the strictest thing the rule can say, not the first thing it happened to.
fn rule_default_level(code: &str) -> &'static str {
    match code {
        "RC001" | "RC002" | "RC003" | "RC004" | "RC005" => "error",
        _ => "warning",
    }
}

/// Where the explain page for `code` is published.
///
/// Every finding already carries `"explain"`, five pages are written, and
/// `cargo reconverge --explain RC001` renders them — but the SARIF rule had
/// no `helpUri`, so a code-scanning alert offered nothing to click at the
/// point where the reader most wants the "why".
fn help_uri(explain: &str) -> String {
    format!(
        "https://github.com/vyncint/reconverge/blob/v{}/crates/cargo-reconverge/explain/{explain}.md",
        env!("CARGO_PKG_VERSION")
    )
}

/// A rule's `fullDescription`: the opening paragraph of its explain page.
///
/// Derived rather than written a second time, so the alert and `--explain`
/// cannot describe two different rules. The page's line breaks are wrapping,
/// not structure, so they are joined.
fn full_description(page: &str) -> String {
    page.lines()
        .skip_while(|line| line.starts_with('#') || line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The base every relative `uri` in the report is resolved against.
const SRCROOT: &str = "%SRCROOT%";

/// The directory the report's relative URIs are relative to: the
/// repository that holds the workspace, or the workspace itself outside one.
///
/// GitHub resolves a relative URI against the repository root and nothing
/// else — `uriBaseId` is not consulted — so a Cargo workspace in a
/// subdirectory (`working-directory: kernels` in the Action) has to report
/// `kernels/src/lib.rs`, not `src/lib.rs`. Finding the repository here
/// rather than having the Action pass a prefix fixes every way of running
/// the check, not only the Action's. A `.git` *file* counts: a worktree or
/// a submodule is a repository root as far as the paths in it go.
///
/// The choice never changes what a location means, only how it is spelled:
/// `originalUriBaseIds` declares this directory, so a consumer that does
/// resolve `%SRCROOT%` lands on the same file either way.
fn source_root(workspace_root: &Path) -> &Path {
    workspace_root
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace_root)
}

/// `a/./b/../c` → `a/c`, without touching the filesystem: the spans name
/// files the compiler read, and the report should spell them the way the
/// compiler did, not the way a symlink resolves.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Percent-encode one path segment. RFC 3986 `pchar` stays as it is —
/// including `:`, so a Windows drive reads `C:` — and everything else,
/// space, `%`, `#`, `?` and any non-ASCII byte among it, is encoded, or a
/// consumer would read part of a file name as a query or a fragment.
fn encode_segment(segment: &str, out: &mut String) {
    use std::fmt::Write as _;
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

/// The `/`-separated, percent-encoded form of a path's segments.
fn encode_path(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        let segment = match component {
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy(),
            Component::Normal(segment) => segment.to_string_lossy(),
            Component::RootDir | Component::CurDir | Component::ParentDir => continue,
        };
        if !out.is_empty() {
            out.push('/');
        }
        encode_segment(&segment, &mut out);
    }
    out
}

/// An absolute path as a `file://` URI: `file:///home/u/k`,
/// `file:///C:/Users/u/k`.
fn file_uri(path: &Path) -> String {
    format!("file:///{}", encode_path(path))
}

/// Where a span's file is, for SARIF.
///
/// Relative to `%SRCROOT%` when it is inside the source root, which is
/// every workspace file; an absolute `file://` URI otherwise (a path
/// dependency outside the repository), which GitHub keeps as it is and
/// every other consumer can open.
fn artifact_location(file: &str, workspace_root: &Path, source_root: &Path) -> serde_json::Value {
    let absolute = normalize(&workspace_root.join(file));
    match absolute.strip_prefix(source_root) {
        Ok(relative) => json!({ "uri": encode_path(relative), "uriBaseId": SRCROOT }),
        Err(_) => json!({ "uri": file_uri(&absolute) }),
    }
}

/// Where the report's paths are resolved from, as a SARIF location.
struct Locator<'a> {
    workspace_root: &'a Path,
    source_root: &'a Path,
}

impl Locator<'_> {
    fn physical_location(&self, span: &SourceSpan) -> serde_json::Value {
        physical_location(
            artifact_location(&span.file, self.workspace_root, self.source_root),
            span,
        )
    }
}

fn physical_location(artifact: serde_json::Value, span: &SourceSpan) -> serde_json::Value {
    json!({
        "artifactLocation": artifact,
        "region": {
            "startLine": span.line_start,
            "startColumn": span.column_start,
            "endLine": span.line_end,
            "endColumn": span.column_end,
        },
    })
}

/// Write the report for `review`. `workspace_root` is what the findings'
/// relative paths are relative to.
pub fn write_report(path: &Path, review: &Review, workspace_root: &Path) -> std::io::Result<()> {
    let report = report(review, workspace_root);
    let mut file = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut file, &report)?;
    writeln!(file)?;
    file.flush()
}

fn rule(code: &str, explain: &str) -> serde_json::Value {
    let mut rule = json!({
        "id": code,
        "shortDescription": { "text": rule_description(code) },
        // Not `level(finding.confidence)`: that sampled the first
        // result of this code, so swapping two kernels in a source
        // file flipped the published default.
        "defaultConfiguration": { "level": rule_default_level(code) },
        "helpUri": help_uri(explain),
    });
    // The page `--explain` prints is the help an alert shows: GitHub renders
    // `help.markdown` where it has it and `help.text` elsewhere, and the page
    // is Markdown that reads as plain text too.
    if let Some(page) = crate::explain::page(explain) {
        rule["fullDescription"] = json!({ "text": full_description(page) });
        rule["help"] = json!({ "text": page, "markdown": page });
    }
    rule
}

fn report(review: &Review, workspace_root: &Path) -> serde_json::Value {
    let mut rules: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut results = Vec::new();
    let locator = Locator {
        workspace_root,
        source_root: source_root(workspace_root),
    };

    for item in review.items() {
        let finding = item.finding;
        rules
            .entry(finding.code.clone())
            .or_insert_with(|| rule(&finding.code, &finding.explain));

        let mut text = finding.message.clone();
        for note in &finding.notes {
            text.push_str("\n\nnote: ");
            text.push_str(note);
        }
        if let Some(help) = &finding.help {
            text.push_str("\n\nhelp: ");
            text.push_str(help);
        }

        let mut result = json!({
            "ruleId": finding.code,
            "level": level(finding.confidence),
            "message": { "text": text },
            "locations": [{ "physicalLocation": locator.physical_location(&finding.span) }],
        });
        // The provenance walk: an ordered chain of spans back to the
        // divergence source, which is the one part of the diagnostic SARIF
        // has a first-class place for. `relatedLocations` renders as
        // clickable links; `codeFlows` carries the order, which is what
        // makes the chain a walk rather than a set.
        if !finding.provenance.is_empty() {
            let related: Vec<serde_json::Value> = finding
                .provenance
                .iter()
                .enumerate()
                .map(|(i, step)| {
                    json!({
                        "id": i,
                        "physicalLocation": locator.physical_location(&step.span),
                        "message": { "text": step.what },
                    })
                })
                .collect();
            result["codeFlows"] = json!([{
                "message": { "text": "def-use chain back to the divergence source" },
                "threadFlows": [{
                    "locations": related
                        .iter()
                        .map(|location| json!({ "location": location }))
                        .collect::<Vec<_>>(),
                }],
            }]);
            result["relatedLocations"] = json!(related);
        }
        if let Some(entry) = item.suppression {
            // "external": the decision lives outside the analysis, in the
            // reviewed baseline file.
            result["suppressions"] = json!([{
                "kind": "external",
                "justification": entry.reason,
            }]);
        }
        results.push(result);
    }

    // A base URI must end in `/`, or `src/lib.rs` resolves against its
    // parent and drops the last directory.
    let mut base = file_uri(locator.source_root);
    if !base.ends_with('/') {
        base.push('/');
    }
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "reconverge",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/vyncint/reconverge",
                    "rules": rules.into_values().collect::<Vec<_>>(),
                },
            },
            "originalUriBaseIds": { SRCROOT: { "uri": base } },
            "results": results,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_file_is_spelled_from_the_source_root() {
        // A workspace in a subdirectory of the repository: the URI has to
        // carry the subdirectory, because GitHub reads it from the
        // repository root and does not consult `uriBaseId`.
        let repo = Path::new("/home/u/my repo");
        let workspace = repo.join("kernels");
        assert_eq!(
            artifact_location("src/lib.rs", &workspace, repo),
            json!({ "uri": "kernels/src/lib.rs", "uriBaseId": SRCROOT })
        );
        // A path dependency beside the workspace is still in the repository.
        assert_eq!(
            artifact_location("../shared/src/a b#c.rs", &workspace, repo),
            json!({ "uri": "shared/src/a%20b%23c.rs", "uriBaseId": SRCROOT })
        );
        // Outside the repository there is no relative spelling to give.
        assert_eq!(
            artifact_location("../../elsewhere/x.rs", &workspace, repo),
            json!({ "uri": "file:///home/u/elsewhere/x.rs" })
        );
    }

    #[test]
    fn the_base_uri_is_an_encoded_absolute_directory() {
        assert_eq!(
            file_uri(Path::new("/home/u/my repo/é")),
            "file:///home/u/my%20repo/%C3%A9"
        );
        assert_eq!(file_uri(Path::new("/")), "file:///");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_path_keeps_its_drive_and_loses_its_backslashes() {
        assert_eq!(file_uri(Path::new(r"C:\Users\u\k")), "file:///C:/Users/u/k");
    }

    #[test]
    fn the_source_root_is_the_repository_around_the_workspace() {
        let base = std::env::temp_dir().join(format!("reconverge-sarif-{}", std::process::id()));
        let workspace = base.join("repo/kernels");
        std::fs::create_dir_all(&workspace).unwrap();
        // A worktree's `.git` is a file, and counts. (The fallback — no
        // repository at all — is not asserted: whether the temp dir sits
        // inside one is the machine's business.)
        std::fs::write(base.join("repo/.git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(source_root(&workspace), base.join("repo").as_path());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn every_rule_carries_what_code_scanning_shows() {
        for code in ["RC001", "RC002", "RC003", "RC004", "RC005"] {
            let rule = rule(code, code);
            let full = rule["fullDescription"]["text"].as_str().unwrap();
            // GitHub's limit for both descriptions.
            assert!(
                !full.is_empty() && full.len() <= 1024,
                "{code}: {} chars",
                full.len()
            );
            assert!(
                !full.starts_with('#') && !full.contains('\n'),
                "{code}: {full}"
            );
            let page = crate::explain::page(code).unwrap();
            assert_eq!(rule["help"]["markdown"], page, "{code}");
            assert_eq!(rule["help"]["text"], page, "{code}");
            assert!(page.contains(full.split(' ').next().unwrap()));
        }
        // RC001 is every all-threads barrier, not one call.
        assert_eq!(
            rule("RC001", "RC001")["shortDescription"]["text"],
            "an all-threads barrier under thread-divergent control"
        );
    }
}
