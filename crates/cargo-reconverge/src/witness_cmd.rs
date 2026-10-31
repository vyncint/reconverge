//! The `witness` subcommand: hand the witness artifacts of the last
//! `check` run to the reconverge TUI's 32-lane debugger.
//!
//! The CLI stays a thin launcher — the TUI is a separate binary and a pure
//! reader of artifacts, exactly like the architecture demands. Witness
//! artifacts exist only for findings the interpreter replayed (confirmed),
//! so "none found" gets a hint, not an empty screen.

use std::path::PathBuf;
use std::process::Command;

use reconverge_artifacts::findings::FindingsArtifact;

use crate::args::{self, ArgError};
use crate::check;
use crate::inspect::locate_tui;

pub struct WitnessOptions {
    pub ascii: bool,
    /// Only witnesses of this kernel (filename filter).
    pub kernel: Option<String>,
}

impl WitnessOptions {
    pub fn parse(args: &[String]) -> Result<WitnessOptions, ArgError> {
        let mut options = WitnessOptions {
            ascii: false,
            kernel: None,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = args::split_flag(arg);
            match flag {
                "--ascii" => {
                    args::reject_value("--ascii", inline_value)?;
                    options.ascii = true;
                }
                "--kernel" => {
                    options.kernel = Some(args::require_value("--kernel", inline_value, || {
                        iter.next().cloned()
                    })?);
                }
                // Below the value-taking flags, so `--baseline --help` is
                // still the missing value it was.
                flag if ArgError::help(flag) => return Err(ArgError::Help),
                flag if ArgError::version(flag) => return Err(ArgError::Version),
                other => return Err(ArgError::unknown(other)),
            }
        }
        Ok(options)
    }
}

/// Locate this project's witness artifacts and launch the debugger on
/// them. Returns the TUI's exit code.
pub fn run(options: &WitnessOptions) -> Result<u8, String> {
    let Some(artifacts) = discover_witnesses(options.kernel.as_deref())? else {
        // A check ran and replayed nothing to a confirmation. That is an
        // answer, not a failure: it used to be told to run `check` first,
        // exit 2, right after a `check` that had just passed.
        eprintln!(
            "reconverge: the last check confirmed no finding by replay, so \
             there is no witness to step through (one is written for every \
             finding the 32-lane interpreter confirms)"
        );
        return Ok(0);
    };
    let tui = locate_tui()?;

    let mut command = Command::new(tui);
    command.arg("witness");
    if options.ascii {
        command.arg("--ascii");
    }
    command.args(&artifacts);
    let status = command
        .status()
        .map_err(|e| format!("cannot launch reconverge-tui: {e}"))?;
    Ok(u8::try_from(status.code().unwrap_or(2)).unwrap_or(2))
}

/// The `(kernel, code)` pairs this run's findings artifacts still carry.
///
/// `None` when none of them can be read — an unreadable artifact is not
/// evidence that a witness is stale, so the filter stands down rather than
/// hiding everything.
fn live_findings(findings: &[PathBuf]) -> Option<Vec<(String, String)>> {
    let mut live = None;
    for path in findings {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(artifact) =
            reconverge_artifacts::read::deserialize_checked::<FindingsArtifact>(&text)
        else {
            continue;
        };
        let live = live.get_or_insert_with(Vec::new);
        for finding in artifact.findings {
            if let Some(kernel) = finding.kernel {
                live.push((kernel, finding.code));
            }
        }
    }
    live
}

/// The witness artifacts of the current workspace members' targets, from
/// `<target>/reconverge/`; `None` when the last check left nothing to
/// replay.
fn discover_witnesses(kernel: Option<&str>) -> Result<Option<Vec<PathBuf>>, String> {
    let metadata = check::cargo_metadata()?;
    let dir = metadata.target_directory.join("reconverge");
    let never_checked = || {
        format!(
            "no findings artifacts at {}; run `cargo reconverge check` first",
            dir.display()
        )
    };
    let findings = metadata
        .artifacts_in(&dir, &["findings"])
        .map_err(|_| never_checked())?;
    if findings.is_empty() {
        return Err(never_checked());
    }
    let mut artifacts = metadata
        .artifacts_in(&dir, &["witness"])
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    // Second filter, on the findings this run actually has. The driver
    // prunes its own witnesses now, which is the fix that also serves a
    // consumer reading the directory — but a file left by an older
    // reconverge is not the driver's to remove, and the reader should not
    // replay a finding that no longer exists whoever wrote it.
    if let Some(live) = live_findings(&findings) {
        artifacts.retain(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    live.iter()
                        .any(|(kernel, code)| name.contains(&format!("-{kernel}-{code}-")))
                })
        });
    }
    if artifacts.is_empty() {
        return Ok(None);
    }
    if let Some(k) = kernel {
        artifacts.retain(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.contains(&format!("-{k}-")))
        });
        if artifacts.is_empty() {
            return Err(format!(
                "no witness artifacts of kernel `{k}` at {}; drop `--kernel {k}` to see \
                 every witness",
                dir.display()
            ));
        }
    }
    Ok(Some(artifacts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn kernel_does_not_swallow_the_next_flag() {
        // `witness --kernel --ascii` used to treat `--ascii` as the kernel
        // name, then advise dropping `--kernel --ascii` to see every witness.
        let err = WitnessOptions::parse(&argv(&["--kernel", "--ascii"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--kernel` requires a value (got the flag `--ascii`)"
        );
        assert!(!err.wants_usage());
    }

    #[test]
    fn ascii_rejects_an_inline_value() {
        let err = WitnessOptions::parse(&argv(&["--ascii=false"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--ascii` takes no value");
        assert!(!err.wants_usage());
    }

    #[test]
    fn documented_flags_still_parse() {
        let options = WitnessOptions::parse(&argv(&["--ascii", "--kernel", "reduce"])).unwrap();
        assert!(options.ascii);
        assert_eq!(options.kernel.as_deref(), Some("reduce"));
    }
}
