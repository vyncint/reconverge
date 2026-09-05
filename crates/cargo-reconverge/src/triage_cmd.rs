//! The `triage` subcommand: review the last check's findings and record
//! the accepted ones in the baseline.
//!
//! The CLI locates the artifacts and names the baseline path; the TUI does
//! the reviewing and writes that one file. Nothing else is ever written.

use std::path::PathBuf;
use std::process::Command;

use crate::args::{self, ArgError};
use crate::check;
use crate::inspect::locate_tui;
use crate::review::{DEFAULT_BASELINE, Review};

pub struct TriageOptions {
    pub ascii: bool,
    pub baseline: Option<PathBuf>,
}

impl TriageOptions {
    pub fn parse(args: &[String]) -> Result<TriageOptions, ArgError> {
        let mut options = TriageOptions {
            ascii: false,
            baseline: None,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = args::split_flag(arg);
            match flag {
                "--ascii" => {
                    args::reject_value("--ascii", inline_value)?;
                    options.ascii = true;
                }
                "--baseline" => {
                    options.baseline = Some(PathBuf::from(args::require_value(
                        "--baseline",
                        inline_value,
                        || iter.next().cloned(),
                    )?));
                }
                // Below the value-taking flags, so `--baseline --help` is
                // still the missing value it was.
                flag if ArgError::help(flag) => return Err(ArgError::Help),
                other => return Err(ArgError::unknown(other)),
            }
        }
        Ok(options)
    }
}

/// Launch the triage view on the last check's findings. Returns the TUI's
/// exit code.
pub fn run(options: &TriageOptions) -> Result<u8, String> {
    let metadata = check::cargo_metadata()?;
    let baseline = options
        .baseline
        .clone()
        .unwrap_or_else(|| metadata.workspace_root.join(DEFAULT_BASELINE));
    // The same file, behind the same flag, must mean the same thing to both
    // subcommands. `check` exits 2 naming the path and the serde error;
    // `triage` opened on it looking entirely normal, and one keystroke
    // replaced every reviewed acceptance with an empty document — which is
    // the loop a maintainer walks precisely *because* `check` complained
    // about that file. `Review::load` is the reader and the wording both.
    Review::load(Vec::new(), baseline.clone(), options.baseline.is_some())?;
    let artifacts = discover_findings(&metadata)?;
    let tui = locate_tui()?;

    let mut command = Command::new(tui);
    command.arg("triage").arg("--baseline").arg(&baseline);
    if options.ascii {
        command.arg("--ascii");
    }
    command.args(&artifacts);
    let status = command
        .status()
        .map_err(|e| format!("cannot launch reconverge-tui: {e}"))?;
    Ok(u8::try_from(status.code().unwrap_or(2)).unwrap_or(2))
}

/// The findings artifacts of the current workspace members.
fn discover_findings(metadata: &check::Metadata) -> Result<Vec<PathBuf>, String> {
    let dir = metadata.target_directory.join("reconverge");
    let mut artifacts = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|_| {
        format!(
            "no artifacts at {}; run `cargo reconverge check` first",
            dir.display()
        )
    })?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let recognized = name.ends_with(".json")
            && name.starts_with("findings-")
            && metadata
                .member_crates
                .iter()
                .any(|krate| name.contains(&format!("-{krate}-")));
        if recognized {
            artifacts.push(path);
        }
    }
    artifacts.sort();
    if artifacts.is_empty() {
        return Err(format!(
            "no findings artifacts at {}; run `cargo reconverge check` first",
            dir.display()
        ));
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn baseline_does_not_swallow_the_next_flag() {
        let err = TriageOptions::parse(&argv(&["--baseline", "--ascii"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--baseline` requires a value (got the flag `--ascii`)"
        );
        assert!(!err.wants_usage());
    }

    #[test]
    fn ascii_rejects_an_inline_value() {
        let err = TriageOptions::parse(&argv(&["--ascii=false"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--ascii` takes no value");
        assert!(!err.wants_usage());
    }

    #[test]
    fn documented_flags_still_parse() {
        let options = TriageOptions::parse(&argv(&["--ascii", "--baseline", "bl.json"])).unwrap();
        assert!(options.ascii);
        assert_eq!(
            options.baseline.as_deref(),
            Some(std::path::Path::new("bl.json"))
        );
    }
}
