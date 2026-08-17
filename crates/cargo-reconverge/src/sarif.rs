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

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use reconverge_artifacts::findings::Confidence;
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

pub fn write_report(path: &Path, review: &Review) -> std::io::Result<()> {
    let mut rules: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut results = Vec::new();

    for item in review.items() {
        let finding = item.finding;
        rules.entry(finding.code.clone()).or_insert_with(|| {
            json!({
                "id": finding.code,
                "shortDescription": { "text": rule_description(&finding.code) },
                "defaultConfiguration": { "level": level(finding.confidence) },
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
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": finding.span.file },
                    "region": {
                        "startLine": finding.span.line_start,
                        "startColumn": finding.span.column_start,
                        "endLine": finding.span.line_end,
                        "endColumn": finding.span.column_end,
                    },
                },
            }],
        });
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
