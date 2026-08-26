//! `cargo reconverge` — the reconverge CLI.
//!
//! `check` runs the analysis and owns the exit-code contract: 0 = clean,
//! 1 = findings at deny/confirmed confidence, 2 = tool error. `--explain` prints a diagnostic's page; the rest are launchers
//! and loops around that one analysis — `inspect`, `witness`, `learn`, and
//! `triage` hand artifacts to the TUI, and `watch` re-runs `check` on
//! every save.
//!
//! Every rendering decision (what is shown, what gates CI, what the
//! baseline accepts) lives in [`review`], so text, SARIF, and the exit
//! code cannot disagree.

#![forbid(unsafe_code)]

mod args;
mod check;
mod explain;
mod inspect;
mod learn_cmd;
mod render;
mod review;
mod sarif;
mod setup_cmd;
mod triage_cmd;
mod watch_cmd;
mod witness_cmd;

use std::process::ExitCode;

use crate::args::ArgError;

const USAGE: &str = "\
cargo-reconverge: static reconvergence analysis for Rust GPU kernels

Usage:
  cargo reconverge setup               install the matching reconverge-driver
                                       and reconverge-tui (run once after
                                       `cargo install cargo-reconverge`)
  cargo reconverge check [OPTIONS]     analyze the current project
  cargo reconverge inspect [--ascii]   browse the last check's uniformity
                                       map and findings in the Inspector
  cargo reconverge witness [OPTIONS]   step through the last check's
                                       confirmed replays, 32 lanes at a
                                       time (--ascii, --kernel <name>)
  cargo reconverge learn               four interactive SIMT lessons —
                                       divergence, barriers, masks,
                                       reconvergence — fully offline
                                       (--ascii)
  cargo reconverge triage [OPTIONS]    review findings and record the
                                       accepted ones in the baseline
                                       (--ascii, --baseline <path>)
  cargo reconverge watch [OPTIONS]     re-run the check on every save;
                                       takes every check option plus
                                       --max-runs <N>
  cargo reconverge --explain <RCxxx>   print the explain page for a
                                       diagnostic code

check options:
  --strict                    also display warning-confidence findings
                              (they never affect the exit code)
  --cc <X.Y>                  target compute capability for shared-memory
                              capacity context (e.g. 8.6)
  --message-format <FORMAT>   text (default) or json; json prints one
                              findings.v1 document per analyzed crate, one
                              per line
  --sarif <PATH>              also write a SARIF 2.1.0 report to PATH
  --baseline <PATH>           reviewed suppressions to apply. The default,
                              reconverge-baseline.json at the workspace
                              root, is treated as empty when absent; a path
                              named here must exist
  --show-suppressed           display findings the baseline accepts,
                              with their recorded reasons

Exit codes: 0 = no findings at deny/confirmed confidence, 1 = findings,
2 = tool error. Findings accepted by the baseline never gate the exit
code, and their count is always reported.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(run(&args))
}

/// Report a command-line error, with the usage text only where it earns its
/// place.
///
/// An argument nobody recognises needs the reference; a recognised argument
/// with an unusable value does not, because the message already names what is
/// accepted. Printing it regardless put forty-odd lines between the reader
/// and the answer — and put the exit-code legend, rather than the reason, at
/// the end of stderr where a calling tool looks for it.
fn report_arg_error(err: &ArgError) -> u8 {
    if err.wants_usage() {
        eprintln!("error: {err}\n\n{USAGE}");
    } else {
        eprintln!("error: {err}");
    }
    2
}

/// Exit codes follow the CLI contract of README.md: 0 = clean,
/// 1 = findings at deny/confirmed, 2 = tool error.
fn run(args: &[String]) -> u8 {
    // Invoked as `cargo reconverge …`, cargo passes `reconverge` as the
    // first argument; invoked directly, it is absent.
    let rest = match args.first().map(String::as_str) {
        Some("reconverge") => &args[1..],
        _ => args,
    };
    match rest.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("cargo-reconverge {}", env!("CARGO_PKG_VERSION"));
            0
        }
        None | Some("--help" | "-h") => {
            print!("{USAGE}");
            0
        }
        Some("check") => match check::CheckOptions::parse(&rest[1..]) {
            Ok(options) => match check::run(&options) {
                Ok(outcome) => outcome.exit_code(),
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("--explain" | "explain") => match explain::parse(&rest[1..]) {
            Ok(code) => explain::run(&code),
            Err(err) => report_arg_error(&err),
        },
        Some("inspect") => match inspect::InspectOptions::parse(&rest[1..]) {
            Ok(options) => match inspect::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("setup") => match setup_cmd::SetupOptions::parse(&rest[1..]) {
            Ok(options) => match setup_cmd::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("triage") => match triage_cmd::TriageOptions::parse(&rest[1..]) {
            Ok(options) => match triage_cmd::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("watch") => match watch_cmd::WatchOptions::parse(&rest[1..]) {
            Ok(options) => match watch_cmd::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("learn") => match learn_cmd::LearnOptions::parse(&rest[1..]) {
            Ok(options) => match learn_cmd::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some("witness") => match witness_cmd::WitnessOptions::parse(&rest[1..]) {
            Ok(options) => match witness_cmd::run(&options) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err}");
                    2
                }
            },
            Err(err) => report_arg_error(&err),
        },
        Some(other) => {
            eprintln!("error: unrecognized argument `{other}`\n\n{USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn version_exits_zero_under_cargo_and_direct_invocation() {
        assert_eq!(run(&argv(&["reconverge", "--version"])), 0);
        assert_eq!(run(&argv(&["--version"])), 0);
        assert_eq!(run(&argv(&["reconverge", "-V"])), 0);
    }

    #[test]
    fn unknown_arguments_are_a_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "frobnicate"])), 2);
    }

    #[test]
    fn explain_dispatches_for_known_codes_only() {
        assert_eq!(run(&argv(&["reconverge", "--explain", "RC001"])), 0);
        assert_eq!(run(&argv(&["explain", "RC005"])), 0);
        assert_eq!(run(&argv(&["reconverge", "--explain"])), 2);
        assert_eq!(run(&argv(&["reconverge", "--explain", "RC999"])), 2);
        assert_eq!(
            run(&argv(&["reconverge", "--explain", "RC001", "extra"])),
            2
        );
    }

    #[test]
    fn check_rejects_bad_flags_as_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "check", "--bogus"])), 2);
        assert_eq!(run(&argv(&["reconverge", "check", "--cc"])), 2);
        assert_eq!(run(&argv(&["reconverge", "check", "--cc", "nope"])), 2);
        assert_eq!(
            run(&argv(&["reconverge", "check", "--message-format", "yaml"])),
            2
        );
    }

    #[test]
    fn witness_rejects_bad_flags_as_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "witness", "--bogus"])), 2);
        assert_eq!(run(&argv(&["reconverge", "witness", "--kernel"])), 2);
    }

    #[test]
    fn learn_rejects_bad_flags_as_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "learn", "--bogus"])), 2);
    }

    #[test]
    fn setup_rejects_arguments_as_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "setup", "--bogus"])), 2);
    }

    #[test]
    fn triage_and_watch_reject_bad_flags_as_tool_error() {
        assert_eq!(run(&argv(&["reconverge", "triage", "--bogus"])), 2);
        assert_eq!(run(&argv(&["reconverge", "triage", "--baseline"])), 2);
        assert_eq!(run(&argv(&["reconverge", "watch", "--max-runs", "0"])), 2);
        assert_eq!(run(&argv(&["reconverge", "watch", "--cc", "nope"])), 2);
    }
}
