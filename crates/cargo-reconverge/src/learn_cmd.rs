//! The `learn` subcommand: launch the TUI's four SIMT lessons.
//!
//! Deliberately the thinnest launcher of all: the lessons are embedded in
//! the TUI binary (prose, kernels, fixture witnesses), so there is no
//! artifact discovery, no cargo metadata, no analysis step — `cargo
//! reconverge learn` works in an empty directory on an offline machine.

use std::process::Command;

use crate::inspect::locate_tui;

pub struct LearnOptions {
    pub ascii: bool,
}

impl LearnOptions {
    pub fn parse(args: &[String]) -> Result<LearnOptions, String> {
        let mut options = LearnOptions { ascii: false };
        for arg in args {
            match arg.as_str() {
                "--ascii" => options.ascii = true,
                other => return Err(format!("unrecognized argument `{other}`")),
            }
        }
        Ok(options)
    }
}

/// Launch learn mode. Returns the TUI's exit code.
pub fn run(options: &LearnOptions) -> Result<u8, String> {
    let tui = locate_tui()?;
    let mut command = Command::new(tui);
    command.arg("learn");
    if options.ascii {
        command.arg("--ascii");
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot launch reconverge-tui: {e}"))?;
    Ok(u8::try_from(status.code().unwrap_or(2)).unwrap_or(2))
}
