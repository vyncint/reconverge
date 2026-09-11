//! Learn-mode flow tests: keyboard journeys through a real PTY, plus the
//! size and color matrix — {80×24, 120×40} × {color, NO_COLOR}. Learn mode
//! must run with no network and no
//! analysis step, so every spawn here uses a scrubbed environment and an
//! EMPTY working directory: everything on screen is embedded in the
//! binary.
//!
//! Sync policy: `wait_frame` after every key, and the frame it returns is
//! the one asserted on — never `wait_idle`, never sleep. The shell brackets
//! repaints in DEC 2026 synchronized updates, so a frame is only ever
//! observed whole. Waiting for a 150ms quiet period instead was a guess at
//! how long a repaint takes, and on a loaded macOS runner it was wrong.
//! Regenerate goldens after an intentional UI change with
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test learn_flow`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Screen, Style, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);

/// An empty working directory: the "runs with nothing on disk" claim made
/// literal. The caller names it, because these tests run in parallel and
/// each one wipes its own directory on the way in.
fn empty_dir(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("learn-cwd-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn(size: (u16, u16), extra_env: &[(&str, &str)], extra_args: &[&str], tag: &str) -> Terminal {
    let mut builder = Terminal::builder()
        .size(size.0, size.1)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(empty_dir(tag))
        .arg("learn");
    for (key, value) in extra_env {
        builder = builder.env(key, value);
    }
    for arg in extra_args {
        builder = builder.arg(arg);
    }
    builder
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .expect("failed to spawn learn mode in a PTY")
}

fn normalize(frame: &str) -> String {
    frame
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_golden(name: &str, screen: &str, context: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
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

/// How many cells carry any styling. Compared against zero or "some" only:
/// see the NO_COLOR leg below.
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

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("learn mode did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
    assert!(
        !t.screen().alternate_screen(),
        "{context}: learn mode must restore the terminal on the way out"
    );
}

/// The §9 journey: list → the barriers lesson → step the embedded replay
/// to the hang → back through pages → Esc to the list → the reconvergence
/// lesson → watch the fix complete → quit.
#[test]
fn learn_flow_journey() {
    let mut t = spawn((80, 24), &[], &[], "journey");
    let frame = t
        .wait_frame(|s| s.contains("reconverge learn") && s.contains("1. divergence"))
        .expect("lesson list");
    assert_golden("learn-list-80x24.txt", &frame.to_string(), "list");

    // j, Enter: open the barriers lesson.
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.wait_until(|s| s.contains("> 2. barriers"))
        .expect("selected");
    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("lesson 2/4") && s.contains("page 1/3"))
        .expect("lesson opened");

    // n: the interactive page; v: the hang verdict on the embedded replay.
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    // 3 steps, not 5: the embedded replay is the recorded fixture, which is
    // what a user's own `check` writes. The 5-step version walked MIR the
    // driver has never emitted, so the lesson taught a frame nobody could
    // reproduce from their own kernel.
    t.wait_until(|s| s.contains("page 2/3") && s.contains("step 0/3"))
        .expect("interactive page");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    let frame = t
        .wait_frame(|s| {
            s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
                && s.contains("verdict: undefined behavior")
        })
        .expect("the hang lands inside the lesson");
    assert_golden("learn-barriers-hang-80x24.txt", &frame.to_string(), "hang");

    // n, then Esc: last page, back to the list.
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    t.wait_until(|s| s.contains("page 3/3")).expect("last page");
    t.send(Key::Esc).expect("send Key::Esc");
    t.wait_until(|s| s.contains("1. divergence"))
        .expect("back to list");

    // The reconvergence lesson: the fixed kernel completes.
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.wait_until(|s| s.contains("> 4. reconvergence"))
        .expect("selected");
    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("lesson 4/4")).expect("opened");
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    // Still 5: `reconverged-clean.json` is the one fixture no run can
    // record, because a witness is only written for a *confirmed* finding
    // and this kernel has none. fixtures/README.md says so.
    t.wait_until(|s| s.contains("page 2/3") && s.contains("step 0/5"))
        .expect("interactive page");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    let frame = t
        .wait_frame(|s| s.contains("verdict: completed") && s.contains("cannot hang"))
        .expect("the fix completes");
    assert_golden(
        "learn-reconverged-80x24.txt",
        &frame.to_string(),
        "completed",
    );

    quit(t, "journey");
}

/// `--ascii` renders learn mode without any non-ASCII glyph.
#[test]
fn learn_ascii_mode_is_pure_ascii() {
    let mut t = spawn((80, 24), &[], &["--ascii"], "ascii");
    t.wait_until(|s| s.contains("reconverge learn"))
        .expect("list");
    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("lesson 1/4")).expect("opened");
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    let frame = t
        .wait_frame(|s| s.contains("page 2/3"))
        .expect("interactive page");
    let screen = frame.to_string();
    assert_golden("learn-divergence-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}

/// One matrix leg: the masks lesson at its collective moment; the NO_COLOR
/// run of the same leg must produce the identical character grid.
///
/// "Collective moment" is now the step carrying the `warp_op`, not the last
/// step: the recorded fixture has the driver's trailing narration step,
/// which the hand-written one did not.
fn matrix_leg(size: (u16, u16)) {
    let golden = format!("learn-masks-{}x{}.txt", size.0, size.1);

    let mut screens = Vec::new();
    for (color, extra_env) in [("color", &[][..]), ("nocolor", &[("NO_COLOR", "1")][..])] {
        let tag = format!("matrix-{}x{}-{color}", size.0, size.1);
        let mut t = spawn(size, extra_env, &[], &tag);
        t.wait_until(|s| s.contains("reconverge learn"))
            .expect("list");
        t.send(Key::Char('j')).expect("send Key::Char('j')");
        t.send(Key::Char('j')).expect("send Key::Char('j')");
        t.wait_until(|s| s.contains("> 3."))
            .expect("masks selected");
        t.send(Key::Enter).expect("send Key::Enter");
        t.send(Key::Char('n')).expect("send Key::Char('n')");
        t.wait_until(|s| s.contains("page 2/3"))
            .expect("interactive page");
        // l ×2 to the collective. `v` used to land here because the
        // hand-written fixture made the call the last step; the recorded
        // one has the trailing narration the driver writes, so the mask
        // panel is one step before the verdict.
        for step in 1..=2 {
            t.send(Key::Char('l')).expect("send Key::Char('l')");
            t.wait_until(move |s| s.contains(&format!("step {step}/3")))
                .expect("stepped forward");
        }
        let frame = t
            .wait_frame(|s| {
                s.contains("######## ######## ######## ########")
                    && s.contains("#.#.#.#.")
                    // The strip agrees with the active row below it, which
                    // is the whole point of this panel and the one thing
                    // the previous golden could not see: the hand-written
                    // fixture put its deltas on the branch step, so it
                    // rendered coherently while the shipping artifact did
                    // not. The verdict is one step on and is goldened by
                    // the narrow-width case and by witness_flow.
                    && s.contains("o.o.o.o. o.o.o.o. o.o.o.o. o.o.o.o.")
            })
            .expect("the lane strip and the active mask agree at the collective");

        // "Agree" made structural, rather than "both appear somewhere". The
        // three rows are one picture: lanes, then the mask the call named,
        // then the lanes that actually arrived — adjacent, in that order, and
        // starting in the same cell so a reader compares them column by
        // column. A strip drawn against the wrong mask row satisfies
        // `contains` and nothing here.
        let lanes = frame.find_all("o.o.o.o. o.o.o.o. o.o.o.o. o.o.o.o.");
        assert_eq!(lanes.len(), 1, "one lane strip per replay:\n{frame}");
        let (lanes_row, lanes_col) = lanes[0];
        let active = frame.find_all("#.#.#.#. #.#.#.#. #.#.#.#. #.#.#.#.");
        assert_eq!(active.len(), 1, "one active-mask row:\n{frame}");
        let (active_row, active_col) = active[0];
        assert_eq!(
            active_col, lanes_col,
            "the lane strip and the active mask share a left edge:\n{frame}"
        );
        assert_eq!(
            active_row,
            lanes_row + 2,
            "lanes, then mask, then active — adjacent and in that order:\n{frame}"
        );
        assert!(
            frame.row_text(lanes_row + 1).contains("0xffffffff"),
            "the mask row carries the mask the call named:\n{frame}"
        );
        assert!(
            frame.row_text(active_row).contains("0x55555555"),
            "and the active row the lanes that arrived:\n{frame}"
        );

        screens.push(frame);
        quit(t, "matrix leg");
    }
    let (colour, plain) = (&screens[0], &screens[1]);

    assert_golden(&golden, &colour.to_string(), "matrix color leg");
    assert_eq!(
        normalize(&colour.to_string()),
        normalize(&plain.to_string()),
        "NO_COLOR must not change the character grid ({golden})"
    );

    // Why the equality above means anything, and what NO_COLOR actually did.
    // Identical grids would also be produced by an emulator that dropped a
    // cursor-moving sequence in both runs, and by a view that hardcoded a
    // colour. `unsupported` rules out the first — the one sequence dropped is
    // `SGR 59`, ratatui's underline-colour reset, which changes no cell — and
    // the styled-cell counts rule out the second. tests/emulation.rs explains
    // why the coloured list can be pinned exactly here.
    assert_eq!(
        colour.unsupported(),
        ["^[[59m"],
        "the coloured leg's grid was built from a stream the emulator \
         implemented apart from the underline-colour reset ({golden})"
    );
    assert!(
        plain.unsupported().is_empty(),
        "with nothing to style there is nothing left to drop, got {:?}",
        plain.unsupported()
    );
    assert!(
        styled_cells(colour) > 0,
        "learn mode styles cells when colour is allowed ({golden})"
    );
    assert_eq!(
        styled_cells(plain),
        0,
        "NO_COLOR must leave no styling on the grid, not merely the same \
         glyphs ({golden}):\n{}",
        plain.with_styles()
    );
}

/// Guard against the truncation returning: the verdict is the last content
/// row of every replay golden and must be wrapped in full, never cut off
/// with the fit ellipsis. Checked over the checked-in goldens so it holds
/// without spawning a PTY.
#[test]
fn no_learn_golden_truncates_its_final_line() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("learn-") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        // Inner content rows carry the side border (`│`, or `|` in ASCII
        // mode); the last one is the final content line.
        let final_line = text
            .lines()
            .rfind(|l| l.starts_with('│') || l.starts_with('|'))
            .unwrap_or_else(|| panic!("{name}: no content rows"));
        let content = final_line.trim_end_matches(['│', '|']).trim_end();
        assert!(
            !content.ends_with('…') && !content.ends_with("..."),
            "{name}: final content line is truncated: {final_line:?}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no learn goldens found to check");
}

/// Regression (#69 review): at inner widths 41–44 the multi-span lanes strip
/// used to wrap and shove the verdict's tail off the panel — with no ellipsis
/// to show anything was missing, worse than the truncation this replaced.
/// Drive the masks verdict at 46×24, inside that band, and require it in
/// full; the 80/120 goldens all sit outside the band and cannot catch it.
#[test]
fn masks_verdict_is_whole_at_a_narrow_width() {
    let mut t = spawn((46, 24), &[], &[], "narrow-verdict");
    t.wait_until(|s| s.contains("reconverge learn"))
        .expect("list");
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.wait_until(|s| s.contains("> 3."))
        .expect("masks selected");
    t.send(Key::Enter).expect("send Key::Enter");
    // At 46 columns the "page N/M" header itself truncates, so key off the
    // replay step line instead — it renders in full at this width.
    t.wait_until(|s| s.contains("lesson 3/4"))
        .expect("masks opened");
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    t.wait_until(|s| s.contains("step 0/3"))
        .expect("interactive page");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    // Its final word fell off the panel before the fix — `finishes` here,
    // the last word of the verdict the driver actually writes.
    let frame = t
        .wait_frame(|s| s.contains("verdict") && s.contains("finishes"))
        .expect("the masks verdict must appear in full at 46 columns");
    assert_golden(
        "learn-masks-46x24.txt",
        &frame.to_string(),
        "narrow verdict",
    );
    quit(t, "narrow verdict");
}

#[test]
fn matrix_80x24() {
    matrix_leg((80, 24));
}

#[test]
fn matrix_120x40() {
    matrix_leg((120, 40));
}
