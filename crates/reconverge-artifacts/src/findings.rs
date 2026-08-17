//! `findings.v1` — the diagnostics artifact (`schemas/findings.v1.json`).
//!
//! One document per analyzed crate. Additive-only within v1: adding a field
//! requires a `#[serde(default)]` (or `Option`) so every existing fixture
//! still parses, and the fixtures must be updated in the same PR.

use serde::{Deserialize, Serialize};

use crate::schema;

/// Top-level findings artifact for one analyzed crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsArtifact {
    /// Always [`schema::FINDINGS`].
    pub schema: String,
    pub tool: ToolInfo,
    /// Name of the analyzed crate.
    #[serde(rename = "crate")]
    pub krate: String,
    pub findings: Vec<Finding>,
}

impl FindingsArtifact {
    pub fn new(krate: impl Into<String>, findings: Vec<Finding>) -> Self {
        FindingsArtifact {
            schema: schema::FINDINGS.to_string(),
            tool: ToolInfo::current(),
            krate: krate.into(),
            findings,
        }
    }
}

/// Identity of the tool that produced an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
}

impl ToolInfo {
    pub fn current() -> Self {
        ToolInfo {
            name: "reconverge".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// A single diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Diagnostic code from the registry, e.g. `"RC003"`.
    pub code: String,
    pub confidence: Confidence,
    /// One-line, human-readable statement of the problem.
    pub message: String,
    /// User-facing name of the kernel the finding is about, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    pub span: SourceSpan,
    /// Hardware consequences and other context, one note per line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Suggested fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Explain code (`--explain` key); usually equal to `code`.
    pub explain: String,
    /// Def→use chain back to the divergence source. Empty for syntactic
    /// findings; the uniformity dataflow fills it in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<ProvenanceStep>,
}

/// Confidence tiers (docs/ARCHITECTURE.md).
///
/// `deny` — syntactically proven, always shown. `confirmed` — static finding
/// plus a witness replay, always shown. `warning` — conservative static
/// result with no witness, hidden unless `--strict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Deny,
    Confirmed,
    Warning,
}

impl Confidence {
    /// Whether findings of this tier are shown without `--strict`.
    #[must_use]
    pub fn shown_by_default(self) -> bool {
        matches!(self, Confidence::Deny | Confidence::Confirmed)
    }

    /// Whether findings of this tier make `cargo reconverge check` exit 1.
    #[must_use]
    pub fn gates_exit_code(self) -> bool {
        matches!(self, Confidence::Deny | Confidence::Confirmed)
    }
}

/// A source region, 1-based lines and columns, end-inclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: String,
    pub line_start: usize,
    pub column_start: usize,
    pub line_end: usize,
    pub column_end: usize,
}

/// One hop of a provenance chain (def→use back to a divergence source).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceStep {
    /// What this hop is, e.g. "condition derives from `index_1d()`".
    pub what: String,
    pub span: SourceSpan,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    #[test]
    fn confidence_tiers_gate_display_and_exit() {
        assert!(Confidence::Deny.shown_by_default());
        assert!(Confidence::Confirmed.shown_by_default());
        assert!(!Confidence::Warning.shown_by_default());
        assert!(!Confidence::Warning.gates_exit_code());
    }

    /// The fixtures are the API tests: every findings fixture must
    /// round-trip through the serde types without losing information.
    #[test]
    fn findings_fixtures_round_trip() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/findings");
        let mut checked = 0;
        for entry in fs::read_dir(&dir).expect("fixtures/findings must exist") {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let parsed: FindingsArtifact = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
            assert_eq!(parsed.schema, crate::schema::FINDINGS);
            let reserialized = serde_json::to_value(&parsed).unwrap();
            let original: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                reserialized,
                original,
                "{} does not round-trip",
                path.display()
            );
            checked += 1;
        }
        assert!(checked >= 1, "no findings fixtures found in {dir:?}");
    }
}
