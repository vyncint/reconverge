//! The `inspect` subcommand: hand the artifacts of the last `check` run to
//! the reconverge TUI's Inspector.
//!
//! The CLI stays a thin launcher — the TUI is a separate binary and a pure
//! reader of artifacts, exactly like the architecture demands.

use std::path::PathBuf;
use std::process::Command;

use crate::args::ArgError;
use crate::check;

pub struct InspectOptions {
    pub ascii: bool,
}

impl InspectOptions {
    pub fn parse(args: &[String]) -> Result<InspectOptions, ArgError> {
        let mut options = InspectOptions { ascii: false };
        for arg in args {
            match arg.as_str() {
                "--ascii" => options.ascii = true,
                other => return Err(ArgError::unknown(other)),
            }
        }
        Ok(options)
    }
}

/// Locate this project's reconverge artifacts and launch the Inspector on
/// them. Returns the TUI's exit code.
pub fn run(options: &InspectOptions) -> Result<u8, String> {
    let artifacts = discover_artifacts()?;
    let tui = locate_tui()?;

    let mut command = Command::new(tui);
    command.arg("inspect");
    if options.ascii {
        command.arg("--ascii");
    }
    command.args(&artifacts);
    let status = command
        .status()
        .map_err(|e| format!("cannot launch reconverge-tui: {e}"))?;
    Ok(u8::try_from(status.code().unwrap_or(2)).unwrap_or(2))
}

/// The unimap and findings artifacts of the current workspace members,
/// from `<target>/reconverge/`.
fn discover_artifacts() -> Result<Vec<PathBuf>, String> {
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
            && (name.starts_with("unimap-") || name.starts_with("findings-"))
            && metadata
                .member_crates
                .iter()
                .any(|krate| name.contains(&format!("-{krate}-")));
        if recognized {
            artifacts.push(path);
        }
    }
    artifacts.sort();
    if artifacts.iter().all(|p| {
        !p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("unimap-"))
    }) {
        return Err(format!(
            "no unimap artifacts at {}; run `cargo reconverge check` first",
            dir.display()
        ));
    }
    Ok(artifacts)
}

/// The TUI binary: `$RECONVERGE_TUI`, or the sibling of this executable.
pub(crate) fn locate_tui() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("RECONVERGE_TUI") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("RECONVERGE_TUI={} does not exist", path.display()));
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    let sibling = exe
        .parent()
        .ok_or("cannot locate own directory")?
        .join(format!("reconverge-tui{}", std::env::consts::EXE_SUFFIX));
    if sibling.is_file() {
        return Ok(sibling);
    }
    Err(format!(
        "reconverge-tui not found next to this binary ({}); run \
         `cargo reconverge setup` to install the matching version — in a \
         source checkout, `cargo build -p reconverge-tui` — or set \
         RECONVERGE_TUI",
        sibling.display()
    ))
}
