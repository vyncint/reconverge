//! End-to-end: `cargo reconverge learn` opens the embedded lessons —
//! in an EMPTY directory, with a scrubbed environment, no artifacts, no
//! cargo metadata, no network. The offline gate, proven through the whole
//! CLI. Sync per §9: content-based `wait_until`, never sleep.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Build the TUI binary on demand (same shape as check_cli's
/// `ensure_driver`; integration tests of one crate do not build another
/// crate's binaries by themselves).
fn ensure_tui() -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    let path = cli
        .parent()
        .unwrap()
        .join(format!("reconverge-tui{}", std::env::consts::EXE_SUFFIX));
    if !path.is_file() {
        let status = Command::new(env::var("CARGO").unwrap())
            .args(["build", "-p", "reconverge-tui"])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .expect("failed to spawn cargo build for the TUI");
        assert!(status.success(), "building reconverge-tui failed");
    }
    path
}

#[test]
fn learn_opens_offline_in_an_empty_directory() {
    let tui = ensure_tui();
    let empty = Path::new(env!("CARGO_TARGET_TMPDIR")).join("t3-learn-empty");
    let _ = fs::remove_dir_all(&empty);
    fs::create_dir_all(&empty).unwrap();

    // Only RECONVERGE_TUI crosses into the scrubbed environment: learn
    // needs no cargo, no PATH, no artifacts — everything is embedded.
    let mut t = Terminal::builder()
        .size(80, 24)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(&empty)
        .env("RECONVERGE_TUI", tui.to_str().unwrap())
        .arg("reconverge")
        .arg("learn")
        .spawn(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .expect("failed to spawn cargo reconverge learn in a PTY");

    t.wait_until(|s| {
        s.contains("reconverge learn")
            && s.contains("no GPU required")
            && s.contains("4. reconvergence")
    })
    .expect("the lesson list opens with nothing on disk");

    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("lesson 1/4") && s.contains("SIMT"))
        .expect("the divergence lesson opens");

    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("learn did not exit after q");
    assert!(status.success(), "learn subcommand exited with {status:?}");
}
