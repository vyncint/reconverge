//! The `witness` subcommand: hand the witness artifacts of the last
//! `check` run to the reconverge TUI's 32-lane debugger.
//!
//! The CLI stays a thin launcher — the TUI is a separate binary and a pure
//! reader of artifacts, exactly like the architecture demands. Witness
//! artifacts exist only for findings the interpreter replayed (confirmed),
//! so "none found" gets a hint, not an empty screen.

use std::path::PathBuf;
use std::process::Command;

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
                other => return Err(ArgError::unknown(other)),
            }
        }
        Ok(options)
    }
}

/// Locate this project's witness artifacts and launch the debugger on
/// them. Returns the TUI's exit code.
pub fn run(options: &WitnessOptions) -> Result<u8, String> {
    let artifacts = discover_witnesses(options.kernel.as_deref())?;
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

/// The witness artifacts of the current workspace members, from
/// `<target>/reconverge/`.
fn discover_witnesses(kernel: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let metadata = check::cargo_metadata()?;
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
            && name.starts_with("witness-")
            && metadata
                .member_crates
                .iter()
                .any(|krate| name.contains(&format!("-{krate}-")))
            && kernel.is_none_or(|k| name.contains(&format!("-{k}-")));
        if recognized {
            artifacts.push(path);
        }
    }
    artifacts.sort();
    if artifacts.is_empty() {
        return Err(format!(
            "no witness artifacts at {}; they are written when a finding is \
             confirmed by replay — run `cargo reconverge check` first{}",
            dir.display(),
            kernel.map_or_else(String::new, |k| format!(
                " (or drop `--kernel {k}` to see every witness)"
            ))
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
