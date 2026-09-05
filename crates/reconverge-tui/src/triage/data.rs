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
    /// Why the baseline could not be read, when it exists and did not parse.
    ///
    /// `Some` means the file on disk holds review decisions this session
    /// cannot see, so writing would replace them with whatever the session
    /// *can* see — which was an empty document, reported as
    /// `baseline written`. The load stays lenient so the findings are still
    /// reviewable; the write is what is refused.
    pub baseline_unreadable: Option<String>,
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
        // One sniff, shared with the shell view: a document that does not
        // parse says so, with its position, instead of collapsing into
        // `unsupported schema ``` — a version statement about a file that
        // is merely damaged.
        let schema = match crate::load::sniff_schema(&name, &text) {
            Ok(schema) => schema,
            Err(e) => {
                data.errors.push(e);
                continue;
            }
        };
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
        Ok(text) => {
            match reconverge_artifacts::read::deserialize_checked::<BaselineArtifact>(&text) {
                Ok(mut parsed) => {
                    for entry in &mut parsed.entries {
                        entry.reason = nfc(&entry.reason);
                    }
                    parsed.normalize();
                    parsed
                }
                Err(e) => {
                    let message = format!(
                        "{}: not a baseline.v1 document: {e}",
                        display_name(baseline_path)
                    );
                    data.errors.push(message.clone());
                    data.baseline_unreadable = Some(message);
                    BaselineArtifact::empty()
                }
            }
        }
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

    #[test]
    fn a_damaged_findings_file_is_named_a_damaged_file() {
        let (dir, paths) = crate::load::tests_support::damaged_inputs("triage");
        let (data, _) = load(&paths, &fixture("baseline/minimal.json"));
        assert!(data.items.is_empty());
        assert_eq!(data.errors.len(), 4, "{:?}", data.errors);
        assert!(data.errors[0].contains("not JSON:"), "{:?}", data.errors);
        assert!(
            !data.errors.iter().any(|e| e.contains("schema ``")),
            "{:?}",
            data.errors
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A baseline that does not parse must leave the write refused, not
    /// merely visible. The load stays lenient so the findings are still
    /// reviewable; `baseline_unreadable` is what stops `w`.
    #[test]
    fn a_baseline_that_does_not_parse_blocks_the_write() {
        let dir = std::env::temp_dir().join(format!("rc-tui-badbl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Four corruption shapes, all reachable without doing anything
        // unusual: a truncated tail, a trailing comma, git conflict
        // markers, and a wrong-but-valid schema tag.
        for (name, body) in [
            (
                "trunc.json",
                "{\n \"schema\": \"baseline.v1\",\n \"entries\": [\n",
            ),
            (
                "comma.json",
                "{\"schema\":\"baseline.v1\",\"tool\":{\"name\":\"reconverge\",\
                 \"version\":\"0\"},\"entries\":[],}",
            ),
            (
                "conflict.json",
                "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> other\n",
            ),
            (
                "wrongtag.json",
                "{\"schema\":\"findings.v1\",\"tool\":{\"name\":\"x\",\"version\":\"9\"},\
                 \"entries\":[]}",
            ),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            let (data, baseline) = load(&[fixture("findings/rc003-minimal.json")], &path);
            assert!(
                !data.items.is_empty(),
                "{name}: the findings stay reviewable"
            );
            assert!(
                data.baseline_unreadable.is_some(),
                "{name}: the write must be refused"
            );
            assert!(
                data.errors.iter().any(|e| e.contains(name)),
                "{name}: and the reason must be on screen: {:?}",
                data.errors
            );
            assert!(baseline.entries.is_empty());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
