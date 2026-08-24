//! Smoke tests — the flakiness gate (see docs/ARCHITECTURE.md).
//!
//! Spawns the real shell binary in a real PTY on the shipped fixtures,
//! waits for a **complete frame**, compares the rendered screen against a
//! checked-in golden, and quits cleanly — 50 times in a row.
//!
//! It used to sync on a 150ms quiet period, which is a guess about how long
//! a repaint takes, and on a loaded macOS runner it was the wrong guess: the
//! gate failed on 2026-08-24 at both 2 and 16 threads, against goldens that
//! were correct, because `wait_idle` returned mid-repaint and the frame was
//! torn. The shell now brackets every repaint in DEC 2026 synchronized
//! updates (`sync_draw` in `main.rs`), so `wait_frame` can wait for the
//! repaint to *finish* rather than for the output to go quiet. No duration
//! is involved, so there is no duration to get wrong.
//!
//! Sync policy: `wait_frame` / `wait_until` — never `wait_idle`, never sleep.
//!
//! To regenerate a golden after an intentional UI change:
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test smoke`
//! then review the diff like code.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// Compare with per-line right-trim on both sides: terminal grids pad rows
/// with spaces, and editors are allowed to strip trailing whitespace from
/// the checked-in goldens.
fn normalize(frame: &str) -> String {
    frame
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_golden(name: &str, screen: &str, context: &str) {
    let path = golden_path(name);
    let actual = normalize(screen);
    if env::var_os("RECONVERGE_BLESS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{actual}\n")).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; bless with RECONVERGE_BLESS=1"));
    assert_eq!(
        normalize(&expected),
        actual,
        "{context}: frame differs from golden {name}\n--- rendered ---\n{screen}"
    );
}

fn spawn_shell(size: (u16, u16), extra_env: &[(&str, &str)], args: &[&str]) -> Terminal {
    let mut builder = Terminal::builder()
        .size(size.0, size.1)
        .env_clear()
        .timeout(TIMEOUT);
    for (key, value) in extra_env {
        builder = builder.env(key, value);
    }
    for arg in args {
        builder = builder.arg(arg);
    }
    builder
        .arg(fixture("findings/rc003-minimal.json"))
        .arg(fixture("unimap/divergent-barrier.json"))
        .arg(fixture("witness/rc001-divergent-barrier.json"))
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .expect("failed to spawn the shell in a PTY")
}

/// The first complete repaint, as text.
///
/// `q quit` sits in the footer, which the shell draws last, so a frame
/// carrying it carries everything above it too — and `wait_frame` only ever
/// evaluates whole frames, so there is no half-painted screen to catch.
fn first_frame(t: &mut Terminal, context: &str) -> String {
    t.wait_frame(|screen| screen.contains("q quit"))
        .unwrap_or_else(|e| panic!("{context}: waiting for the first complete frame: {e}"))
        .to_string()
}

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("shell did not exit after q");
    assert!(status.success(), "{context}: shell exited with {status:?}");
}

/// The flakiness gate: 50 consecutive spawn → frame → golden → quit cycles.
#[test]
fn shell_smoke_50_runs_at_80x24() {
    for run in 0..50 {
        let mut t = spawn_shell((80, 24), &[], &[]);
        let screen = first_frame(&mut t, &format!("run {run}"));
        assert_golden("shell-80x24.txt", &screen, &format!("run {run}"));
        quit(t, &format!("run {run}"));
    }
}

/// Deterministic layout at the second mandated geometry (docs/ARCHITECTURE.md).
#[test]
fn shell_is_deterministic_at_120x40() {
    let mut t = spawn_shell((120, 40), &[], &[]);
    let screen = first_frame(&mut t, "120x40");
    assert_golden("shell-120x40.txt", &screen, "120x40");
    quit(t, "120x40");
}

/// NO_COLOR strips styling but must not change the character grid.
#[test]
fn no_color_preserves_the_character_grid() {
    let mut t = spawn_shell((80, 24), &[("NO_COLOR", "1")], &[]);
    let screen = first_frame(&mut t, "NO_COLOR");
    assert_golden("shell-80x24.txt", &screen, "NO_COLOR");
    quit(t, "NO_COLOR");
}

/// --ascii swaps every non-ASCII glyph the shell draws.
#[test]
fn ascii_mode_renders_without_box_drawing() {
    let mut t = spawn_shell((80, 24), &[], &["--ascii"]);
    let screen = first_frame(&mut t, "--ascii");
    assert_golden("shell-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}
