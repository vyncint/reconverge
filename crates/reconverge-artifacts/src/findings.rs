//! `findings.v1` — the diagnostics artifact (`schemas/findings.v1.json`).
//!
//! One document per analyzed **target**. A package with a lib and a bin
//! compiles twice under one crate name, and each compilation writes its own
//! document; `target` is what tells them apart. It was added in 0.5.0,
//! because before it a consumer keyed on `crate` — as the driver's own
//! comment told it to — silently kept one of the two, and in the ordinary
//! GPU-project shape (kernels in the lib, a thin host binary beside them)
//! the one it kept could be the empty one.
//!
//! Additive-only within v1: adding a field requires a `#[serde(default)]`
//! (or `Option`) so every existing fixture still parses, and the fixtures
//! must be updated in the same PR.

use serde::{Deserialize, Serialize};

use crate::read::Artifact;
use crate::schema;

/// Top-level findings artifact for one analyzed target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingsArtifact {
    /// Always [`schema::FINDINGS`].
    pub schema: String,
    pub tool: ToolInfo,
    /// Name of the analyzed crate.
    #[serde(rename = "crate")]
    pub krate: String,
    /// The compiled target's crate types, as cargo spells them — `lib`,
    /// `bin`, `proc-macro`. Absent only in a document written before 0.5.0,
    /// which is why it is an `Option` rather than a defaulted `String`: a
    /// reader can tell "the bin target" from "a document that predates the
    /// field" instead of being handed a plausible-looking empty string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// What the analyzer could and could not read across this whole target.
    ///
    /// Coverage used to reach the user only as a note on an RC001 finding,
    /// so it was missing in exactly the run where it is load-bearing: a
    /// kernel whose divergent barrier is spelled in `asm!` has no finding to
    /// hang the note on, and `--strict` exited 0 on it with no mention that
    /// a twentieth of the body was never read. Here it is a property of the
    /// analysis, so the JSON surface and SARIF carry it structurally rather
    /// than a script string-matching a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<RunCoverage>,
    pub findings: Vec<Finding>,
}

impl FindingsArtifact {
    pub fn new(krate: impl Into<String>, findings: Vec<Finding>) -> Self {
        FindingsArtifact {
            schema: schema::FINDINGS.to_string(),
            tool: ToolInfo::current(),
            krate: krate.into(),
            target: None,
            coverage: None,
            findings,
        }
    }

    /// The same document, naming the compiled target it came from.
    #[must_use]
    pub fn for_target(mut self, crate_types: impl Into<String>) -> Self {
        self.target = Some(crate_types.into());
        self
    }

    /// The same document, carrying the analysis's own coverage.
    #[must_use]
    pub fn with_coverage(mut self, coverage: RunCoverage) -> Self {
        self.coverage = Some(coverage);
        self
    }

    /// Sort key that is total across the documents of one run.
    ///
    /// `crate` alone is not: `sort_by` is stable, so two documents sharing a
    /// crate name fell through to whatever `read_dir` handed back — stable
    /// within a project, different between two projects of identical shape.
    #[must_use]
    pub fn sort_key(&self) -> (&str, &str) {
        (&self.krate, self.target.as_deref().unwrap_or(""))
    }
}

impl Artifact for FindingsArtifact {
    const SCHEMA: &'static str = schema::FINDINGS;

    fn declared_schema(&self) -> &str {
        &self.schema
    }
}

/// How much of an analyzed target the engine could actually read.
///
/// Counted over every analyzed function, kernels and callees alike, so it
/// answers the question a clean run raises: is this kernel clean, or was a
/// fifth of it never looked at?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCoverage {
    /// Statements the uniformity engine modeled.
    pub analyzed_statements: usize,
    /// Statements it could not (`asm!`, unmodeled intrinsics).
    pub opaque_statements: usize,
    /// Analyzed functions carrying at least one opaque statement.
    pub opaque_functions: usize,
}

impl RunCoverage {
    /// Whether anything at all was left unread.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.opaque_statements > 0
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
