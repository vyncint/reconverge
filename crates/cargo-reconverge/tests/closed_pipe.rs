//! Every stdout surface keeps its exit code when the reader has gone.
//!
//! `check` already did (`a_reader_that_closes_early_costs_the_report_and_not_the_verdict`
//! in `check_cli.rs`). `--explain`, `--help` and `--version` printed with
//! `print!`, which panics on a closed pipe, so `cargo reconverge --explain
//! RC001 | head -3` exited 101 with a panic notice.
//!
//! A binary of its own, holding a single test: the read end is closed
//! before the child is spawned, but a sibling test forking at the same
//! moment would inherit a copy of it until its own `exec`, the write would
//! succeed, and the test would pass without the fix. Alone here, nothing
//! else forks.

use std::process::{Command, Stdio};

#[test]
fn printing_into_a_closed_pipe_is_not_a_panic() {
    for args in [
        &["--explain", "RC001"][..],
        &["explain", "rc002"],
        &["--help"],
        &["help"],
        &["--version"],
        &["check", "--help"],
        &["check", "-V"],
    ] {
        let (reader, writer) = std::io::pipe().expect("pipe");
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .arg("reconverge")
            .args(args)
            .stdout(Stdio::from(writer))
            .stderr(Stdio::piped())
            .output()
            .expect("failed to spawn cargo-reconverge");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
            "a closed reader is not a failure ({args:?}):\n{stderr}"
        );
        assert_eq!(output.status.code(), Some(0), "({args:?}):\n{stderr}");
    }
}
