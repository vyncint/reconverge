//! Triage inputs: the findings under review plus the baseline that
//! accepts some of them.

use std::fs;
use std::path::{Path, PathBuf};

use reconverge_artifacts::baseline::BaselineArtifact;
use reconverge_artifacts::findings::{Finding, FindingsArtifact};

use crate::load::{display_name, nfc};

/// Everything the triage view reads, loaded once up front. The baseline
/// itself lives in the state, because it is what the keystrokes edit.
#[derive(Debug, Default)]
pub struct TriageData {
    pub items: Vec<TriageItem>,
    /// Where the baseline will be written; may not exist yet.
    pub baseline_path: PathBuf,
    /// Load problems, rendered in-frame.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TriageItem {
    /// Analyzed crate the finding belongs to.
    pub krate: String,
    pub finding: Finding,
}

/// Load `findings.v1` documents and the baseline at `baseline_path`.
///
/// A missing baseline is an empty one — triage's whole job is to create
/// it. A malformed one is an error surfaced in-frame rather than a panic,
/// because the file is hand-editable by design.
pub fn load(paths: &[PathBuf], baseline_path: &Path) -> (TriageData, BaselineArtifact) {
    let mut data = TriageData {
        baseline_path: baseline_path.to_path_buf(),
        ..TriageData::default()
    };

    for path in paths {
        let name = display_name(path);
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                data.errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let schema = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["schema"].as_str().map(str::to_string))
            .unwrap_or_default();
        if schema != "findings.v1" {
            data.errors
                .push(format!("{name}: unsupported schema `{schema}`"));
            continue;
        }
        match serde_json::from_str::<FindingsArtifact>(&text) {
            Ok(artifact) => {
                for mut finding in artifact.findings {
                    finding.message = nfc(&finding.message);
                    finding.span.file = display_name(Path::new(&finding.span.file));
                    data.items.push(TriageItem {
                        krate: nfc(&artifact.krate),
                        finding,
                    });
                }
            }
            Err(e) => data.errors.push(format!("{name}: {e}")),
        }
    }

    // Stable order regardless of how the launcher globbed the files.
    data.items.sort_by(|a, b| {
        (&a.krate, &a.finding.code, &a.finding.kernel).cmp(&(
            &b.krate,
            &b.finding.code,
            &b.finding.kernel,
        ))
    });

    let baseline = match fs::read_to_string(baseline_path) {
        Ok(text) => match serde_json::from_str::<BaselineArtifact>(&text) {
            Ok(mut parsed) => {
                for entry in &mut parsed.entries {
                    entry.reason = nfc(&entry.reason);
                }
                parsed.normalize();
                parsed
            }
            Err(e) => {
                data.errors.push(format!(
                    "{}: not a baseline.v1 document: {e}",
                    display_name(baseline_path)
                ));
                BaselineArtifact::empty()
            }
        },
        Err(_) => BaselineArtifact::empty(),
    };

    (data, baseline)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::load;

    fn fixture(rel: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(rel)
    }

    #[test]
    fn loads_findings_with_the_baseline_that_accepts_them() {
        let (data, baseline) = load(
            &[fixture("findings/rc003-minimal.json")],
            &fixture("baseline/minimal.json"),
        );
        assert!(data.errors.is_empty(), "{:?}", data.errors);
        assert_eq!(data.items.len(), 2);
        assert_eq!(data.items[0].krate, "sample_kernels");
        assert_eq!(baseline.entries.len(), 1);
        // The fixture baseline accepts the RC003 on `bad_mut_slice`, and
        // leaves the RC005 open — the mixed state a review starts from.
        let accepted = data
            .items
            .iter()
            .filter(|i| baseline.suppression_of(&i.krate, &i.finding).is_some())
            .count();
        assert_eq!(accepted, 1);
    }

    #[test]
    fn a_missing_baseline_is_simply_empty() {
        let (data, baseline) = load(
            &[fixture("findings/rc003-minimal.json")],
            &fixture("baseline/does-not-exist.json"),
        );
        assert!(data.errors.is_empty(), "{:?}", data.errors);
        assert!(baseline.entries.is_empty());
        assert!(!data.items.is_empty());
    }

    #[test]
    fn wrong_schemas_are_reported_in_frame() {
        let (data, _) = load(
            &[fixture("witness/rc001-divergent-barrier.json")],
            &fixture("baseline/minimal.json"),
        );
        assert!(data.items.is_empty());
        assert_eq!(data.errors.len(), 1);
        assert!(data.errors[0].contains("unsupported schema `witness.v1`"));
    }
}
