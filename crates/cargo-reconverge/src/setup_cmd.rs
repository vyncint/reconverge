//! The `setup` subcommand: install the driver and TUI that match this CLI.
//!
//! `cargo install cargo-reconverge` delivers only this binary. The analysis
//! needs `reconverge-driver` — a rustc driver, which must be built by the
//! exact nightly it wraps — and the terminal views need `reconverge-tui`.
//! Rather than asking every user to reproduce that incantation, `setup`
//! runs it: the pinned toolchain with the components the driver links
//! against, then both companion crates at this CLI's **own version**, so
//! the three binaries cannot drift apart.
//!
//! Everything it does is printed before it runs, and every failure ends
//! with the manual commands, so `setup` is a convenience — never a
//! mystery.

use crate::args::ArgError;
use std::process::Command;

/// The nightly the driver wraps, in lockstep with upstream cuda-oxide's
/// own pin. `rust-toolchain.toml` at the repository root is the source of
/// truth; a test below fails in-repo if the two ever drift.
pub const PINNED_TOOLCHAIN: &str = "nightly-2026-04-03";

pub struct SetupOptions {}

impl SetupOptions {
    pub fn parse(args: &[String]) -> Result<SetupOptions, ArgError> {
        match args.first() {
            None => Ok(SetupOptions {}),
            Some(other) => Err(ArgError::unknown(other)),
        }
    }
}

/// The commands `setup` runs, in order. A pure plan so tests can hold it
/// to the contract: pinned toolchain, driver components, version-matched
/// companions, locked dependencies.
fn plan() -> [Vec<String>; 2] {
    let version = env!("CARGO_PKG_VERSION");
    let argv = |args: &[&str]| args.iter().map(ToString::to_string).collect::<Vec<_>>();
    [
        argv(&[
            "rustup",
            "toolchain",
            "install",
            PINNED_TOOLCHAIN,
            "--profile",
            "minimal",
            "--component",
            "rustc-dev",
            "--component",
            "llvm-tools",
        ]),
        argv(&[
            "rustup",
            "run",
            PINNED_TOOLCHAIN,
            "cargo",
            "install",
            "--locked",
            &format!("reconverge-driver@{version}"),
            &format!("reconverge-tui@{version}"),
        ]),
    ]
}

/// Run the plan. Returns the CLI exit code.
pub fn run(_options: &SetupOptions) -> Result<u8, String> {
    for command in plan() {
        eprintln!("setup: running `{}`", command.join(" "));
        let status = Command::new(&command[0])
            .args(&command[1..])
            .status()
            .map_err(|e| {
                format!(
                    "cannot run `{}`: {e}\nsetup needs rustup (https://rustup.rs); \
                     the manual steps are:\n{}",
                    command[0],
                    manual_steps()
                )
            })?;
        if !status.success() {
            return Err(format!(
                "`{}` failed; the manual steps are:\n{}",
                command.join(" "),
                manual_steps()
            ));
        }
    }
    eprintln!(
        "setup: done — reconverge-driver and reconverge-tui {} are installed",
        env!("CARGO_PKG_VERSION")
    );
    Ok(0)
}

/// What to paste if `setup` cannot finish the job itself.
fn manual_steps() -> String {
    plan()
        .iter()
        .map(|command| format!("  {}", command.join(" ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_pins_toolchain_version_and_lockfile() {
        let [toolchain, install] = plan();
        assert_eq!(toolchain[0], "rustup");
        assert!(toolchain.contains(&PINNED_TOOLCHAIN.to_string()));
        assert!(toolchain.contains(&"rustc-dev".to_string()));

        let version = env!("CARGO_PKG_VERSION");
        assert!(install.contains(&format!("reconverge-driver@{version}")));
        assert!(install.contains(&format!("reconverge-tui@{version}")));
        assert!(install.contains(&"--locked".to_string()));
        // The companions are built by the pinned toolchain, not whatever
        // toolchain happens to be ambient.
        assert_eq!(&install[..3], &["rustup", "run", PINNED_TOOLCHAIN]);
    }

    #[test]
    fn the_embedded_pin_matches_rust_toolchain_toml() {
        // Only meaningful in a source checkout, which is exactly where the
        // pin could drift; the published package has no toolchain file.
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        assert!(
            text.contains(&format!("channel = \"{PINNED_TOOLCHAIN}\"")),
            "PINNED_TOOLCHAIN drifted from rust-toolchain.toml"
        );
    }

    #[test]
    fn setup_takes_no_arguments() {
        assert!(SetupOptions::parse(&[]).is_ok());
        assert!(SetupOptions::parse(&["--force".into()]).is_err());
    }

    #[test]
    fn failure_guidance_is_pasteable() {
        let steps = manual_steps();
        assert!(steps.contains("rustup toolchain install"));
        assert!(steps.contains("cargo install --locked"));
    }
}
