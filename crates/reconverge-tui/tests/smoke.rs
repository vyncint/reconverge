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

use termlens::{Key, Screen, Style, Terminal};

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

/// How many cells on the frame carry any styling at all.
///
/// Only ever compared against zero or against "some": the exact count is a
/// layout detail, but "the shell painted colour" and "NO_COLOR left none of
/// it" are the two claims the goldens cannot make, because a golden records
/// glyphs and nothing else.
fn styled_cells(frame: &Screen) -> usize {
    (0..frame.rows())
        .flat_map(|row| (0..frame.cols()).map(move |col| (row, col)))
        .filter(|&(row, col)| {
            frame
                .cell(row, col)
                .is_some_and(|cell| *cell.style() != Style::default())
        })
        .count()
}

/// The first complete repaint.
///
/// `q quit` sits in the footer, which the shell draws last, so a frame
/// carrying it carries everything above it too — and `wait_frame` only ever
/// evaluates whole frames, so there is no half-painted screen to catch.
///
/// The synchronized-update precondition is checked by the wait itself, on
/// every frame this file takes: `wait_frame` resolves only for an application that brackets
/// its repaints in DEC 2026 synchronized updates, which is what `sync_draw`
/// in `main.rs` exists to do.
///
/// That resolution IS the assertion — there is deliberately no second one
/// below. `wait_frame` returns only a completed synchronized update, so a
/// frame in hand already has `repaints() >= 1` by construction; and if
/// `sync_draw` were reverted to a bare `terminal.draw`, this call would time
/// out rather than return, so a check after it could never run. termlens
/// names the cause in that timeout itself ("the application never emitted a
/// DEC 2026 synchronized update"), which is the diagnosis an extra assert
/// here would have been trying to add.
fn first_frame(t: &mut Terminal, context: &str) -> Screen {
    t.wait_frame(|screen| screen.contains("q quit"))
        .unwrap_or_else(|e| panic!("{context}: waiting for the first complete frame: {e}"))
}

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("shell did not exit after q");
    assert!(status.success(), "{context}: shell exited with {status:?}");
    // And the terminal is the caller's again. A view that leaves the user in
    // the alternate screen breaks the *next* command, not this test, and no
    // golden can see it: a golden is the alternate screen's contents.
    assert!(
        !t.screen().alternate_screen(),
        "{context}: the shell must restore the terminal on the way out"
    );
}

/// The flakiness gate: 50 consecutive spawn → frame → golden → quit cycles.
#[test]
fn shell_smoke_50_runs_at_80x24() {
    for run in 0..50 {
        let mut t = spawn_shell((80, 24), &[], &[]);
        let frame = first_frame(&mut t, &format!("run {run}"));
        assert_golden("shell-80x24.txt", &frame.to_string(), &format!("run {run}"));
        quit(t, &format!("run {run}"));
    }
}

/// Deterministic layout at the second mandated geometry (docs/ARCHITECTURE.md).
#[test]
fn shell_is_deterministic_at_120x40() {
    let mut t = spawn_shell((120, 40), &[], &[]);
    let frame = first_frame(&mut t, "120x40");
    assert_golden("shell-120x40.txt", &frame.to_string(), "120x40");
    // The colour half of the pair `no_color_preserves_the_character_grid`
    // makes: with colour on, the shell does style cells. Without this the
    // NO_COLOR test's "no styling survives" would also pass against a shell
    // that had stopped colouring anything at all.
    assert!(
        styled_cells(&frame) > 0,
        "the shell paints styled cells when colour is allowed:\n{}",
        frame.with_styles()
    );
    quit(t, "120x40");
}

/// NO_COLOR strips styling but must not change the character grid.
///
/// The grid half is the golden. The styling half had no test at all until
/// now, and it is the half a golden structurally cannot make: a view that
/// hardcoded a colour instead of asking the theme would draw exactly the same
/// glyphs and pass. Reading the styles off the frame is what sees it.
#[test]
fn no_color_preserves_the_character_grid() {
    let mut t = spawn_shell((80, 24), &[("NO_COLOR", "1")], &[]);
    let frame = first_frame(&mut t, "NO_COLOR");
    assert_golden("shell-80x24.txt", &frame.to_string(), "NO_COLOR");
    assert_eq!(
        styled_cells(&frame),
        0,
        "NO_COLOR must leave no styling on the grid, not merely the same \
         glyphs — a cell still styled here is a view that ignored it:\n{}",
        frame.with_styles()
    );
    // And the byte-level counterpart, which is the one difference between
    // this run and the coloured one: with nothing to style, ratatui never
    // writes the `SGR 59` reset that termlens does not model, so this leg's
    // grid is built from a stream the emulator implemented in full. See
    // `tests/emulation.rs` for the coloured leg's pinned list.
    assert!(
        frame.unsupported().is_empty(),
        "with no styling to emit there is nothing left for the emulator to \
         drop, got {:?}",
        frame.unsupported()
    );
    quit(t, "NO_COLOR");
}

/// --ascii swaps every non-ASCII glyph the shell draws.
#[test]
fn ascii_mode_renders_without_box_drawing() {
    let mut t = spawn_shell((80, 24), &[], &["--ascii"]);
    let screen = first_frame(&mut t, "--ascii").to_string();
    assert_golden("shell-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}
