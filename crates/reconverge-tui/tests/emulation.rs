//! What the emulator can and cannot see of the TUI — the assertion every
//! other PTY test in this repository rests on.
//!
//! Every golden under `tests/golden/` is a grid a VT emulator built from the
//! shell's bytes. If the shell emits a sequence termlens does not implement,
//! that grid is quietly wrong and every golden here is blessing a
//! plausible-looking fiction. termlens 0.10 made it checkable:
//! `Screen::unsupported` lists what was dropped.
//!
//! Deliberately whole-suite invariants rather than feature tests. They are
//! cheap — every one reads a `Screen` the spawn already produced — and when
//! one breaks the right response is to distrust the goldens until it is
//! understood.
//!
//! Cost note: `stress.yml` runs this package's suite up to 100 times per
//! dispatch, so every spawn here is paid a hundred times. That is why this
//! file walks four views in one loop and adds five spawns in total, and why
//! the per-leg pins (NO_COLOR, the reason editor's cursor) live in the flow
//! files that already spawn for them rather than in new tests here.
//!
//! Sync policy per AGENTS.md: `wait_frame`, never `wait_idle`, never sleep.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use termlens::{Key, Screen, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The only sequence the TUI emits that termlens does not model.
///
/// `SGR 59` is "underline colour: default", which ratatui writes as part of
/// resetting a style. termlens carries no underline colour, so it records the
/// sequence and moves on — and because the attribute changes no cell, nothing
/// on any grid is wrong as a result. That is the whole reason this list can
/// be pinned exactly: anything joining it is a sequence that *might* change a
/// cell, and would need reading before the goldens are trusted again.
///
/// Not a false positive. termlens#320 reports `^[[5m`/`^[[25m` (blink) and
/// `^[[9m`/`^[[29m` (strikethrough) as unsupported although the attribute
/// shadow does implement them; those cannot appear here, because the TUI's
/// entire style vocabulary is `Modifier::BOLD`, `Modifier::DIM` and five
/// indexed colours — there is no blink and no strikethrough anywhere in
/// `crates/`. If either pair ever shows up in this list, check the modifier
/// that was just added against that issue before believing the failure.
const EXPECTED_UNSUPPORTED: [&str; 1] = ["^[[59m"];

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// An empty working directory, for the views that must run with nothing on
/// disk.
fn empty_dir(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("emulation-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unsupported(screen: &Screen) -> Vec<String> {
    screen
        .unsupported()
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// One spawn per view, each waiting on the text its own flow test waits on.
fn spawn_view(view: &str, size: (u16, u16)) -> Terminal {
    let mut builder = Terminal::builder()
        .size(size.0, size.1)
        .env_clear()
        .timeout(TIMEOUT);
    builder = match view {
        "shell" => builder
            .arg(fixture("findings/rc003-minimal.json"))
            .arg(fixture("unimap/divergent-barrier.json"))
            .arg(fixture("witness/rc001-divergent-barrier.json")),
        "witness" => builder
            .current_dir(fixture("witness"))
            .arg("witness")
            .arg("rc001-divergent-barrier.json")
            .arg("rc002-partial-mask.json"),
        "learn" => builder.current_dir(empty_dir("learn")).arg("learn"),
        "triage" => builder
            .arg("triage")
            .arg("--baseline")
            .arg(fixture("baseline/minimal.json"))
            .arg(fixture("findings/rc003-minimal.json")),
        other => panic!("unknown view {other}"),
    };
    builder
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .unwrap_or_else(|e| panic!("failed to spawn the {view} view in a PTY: {e}"))
}

/// The text the view paints last, so a frame carrying it carries the view.
fn ready(view: &str) -> &'static str {
    match view {
        "shell" => "q quit",
        "witness" => "step 0/3",
        "learn" => "reconverge learn",
        "triage" => "1 suppressed",
        other => panic!("unknown view {other}"),
    }
}

/// The invariant, in every view the TUI has. A dropped sequence in any one of
/// them makes that view's goldens a fiction, and the shell's four views share
/// one renderer, so a regression in one is a regression in all.
#[test]
fn the_emulator_drops_nothing_that_could_change_a_cell() -> termlens::Result<()> {
    for view in ["shell", "witness", "learn", "triage"] {
        let mut t = spawn_view(view, (80, 24));
        let needle = ready(view);
        let screen = t.wait_frame(|s| s.contains(needle))?;

        assert_eq!(
            unsupported(&screen),
            EXPECTED_UNSUPPORTED,
            "{view}: the TUI emitted a sequence termlens does not model. Until \
             it is understood, every golden in this suite is being compared \
             against a grid that may be wrong.\n{screen}"
        );
        assert_eq!(
            screen.unsupported_overflow(),
            0,
            "{view}: the record is complete, not truncated"
        );

        // Insert mode pushes the rest of a row right. A view that left it on
        // would draw a correct-looking panel with every row shifted, and a
        // blessed golden would record the shift as the truth.
        assert!(!screen.insert_mode(), "{view}: the TUI never sets IRM");

        // The plain-text goldens are blind to a bell, and this TUI has real
        // refusal paths ("a reason is required", "write refused") that a
        // future author could reasonably implement with a `\x07`.
        assert_eq!(screen.bells(), 0, "{view}: no audible bell");
        assert_eq!(screen.visual_bells(), 0, "{view}: no visual bell");

        // Nothing wraps: every view lays out to the width it measured, and
        // `learn_flow::masks_verdict_is_whole_at_a_narrow_width` exists
        // because a panel that overflows silently puts its tail on another
        // row. A wrapped row is that failure, before any golden sees it.
        assert!(
            !(0..screen.rows()).any(|row| screen.row_wrapped(row)),
            "{view}: a wrapped row means the layout overflowed:\n{screen}"
        );
        assert_eq!(
            screen.logical_text(),
            screen.text(),
            "{view}: and so nothing needs rejoining"
        );

        // The keyboard-only sync policy in docs/ARCHITECTURE.md assumes no
        // view ever asks the terminal to report the mouse. It is also why
        // termlens 0.10's stricter mouse API cost this repo nothing.
        assert!(
            screen.mouse_modes().is_empty(),
            "{view}: no view enables mouse tracking, got {:?}",
            screen.mouse_modes()
        );

        // Note what is deliberately NOT asserted here: `repaints() >= 1`.
        // Every screen in this loop arrived from `wait_frame`, which returns
        // only a completed DEC 2026 update — so the count is >= 1 by
        // construction, and had `sync_draw` (main.rs) been reverted to a bare
        // `terminal.draw` the wait would have timed out before reaching any
        // assertion. termlens names that cause in the timeout itself. An
        // assert here would read as a guard and be dead code.

        assert!(
            screen.alternate_screen(),
            "{view}: a TUI runs on the alt screen"
        );
        t.send(Key::Char('q'))?;
        let status = t.wait_exit()?;
        assert!(status.success(), "{view}: exited with {status:?}");
        assert!(
            !t.screen().alternate_screen(),
            "{view}: the TUI must put the terminal back on the way out"
        );
    }
    Ok(())
}

/// A frame has to survive being saved and read back, because that is what a
/// golden *is* here: `assert_golden` writes `Screen::to_string()`, and
/// `termlens diff`/`render` read that same text back (tests/cli.rs). If the
/// format ever stopped carrying what the TUI draws — the box drawing, the em
/// dashes, the lane glyphs — every golden would still compare equal to
/// itself while describing something else.
#[test]
fn a_frame_survives_the_snapshot_format_the_goldens_are_written_in() -> termlens::Result<()> {
    let mut t = spawn_view("witness", (80, 24));
    t.wait_until(|s| s.contains("step 0/3"))?;
    t.send(Key::Char('v'))?;
    let screen = t.wait_frame(|s| s.contains("step 3/3") && s.contains("verdict"))?;

    let saved = screen.with_styles().to_string();
    let parsed = Screen::parse(&saved)?;
    assert!(
        screen.diff(&parsed).is_empty(),
        "the saved frame did not read back as the same picture:\n{}",
        screen.diff(&parsed)
    );
    assert_eq!(
        parsed.with_styles().to_string(),
        saved,
        "byte for byte, so a re-blessed golden cannot drift from a saved one"
    );

    // And specifically the lane glyph language, which AGENTS.md calls
    // load-bearing because the ASCII warp diagram in a CI log is literally
    // this row. A round trip that dropped the strip would still pass a
    // whole-screen text comparison if the strip were blank on both sides,
    // so it is asserted present on the way out.
    let strip = "W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.";
    assert_eq!(
        parsed.find_all(strip),
        screen.find_all(strip),
        "the warp diagram survives the format:\n{parsed}"
    );
    assert_eq!(
        screen.find_all(strip).len(),
        1,
        "and appears exactly once — one strip per replay:\n{screen}"
    );

    t.send(Key::Char('q'))?;
    assert!(t.wait_exit()?.success());
    Ok(())
}
