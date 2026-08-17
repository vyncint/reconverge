//! The `triage` subcommand: review the last check's findings and record
//! the accepted ones in the baseline.
//!
//! The CLI locates the artifacts and names the baseline path; the TUI does
//! the reviewing and writes that one file. Nothing else is ever written.

use std::path::PathBuf;
use std::process::Command;

use crate::check;
use crate::inspect::locate_tui;
use crate::review::DEFAULT_BASELINE;

pub struct TriageOptions {
    pub ascii: bool,
    pub baseline: Option<PathBuf>,
}

impl TriageOptions {
    pub fn parse(args: &[String]) -> Result<TriageOptions, String> {
        let mut options = TriageOptions {
            ascii: false,
            baseline: None,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (arg.as_str(), None),
            };
            let mut value = |name: &str| {
                inline_value
                    .clone()
                    .or_else(|| iter.next().cloned())
                    .ok_or_else(|| format!("`{name}` requires a value"))
            };
            match flag {
                "--ascii" => options.ascii = true,
                "--baseline" => options.baseline = Some(PathBuf::from(value("--baseline")?)),
                other => return Err(format!("unrecognized argument `{other}`")),
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
