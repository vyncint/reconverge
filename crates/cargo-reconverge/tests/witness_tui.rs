//! End-to-end: `cargo reconverge check` emits real `witness.v1`
//! artifacts, and `cargo reconverge witness` steps through them in the
//! debugger — the whole wire, from analysis to the 32-lane replay on
//! screen, on the canonical RC001 kernel. Sync policy per §9: content-based
//! `wait_until` only, never sleep.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(30);

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Build a sibling binary of the CLI on demand (mirrors `ensure_driver`
/// in check_cli.rs; an isolated `cargo test -p cargo-reconverge` does not
/// build other crates' binaries by itself).
fn ensure_sibling(package: &str, binary: &str) -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    let path = cli
        .parent()
        .unwrap()
        .join(format!("{binary}{}", std::env::consts::EXE_SUFFIX));
    if !path.is_file() {
        let status = Command::new(env::var("CARGO").unwrap())
            .args(["build", "-p", package])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .unwrap_or_else(|e| panic!("failed to spawn cargo build for {package}: {e}"));
        assert!(status.success(), "building {package} failed");
    }
    path
}

#[test]
fn witness_subcommand_replays_the_real_rc001_artifact() {
    let driver = ensure_sibling("reconverge-driver", "reconverge-driver");
    let tui = ensure_sibling("reconverge-tui", "reconverge-tui");

    // A fresh copy of the samples, checked for real: the witness artifacts
    // this test debugs are produced by this run, not fixtures.
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("t2-witness-samples");
    let _ = fs::remove_dir_all(project.join("src"));
    let _ = fs::remove_file(project.join("Cargo.toml"));
    let _ = fs::remove_file(project.join("Cargo.lock"));
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
        &project,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check"])
        .current_dir(&project)
        .env("RECONVERGE_DRIVER", &driver)
        .output()
        .expect("failed to spawn cargo-reconverge check");
    assert_eq!(
        output.status.code(),
        Some(1),
        "the samples' gating findings exit 1: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // `cargo reconverge witness --kernel …` discovers the emitted artifact
    // (via cargo metadata, hence the passed-through environment) and
    // launches the debugger on it.
    let mut builder = Terminal::builder()
        .size(80, 24)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(&project)
        .env("RECONVERGE_TUI", tui.to_str().unwrap())
        .arg("reconverge")
        .arg("witness")
        .arg("--kernel")
        .arg("rc001_divergent_barrier");
    for var in ["PATH", "HOME", "CARGO", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Ok(value) = env::var(var) {
            builder = builder.env(var, &value);
        }
    }
    let mut t = builder
        .spawn(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .expect("failed to spawn cargo reconverge witness in a PTY");

    t.wait_until(|s| {
        // The 80-column header ends in an ellipsis, so only assert what
        // fits: witness count, kernel, code, and the start of the launch.
        s.contains("witness 1/1")
            && s.contains("kernel `rc001_divergent_barrier`")
            && s.contains("RC001")
            && s.contains("grid (1,1,1)")
    })
    .expect("the real artifact opens in the debugger");

    // v: the verdict moment — the even lanes wait forever, the diagnostics'
    // warp diagram appears as the live lane strip.
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    t.wait_until(|s| {
        s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
            && s.contains("verdict: undefined behavior")
            && s.contains("usually a permanent hang")
    })
    .expect("the replay reaches its calibrated verdict");

    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("debugger did not exit after q");
    assert!(
        status.success(),
        "witness subcommand exited with {status:?}"
    );

    // The RC002 replay from the same run: at the collective, the mask
    // panel shows the full mask against the 16 arriving lanes, with the
    // named-but-absent lanes computed.
    let mut builder = Terminal::builder()
        .size(80, 24)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(&project)
        .env("RECONVERGE_TUI", tui.to_str().unwrap())
        .arg("reconverge")
        .arg("witness")
        .arg("--kernel")
        .arg("rc002_divergent_collective");
    for var in ["PATH", "HOME", "CARGO", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Ok(value) = env::var(var) {
            builder = builder.env(var, &value);
        }
    }
    let mut t = builder
        .spawn(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .expect("failed to spawn the RC002 witness in a PTY");
    t.wait_until(|s| s.contains("kernel `rc002_divergent_collective`") && s.contains("RC002"))
        .expect("the RC002 artifact opens");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    t.wait_until(|s| s.contains("verdict: undefined behavior") && s.contains("never finishes"))
        .expect("the RC002 verdict lands");
    // h: back one event to the collective itself, where the mask panel
    // shows the full mask against the arriving lanes.
    t.send(Key::Char('h')).expect("send Key::Char('h')");
    t.wait_until(|s| {
        s.contains("0xffffffff")
            && s.contains("0x55555555")
            && s.contains("named in the mask but not active: 0xaaaaaaaa")
    })
    .expect("the RC002 mask panel shows the real mismatch");
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("debugger did not exit after q");
    assert!(status.success(), "RC002 witness exited with {status:?}");

    // Asking for a kernel with no witness is a tool error with a hint, not
    // an empty screen.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "witness", "--kernel", "no_such_kernel"])
        .current_dir(&project)
        .env("RECONVERGE_TUI", tui.to_str().unwrap())
        .output()
        .expect("failed to spawn cargo-reconverge witness");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no witness artifacts") && stderr.contains("--kernel no_such_kernel"),
        "{stderr}"
    );
}
