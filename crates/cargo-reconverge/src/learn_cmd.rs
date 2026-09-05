//! The `learn` subcommand: launch the TUI's four SIMT lessons.
//!
//! Deliberately the thinnest launcher of all: the lessons are embedded in
//! the TUI binary (prose, kernels, fixture witnesses), so there is no
//! artifact discovery, no cargo metadata, no analysis step — `cargo
//! reconverge learn` works in an empty directory on an offline machine.

use std::process::Command;

use crate::args::{self, ArgError};
use crate::inspect::locate_tui;

pub struct LearnOptions {
    pub ascii: bool,
}

impl LearnOptions {
    pub fn parse(args: &[String]) -> Result<LearnOptions, ArgError> {
        let mut options = LearnOptions { ascii: false };
        for arg in args {
            let (flag, inline_value) = args::split_flag(arg);
            match flag {
                "--ascii" => {
                    args::reject_value("--ascii", inline_value)?;
                    options.ascii = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn ascii_rejects_an_inline_value() {
        let err = LearnOptions::parse(&argv(&["--ascii=false"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--ascii` takes no value");
        assert!(!err.wants_usage());
        assert!(LearnOptions::parse(&argv(&["--ascii"])).unwrap().ascii);
    }
}
