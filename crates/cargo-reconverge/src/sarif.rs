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

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use reconverge_artifacts::findings::{Confidence, SourceSpan};
use serde_json::json;

use crate::review::Review;

/// One-line rule descriptions from the diagnostic registry (README.md).
fn rule_description(code: &str) -> &'static str {
    match code {
        "RC001" => "sync_threads() under thread-divergent control flow",
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

fn physical_location(span: &SourceSpan) -> serde_json::Value {
    json!({
        "artifactLocation": { "uri": span.file },
        "region": {
            "startLine": span.line_start,
            "startColumn": span.column_start,
            "endLine": span.line_end,
            "endColumn": span.column_end,
        },
    })
}

pub fn write_report(path: &Path, review: &Review) -> std::io::Result<()> {
    let mut rules: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut results = Vec::new();

    for item in review.items() {
        let finding = item.finding;
        rules.entry(finding.code.clone()).or_insert_with(|| {
            json!({
                "id": finding.code,
                "shortDescription": { "text": rule_description(&finding.code) },
                // Not `level(finding.confidence)`: that sampled the first
                // result of this code, so swapping two kernels in a source
                // file flipped the published default.
                "defaultConfiguration": { "level": rule_default_level(&finding.code) },
                "helpUri": help_uri(&finding.explain),
            })
        });

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
            "locations": [{ "physicalLocation": physical_location(&finding.span) }],
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
                        "physicalLocation": physical_location(&step.span),
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

    let report = json!({
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
            "results": results,
        }],
    });

    let mut file = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut file, &report)?;
    writeln!(file)?;
    file.flush()
}
