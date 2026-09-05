//! Debugger inputs: parsed `witness.v1` artifacts, normalized for display.

use std::fs;
use std::path::{Path, PathBuf};

use reconverge_artifacts::witness::WitnessArtifact;

use crate::load::{display_name, nfc};

/// Everything the debugger shows, loaded once up front.
#[derive(Debug, Default)]
pub struct WitnessData {
    pub witnesses: Vec<WitnessArtifact>,
    /// Load problems, rendered in-frame.
    pub errors: Vec<String>,
}

/// Load `witness.v1` files; anything else is reported, never guessed at.
pub fn load(paths: &[PathBuf]) -> WitnessData {
    let mut data = WitnessData::default();
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
        if schema != "witness.v1" {
            data.errors
                .push(format!("{name}: unsupported schema `{schema}`"));
            continue;
        }
        match serde_json::from_str::<WitnessArtifact>(&text) {
            Ok(mut artifact) => {
                normalize(&mut artifact);
                data.witnesses.push(artifact);
            }
            Err(e) => data.errors.push(format!("{name}: {e}")),
        }
    }
    // Stable order regardless of how the launcher globbed the files.
    data.witnesses.sort_by(|a, b| {
        (&a.kernel, a.finding.as_ref().map(|f| &f.code))
            .cmp(&(&b.kernel, b.finding.as_ref().map(|f| &f.code)))
    });
    data
}

/// NFC-normalize and redact everything that can reach the screen: span
/// files become basenames, statements and messages are normalized.
fn normalize(artifact: &mut WitnessArtifact) {
    artifact.kernel = nfc(&artifact.kernel);
    artifact.verdict.message = nfc(&artifact.verdict.message);
    if let Some(finding) = &mut artifact.finding
        && let Some(span) = &mut finding.span
    {
        span.file = display_name(Path::new(&span.file));
    }
    for step in &mut artifact.steps {
        step.statement = nfc(&step.statement);
        if let Some(span) = &mut step.span {
            span.file = display_name(Path::new(&span.file));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::load;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/witness")
            .join(rel)
    }

    #[test]
    fn loads_both_canonical_fixtures_in_stable_order() {
        // Reversed on the command line; sorted by kernel after load.
        let data = load(&[
            fixture("rc002-partial-mask.json"),
            fixture("rc001-divergent-barrier.json"),
        ]);
        assert!(data.errors.is_empty(), "{:?}", data.errors);
        assert_eq!(data.witnesses.len(), 2);
        assert_eq!(data.witnesses[0].kernel, "rc001_divergent_barrier");
        assert_eq!(data.witnesses[1].kernel, "rc002_divergent_collective");
        // Span files are redacted to basenames on load.
        for w in &data.witnesses {
            for step in &w.steps {
                if let Some(span) = &step.span {
                    assert!(!span.file.contains('/'), "unredacted: {}", span.file);
                }
            }
        }
    }

    #[test]
    fn wrong_schemas_and_missing_files_become_errors() {
        let dir = std::env::temp_dir().join(format!("rc-tui-witness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("findings.json");
        std::fs::write(&other, r#"{"schema": "findings.v1"}"#).unwrap();
        let data = load(&[other, dir.join("nope.json")]);
        assert_eq!(data.witnesses.len(), 0);
        assert_eq!(data.errors.len(), 2);
        assert!(data.errors[0].contains("unsupported schema `findings.v1`"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
