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

use termlens::{Key, Terminal};

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

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("learn mode did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
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
    t.wait_until(|s| s.contains("page 2/3") && s.contains("step 0/5"))
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
        // v: in this witness the collective IS the verdict step, so one
        // jump shows the mask against the active lanes and the verdict.
        t.send(Key::Char('v')).expect("send Key::Char('v')");
        let frame = t
            .wait_frame(|s| {
                s.contains("verdict")
                    && s.contains("######## ######## ######## ########")
                    && s.contains("#.#.#.#.")
            })
            .expect("mask, active lanes, and verdict together");
        screens.push(normalize(&frame.to_string()));
        quit(t, "matrix leg");
    }

    assert_golden(&golden, &screens[0], "matrix color leg");
    assert_eq!(
        screens[0], screens[1],
        "NO_COLOR must not change the character grid ({golden})"
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
    // Its final word fell off the panel before the fix.
    let frame = t
        .wait_frame(|s| s.contains("verdict") && s.contains("completes"))
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
