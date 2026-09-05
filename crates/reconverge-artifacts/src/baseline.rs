//! `baseline.v1` — reviewed suppressions (`schemas/baseline.v1.json`).
//!
//! The baseline is the one artifact a *human* owns: a checked-in list of
//! findings that were reviewed and deliberately accepted, each with a
//! written reason. `cargo reconverge triage` maintains it; `check` applies
//! it to display and to the exit code.
//!
//! It is a **policy layer, not an analysis record**: the driver never
//! reads it, so `findings.v1` stays a faithful account of what the engine
//! actually found (schemas/README.md — artifacts are the contract; suppression
//! is a review decision layered on top).
//!
//! Entries match on `(crate, kernel, code)` and deliberately **not** on
//! line numbers: a span moves the moment anyone edits the file above it,
//! and a suppression that silently stops matching is worse than no
//! suppression at all. The cost is that an entry covers every finding of
//! that code in that kernel, which is stated in the docs and in the file
//! the tool writes.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, ToolInfo};
use crate::read::Artifact;
use crate::schema;

/// Top-level baseline document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineArtifact {
    /// Always [`schema::BASELINE`].
    pub schema: String,
    pub tool: ToolInfo,
    pub entries: Vec<Entry>,
}

/// One reviewed suppression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entry {
    /// Analyzed crate the finding belongs to.
    #[serde(rename = "crate")]
    pub krate: String,
    /// Kernel the finding is about; absent for crate-level findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// Diagnostic code, e.g. `"RC005"`.
    pub code: String,
    /// Why this finding is accepted. Required: a suppression without a
    /// reason is exactly the kind of silent debt this file exists to
    /// prevent.
    pub reason: String,
}

impl BaselineArtifact {
    #[must_use]
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut artifact = BaselineArtifact {
            schema: schema::BASELINE.to_string(),
            tool: ToolInfo::current(),
            entries,
        };
        artifact.normalize();
        artifact
    }

    /// An empty baseline (what `triage` starts from when none exists).
    #[must_use]
    pub fn empty() -> Self {
        BaselineArtifact::new(Vec::new())
    }

    /// Sort and deduplicate entries so the file diffs cleanly in review.
    pub fn normalize(&mut self) {
        self.entries.sort();
        self.entries.dedup_by(|a, b| a.matches_same_findings(b));
    }

    /// The entry suppressing `finding` in crate `krate`, if any.
    #[must_use]
    pub fn suppression_of(&self, krate: &str, finding: &Finding) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.covers(krate, &finding.code, finding.kernel.as_deref()))
    }

    /// Add or replace the suppression covering this finding, returning
    /// true when the baseline changed.
    pub fn suppress(&mut self, krate: &str, finding: &Finding, reason: &str) -> bool {
        let entry = Entry {
            krate: krate.to_string(),
            kernel: finding.kernel.clone(),
            code: finding.code.clone(),
            reason: reason.to_string(),
        };
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.matches_same_findings(&entry))
        {
            Some(existing) if existing.reason == entry.reason => false,
            Some(existing) => {
                existing.reason = entry.reason;
                true
            }
            None => {
                self.entries.push(entry);
                self.normalize();
                true
            }
        }
    }

    /// Write the document — normalized, pretty-printed, newline-terminated
    /// — so the CLI and the triage view always produce the same bytes for
    /// the same decisions, and the file stays reviewable in a diff.
    /// Written beside the target and renamed into place: a bare
    /// `fs::write` truncates first, so a full disk or a killed process
    /// destroyed a *good* baseline — the one checked-in record of which
    /// findings a human looked at and why.
    pub fn write_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut normalized = self.clone();
        normalized.normalize();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(&normalized)?;
        text.push('\n');
        // Same directory, so the rename is on one filesystem and therefore
        // atomic. A failure before it leaves the original untouched.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        match std::fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                Err(e)
            }
        }
    }

    /// Drop the suppression covering this finding, returning true when the
    /// baseline changed.
    pub fn unsuppress(&mut self, krate: &str, finding: &Finding) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !entry.covers(krate, &finding.code, finding.kernel.as_deref()));
        self.entries.len() != before
    }
}

impl Artifact for BaselineArtifact {
    const SCHEMA: &'static str = schema::BASELINE;

    fn declared_schema(&self) -> &str {
        &self.schema
    }
}

impl Entry {
    /// Whether this entry covers a finding of `code` in `kernel`.
    #[must_use]
    pub fn covers(&self, krate: &str, code: &str, kernel: Option<&str>) -> bool {
        self.krate == krate && self.code == code && self.kernel.as_deref() == kernel
    }

    /// Whether two entries name the same set of findings (everything but
    /// the reason).
    #[must_use]
    pub fn matches_same_findings(&self, other: &Entry) -> bool {
        self.krate == other.krate && self.kernel == other.kernel && self.code == other.code
    }

    /// Display form used by the CLI and the triage view.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.kernel {
            Some(kernel) => format!("{} {} in `{kernel}`", self.krate, self.code),
            None => format!("{} {}", self.krate, self.code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{Confidence, SourceSpan};
    use crate::tests_support::round_trip_fixtures;

    fn finding(code: &str, kernel: Option<&str>) -> Finding {
        Finding {
            code: code.to_string(),
            confidence: Confidence::Warning,
            message: "m".into(),
            kernel: kernel.map(str::to_string),
            span: SourceSpan {
                file: "src/lib.rs".into(),
                line_start: 1,
                column_start: 1,
                line_end: 1,
                column_end: 2,
            },
            notes: Vec::new(),
            help: None,
            explain: code.to_string(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn baseline_fixtures_round_trip() {
        round_trip_fixtures("baseline", |text| {
            let parsed: BaselineArtifact = serde_json::from_str(text)?;
            assert_eq!(parsed.schema, schema::BASELINE);
            for entry in &parsed.entries {
                assert!(!entry.reason.trim().is_empty(), "reasons are required");
            }
            serde_json::to_value(&parsed)
        });
    }

    #[test]
    fn suppression_matches_on_crate_kernel_and_code_only() {
        let mut baseline = BaselineArtifact::empty();
        let f = finding("RC005", Some("scale"));
        assert!(baseline.suppress("k", &f, "reviewed: host validates the shape"));

        // Same code and kernel, different span and message: still covered —
        // spans move, so they are deliberately not part of the key.
        let mut moved = finding("RC005", Some("scale"));
        moved.span.line_start = 99;
        moved.message = "different wording".into();
        assert!(baseline.suppression_of("k", &moved).is_some());

        // Different kernel, different code, different crate: not covered.
        assert!(
            baseline
                .suppression_of("k", &finding("RC005", Some("other")))
                .is_none()
        );
        assert!(
            baseline
                .suppression_of("k", &finding("RC001", Some("scale")))
                .is_none()
        );
        assert!(baseline.suppression_of("other", &f).is_none());
        // A crate-level finding is not covered by a kernel entry.
        assert!(
            baseline
                .suppression_of("k", &finding("RC005", None))
                .is_none()
        );
    }

    #[test]
    fn suppress_is_idempotent_and_updates_reasons() {
        let mut baseline = BaselineArtifact::empty();
        let f = finding("RC003", Some("scale"));
        assert!(baseline.suppress("k", &f, "first reason"));
        assert!(!baseline.suppress("k", &f, "first reason"), "no-op repeat");
        assert!(
            baseline.suppress("k", &f, "better reason"),
            "reason updated"
        );
        assert_eq!(baseline.entries.len(), 1);
        assert_eq!(baseline.entries[0].reason, "better reason");

        assert!(baseline.unsuppress("k", &f));
        assert!(baseline.entries.is_empty());
        assert!(!baseline.unsuppress("k", &f), "already gone");
    }

    #[test]
    fn entries_are_sorted_and_deduplicated_for_clean_diffs() {
        let mut baseline = BaselineArtifact::new(vec![
            Entry {
                krate: "z".into(),
                kernel: None,
                code: "RC004".into(),
                reason: "b".into(),
            },
            Entry {
                krate: "a".into(),
                kernel: Some("k".into()),
                code: "RC001".into(),
                reason: "a".into(),
            },
            Entry {
                krate: "a".into(),
                kernel: Some("k".into()),
                code: "RC001".into(),
                reason: "duplicate, dropped".into(),
            },
        ]);
        baseline.normalize();
        assert_eq!(baseline.entries.len(), 2);
        assert_eq!(baseline.entries[0].krate, "a");
        assert_eq!(baseline.entries[0].reason, "a");
        assert_eq!(baseline.entries[1].krate, "z");
    }

    #[test]
    fn labels_read_naturally_in_diagnostics() {
        let entry = Entry {
            krate: "kernels".into(),
            kernel: Some("scale".into()),
            code: "RC005".into(),
            reason: "r".into(),
        };
        assert_eq!(entry.label(), "kernels RC005 in `scale`");
        let crate_level = Entry {
            kernel: None,
            ..entry
        };
        assert_eq!(crate_level.label(), "kernels RC005");
    }
}
