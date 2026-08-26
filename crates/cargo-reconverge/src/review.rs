//! The review layer: what the engine found, paired with what a human has
//! already reviewed and accepted.
//!
//! `findings.v1` is the analysis record and never changes here; the
//! baseline is policy applied on top of it (schemas/README.md). Everything that
//! decides *what a user sees and whether CI fails* lives in this one
//! place, so the text renderer, the SARIF writer, the triage view, and the
//! exit code cannot drift apart on it.

use std::fs;
use std::path::PathBuf;

use reconverge_artifacts::baseline::{BaselineArtifact, Entry};
use reconverge_artifacts::findings::{Confidence, Finding, FindingsArtifact};
use reconverge_artifacts::plural;

/// Default baseline filename, looked up at the workspace root.
pub const DEFAULT_BASELINE: &str = "reconverge-baseline.json";

/// Findings plus the reviewed baseline that applies to them.
#[derive(Debug)]
pub struct Review {
    pub artifacts: Vec<FindingsArtifact>,
    pub baseline: BaselineArtifact,
    /// Where the baseline lives (whether or not the file exists yet).
    pub baseline_path: PathBuf,
}

/// One finding in review context.
pub struct Item<'a> {
    pub finding: &'a Finding,
    /// The baseline entry accepting this finding, when there is one.
    pub suppression: Option<&'a Entry>,
}

impl Item<'_> {
    /// Whether this finding is displayed at the given strictness.
    #[must_use]
    pub fn shown(&self, strict: bool, show_suppressed: bool) -> bool {
        if self.suppression.is_some() {
            return show_suppressed;
        }
        strict || self.finding.confidence.shown_by_default()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub deny: usize,
    pub confirmed: usize,
    pub warning: usize,
    /// Findings of any tier accepted by the baseline (not counted above).
    pub suppressed: usize,
}

impl Review {
    /// Read the baseline at `path`. A missing default baseline is simply
    /// an empty one; a missing *explicitly requested* baseline is an error,
    /// because silently ignoring `--baseline` would hide findings the user
    /// believes are suppressed — or fail a build they believe is clean.
    pub fn load(
        artifacts: Vec<FindingsArtifact>,
        path: PathBuf,
        explicit: bool,
    ) -> Result<Review, String> {
        let baseline = match fs::read_to_string(&path) {
            Ok(text) => {
                let mut parsed: BaselineArtifact = serde_json::from_str(&text).map_err(|e| {
                    format!("{} is not a baseline.v1 document: {e}", path.display())
                })?;
                if let Some(bad) = parsed.entries.iter().find(|e| e.reason.trim().is_empty()) {
                    return Err(format!(
                        "{}: entry `{}` has no reason; every suppression must say why",
                        path.display(),
                        bad.label()
                    ));
                }
                parsed.normalize();
                parsed
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
                BaselineArtifact::empty()
            }
            // A named path that is not there is an error, while the default
            // being absent is not. The asymmetry is deliberate and worth
            // stating: treating a typo'd `--baseline` as empty would suppress
            // nothing while looking exactly like a run that suppressed
            // everything asked of it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "cannot read {}: {e}\n       a path given to --baseline must exist; only \
                     the default is treated as empty when absent, so a mistyped one cannot \
                     pass for a clean run",
                    path.display()
                ));
            }
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        Ok(Review {
            artifacts,
            baseline,
            baseline_path: path,
        })
    }

    /// Every finding, in artifact order, with its suppression status.
    pub fn items(&self) -> impl Iterator<Item = Item<'_>> {
        self.artifacts.iter().flat_map(|artifact| {
            artifact.findings.iter().map(|finding| Item {
                finding,
                suppression: self.baseline.suppression_of(&artifact.krate, finding),
            })
        })
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for item in self.items() {
            if item.suppression.is_some() {
                counts.suppressed += 1;
                continue;
            }
            match item.finding.confidence {
                Confidence::Deny => counts.deny += 1,
                Confidence::Confirmed => counts.confirmed += 1,
                Confidence::Warning => counts.warning += 1,
            }
        }
        counts
    }

    /// Exit 1 when an *unsuppressed* deny/confirmed finding remains.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        let gating = self
            .items()
            .any(|item| item.suppression.is_none() && item.finding.confidence.gates_exit_code());
        u8::from(gating)
    }

    /// Baseline entries that no longer match any finding. Reported, never
    /// fatal: they are the debt the ratchet is meant to surface, and the
    /// fix (delete the entry) belongs to a human.
    #[must_use]
    pub fn stale_entries(&self) -> Vec<&Entry> {
        self.baseline
            .entries
            .iter()
            .filter(|entry| {
                !self.artifacts.iter().any(|artifact| {
                    artifact.findings.iter().any(|finding| {
                        entry.covers(&artifact.krate, &finding.code, finding.kernel.as_deref())
                    })
                })
            })
            .collect()
    }
}

impl Counts {
    /// The one-line summary printed after the findings.
    #[must_use]
    pub fn summary_line(&self, strict: bool) -> String {
        use std::fmt::Write as _;
        let total = self.deny + self.confirmed + self.warning;
        let mut line = format!(
            "reconverge: {} deny, {} confirmed, {} warning {}",
            self.deny,
            self.confirmed,
            self.warning,
            plural(total, "finding", "findings")
        );
        if self.warning > 0 && !strict {
            let _ = write!(
                line,
                " ({} hidden; rerun with --strict to see {})",
                self.warning,
                plural(self.warning, "it", "them")
            );
        }
        if self.suppressed > 0 {
            let _ = write!(
                line,
                "; {} suppressed by the baseline (--show-suppressed to review)",
                self.suppressed
            );
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use reconverge_artifacts::findings::SourceSpan;

    use super::*;

    fn finding(code: &str, kernel: Option<&str>, confidence: Confidence) -> Finding {
        Finding {
            code: code.to_string(),
            confidence,
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

    fn review(findings: Vec<Finding>, entries: Vec<Entry>) -> Review {
        Review {
            artifacts: vec![FindingsArtifact::new("k", findings)],
            baseline: BaselineArtifact::new(entries),
            baseline_path: PathBuf::from(DEFAULT_BASELINE),
        }
    }

    fn entry(code: &str, kernel: Option<&str>) -> Entry {
        Entry {
            krate: "k".into(),
            kernel: kernel.map(str::to_string),
            code: code.to_string(),
            reason: "reviewed".into(),
        }
    }

    #[test]
    fn suppressed_findings_leave_the_gate_and_the_counts() {
        let findings = vec![
            finding("RC003", Some("a"), Confidence::Deny),
            finding("RC001", Some("b"), Confidence::Confirmed),
            finding("RC005", Some("c"), Confidence::Warning),
        ];
        let open = review(findings.clone(), Vec::new());
        assert_eq!(open.exit_code(), 1);
        assert_eq!(
            open.counts(),
            Counts {
                deny: 1,
                confirmed: 1,
                warning: 1,
                suppressed: 0
            }
        );

        // Accepting both gating findings clears the exit code; the warning
        // is untouched and still counted.
        let reviewed = review(
            findings,
            vec![entry("RC003", Some("a")), entry("RC001", Some("b"))],
        );
        assert_eq!(reviewed.exit_code(), 0);
        assert_eq!(
            reviewed.counts(),
            Counts {
                deny: 0,
                confirmed: 0,
                warning: 1,
                suppressed: 2
            }
        );
    }

    #[test]
    fn summary_line_always_reports_suppressions() {
        let counts = Counts {
            deny: 0,
            confirmed: 0,
            warning: 2,
            suppressed: 3,
        };
        let line = counts.summary_line(false);
        assert!(line.contains("2 warning findings (2 hidden"), "{line}");
        assert!(line.contains("3 suppressed by the baseline"), "{line}");
        // Even in strict mode the suppressed count stays visible: a
        // suppression is never invisible, only unobtrusive.
        assert!(counts.summary_line(true).contains("3 suppressed"));
        assert!(!Counts::default().summary_line(false).contains("suppressed"));
    }

    #[test]
    fn stale_entries_are_reported_not_fatal() {
        let reviewed = review(
            vec![finding("RC003", Some("a"), Confidence::Deny)],
            vec![entry("RC003", Some("a")), entry("RC004", Some("gone"))],
        );
        let stale = reviewed.stale_entries();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].code, "RC004");
        assert_eq!(reviewed.exit_code(), 0, "stale entries never gate");
    }

    #[test]
    fn items_report_display_eligibility_per_mode() {
        let reviewed = review(
            vec![
                finding("RC003", Some("a"), Confidence::Deny),
                finding("RC005", Some("c"), Confidence::Warning),
            ],
            vec![entry("RC003", Some("a"))],
        );
        let items: Vec<Item<'_>> = reviewed.items().collect();
        assert_eq!(items.len(), 2);
        // Suppressed: hidden unless explicitly reviewed, at any strictness.
        assert!(!items[0].shown(false, false));
        assert!(!items[0].shown(true, false));
        assert!(items[0].shown(false, true));
        // Warning: the usual --strict rule, unaffected by --show-suppressed.
        assert!(!items[1].shown(false, false));
        assert!(items[1].shown(true, false));
    }

    #[test]
    fn a_missing_explicit_baseline_is_an_error_but_a_missing_default_is_not() {
        let missing = PathBuf::from("definitely/not/here.json");
        assert!(Review::load(Vec::new(), missing.clone(), false).is_ok());
        let err = Review::load(Vec::new(), missing, true).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
        // The asymmetry is deliberate, so the message has to say so: a reader
        // who has just been told the default is fine when absent will
        // otherwise read this as a contradiction rather than as a guard
        // against a typo passing for a clean run.
        assert!(
            err.contains("only the default is treated as empty when absent"),
            "the error explains why the two cases differ: {err}"
        );
    }

    #[test]
    fn baselines_round_trip_through_disk_and_reject_empty_reasons() {
        let dir = std::env::temp_dir().join(format!("rc-review-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(DEFAULT_BASELINE);

        // The writer under test is the shared one in reconverge-artifacts,
        // so this also pins that the CLI can read what triage writes.
        let baseline = BaselineArtifact::new(vec![entry("RC003", Some("a"))]);
        baseline.write_to(&path).unwrap();
        let loaded = Review::load(Vec::new(), path.clone(), true).unwrap();
        assert_eq!(loaded.baseline, baseline);

        let mut bad = baseline;
        bad.entries[0].reason = "   ".into();
        bad.write_to(&path).unwrap();
        let err = Review::load(Vec::new(), path, true).unwrap_err();
        assert!(err.contains("must say why"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }
}
