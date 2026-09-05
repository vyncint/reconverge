//! Artifact loading for the shell: sniff the schema, parse with the typed
//! bindings, and reduce each document to one display line.
//!
//! Every string that can reach the screen is NFC-normalized here
//! (docs/ARCHITECTURE.md), and file paths are reduced to their basename so frames
//! never contain absolute paths.

use std::fs;
use std::path::Path;

use reconverge_artifacts::findings::{Confidence, FindingsArtifact};
use reconverge_artifacts::plural;
use reconverge_artifacts::unimap::{Uniformity, UnimapArtifact};
use reconverge_artifacts::witness::{VerdictKind, WitnessArtifact};
use unicode_normalization::UnicodeNormalization;

/// One loaded artifact, reduced to what the shell shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedArtifact {
    /// File basename (redacted: never a full path).
    pub name: String,
    /// Schema identifier, e.g. `findings.v1`.
    pub schema: String,
    /// One-line content summary.
    pub summary: String,
}

/// NFC-normalize a string on its way toward the screen.
#[must_use]
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// Redaction helper: the displayable name of a file is its basename.
#[must_use]
pub fn display_name(path: &Path) -> String {
    nfc(&path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    ))
}

/// Load one artifact file of any supported schema.
pub fn load(path: &Path) -> Result<LoadedArtifact, String> {
    let name = display_name(path);
    let text = fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{name}: not JSON: {e}"))?;
    let schema = value["schema"].as_str().unwrap_or("(missing)").to_string();

    let summary = match schema.as_str() {
        "findings.v1" => {
            let artifact: FindingsArtifact =
                serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
            summarize_findings(&artifact)
        }
        "unimap.v1" => {
            let artifact: UnimapArtifact =
                serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
            summarize_unimap(&artifact)
        }
        "witness.v1" => {
            let artifact: WitnessArtifact =
                serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
            summarize_witness(&artifact)
        }
        other => return Err(format!("{name}: unsupported schema `{other}`")),
    };

    Ok(LoadedArtifact {
        name,
        schema: nfc(&schema),
        summary: nfc(&summary),
    })
}

fn summarize_findings(artifact: &FindingsArtifact) -> String {
    let mut deny = 0;
    let mut confirmed = 0;
    let mut warning = 0;
    for finding in &artifact.findings {
        match finding.confidence {
            Confidence::Deny => deny += 1,
            Confidence::Confirmed => confirmed += 1,
            Confidence::Warning => warning += 1,
        }
    }
    format!(
        "crate {} — {} {}: {deny} deny, {confirmed} confirmed, {warning} warning",
        artifact.krate,
        artifact.findings.len(),
        plural(artifact.findings.len(), "finding", "findings")
    )
}

fn summarize_unimap(artifact: &UnimapArtifact) -> String {
    let functions = artifact.functions.len();
    let values: usize = artifact.functions.iter().map(|f| f.values.len()).sum();
    let divergent: usize = artifact
        .functions
        .iter()
        .flat_map(|f| &f.values)
        .filter(|v| v.uniformity == Uniformity::Divergent)
        .count();
    format!(
        "crate {} — {functions} {}, {values} {}, {divergent} divergent",
        artifact.krate,
        plural(functions, "function", "functions"),
        plural(values, "value", "values")
    )
}

fn summarize_witness(artifact: &WitnessArtifact) -> String {
    let verdict = match artifact.verdict.kind {
        VerdictKind::Hang => "hang",
        VerdictKind::UndefinedBehavior => "undefined behavior",
        VerdictKind::Completed => "completed",
        VerdictKind::NoWitness => "no witness",
    };
    let at = artifact
        .verdict
        .step
        .map(|s| format!(" at step {s}"))
        .unwrap_or_default();
    format!(
        "kernel {} — verdict: {verdict}{at} ({} steps, {} lanes)",
        artifact.kernel,
        artifact.steps.len(),
        artifact.lanes
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fixture(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(rel)
    }

    #[test]
    fn loads_and_summarizes_every_fixture_schema() {
        let findings = load(&fixture("findings/rc003-minimal.json")).unwrap();
        assert_eq!(findings.schema, "findings.v1");
        assert_eq!(
            findings.summary,
            "crate sample_kernels — 2 findings: 1 deny, 0 confirmed, 1 warning"
        );

        let unimap = load(&fixture("unimap/divergent-barrier.json")).unwrap();
        assert_eq!(unimap.schema, "unimap.v1");
        assert_eq!(
            unimap.summary,
            "crate sample_kernels — 1 function, 3 values, 2 divergent"
        );

        let witness = load(&fixture("witness/rc001-divergent-barrier.json")).unwrap();
        assert_eq!(witness.schema, "witness.v1");
        assert_eq!(
            witness.summary,
            "kernel rc001_divergent_barrier — verdict: undefined behavior at step 2 \
             (3 steps, 32 lanes)"
        );

        // The multi-warp fixture: recorded from a run whose kernel declares
        // `block = (64, 1, 1)`, which is the shape `witness.v1` used to
        // reject while the driver had been writing it for four minor
        // versions. Loading it is the reader half of that.
        let wide = load(&fixture("witness/rc001-multiwarp-barrier.json")).unwrap();
        assert_eq!(wide.schema, "witness.v1");
        assert!(wide.summary.contains("64 lanes"), "{}", wide.summary);
    }

    #[test]
    fn display_name_redacts_directories() {
        assert_eq!(
            display_name(Path::new("/very/secret/place/artifact.json")),
            "artifact.json"
        );
    }

    #[test]
    fn strings_are_nfc_normalized() {
        // "é" as 'e' + COMBINING ACUTE ACCENT (NFD) becomes the single
        // precomposed code point (NFC).
        assert_eq!(nfc("e\u{301}"), "\u{e9}");
    }

    #[test]
    fn unknown_schema_is_an_error() {
        let dir = std::env::temp_dir().join(format!("rc-tui-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weird.json");
        std::fs::write(&path, r#"{"schema": "mystery.v9"}"#).unwrap();
        let err = load(&path).unwrap_err();
        assert!(err.contains("unsupported schema `mystery.v9`"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
