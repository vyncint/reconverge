//! Witness-debugger flow tests: multi-step keyboard journeys through a
//! real PTY on the canonical RC001/RC002 fixtures, plus the size and color
//! matrix — {80×24, 120×40} × {color, NO_COLOR}.
//!
//! Since 0.5.0 those fixtures are **recorded from a real `check`** rather
//! than hand-written (`scripts/record-fixtures.sh`), which is why the step
//! counts here are 3 and not 5: the hand-written documents walked MIR
//! statements the driver has never emitted, so every golden in this file
//! was a frame of a document no user could ever produce.
//!
//! Sync policy: `wait_frame` after every key, and the frame it returns is
//! the one asserted on — never `wait_idle`, never sleep. The shell brackets
//! repaints in DEC 2026 synchronized updates, so a frame is only ever
//! observed whole. Waiting for a 150ms quiet period instead was a guess at
//! how long a repaint takes, and on a loaded macOS runner it was wrong.
//! Regenerate goldens after an intentional UI change with
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test witness_flow`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Screen, Style, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/witness")
}

fn spawn(size: (u16, u16), extra_env: &[(&str, &str)], extra_args: &[&str]) -> Terminal {
    let mut builder = Terminal::builder()
        .size(size.0, size.1)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(fixtures_dir())
        .arg("witness");
    for (key, value) in extra_env {
        builder = builder.env(key, value);
    }
    for arg in extra_args {
        builder = builder.arg(arg);
    }
    builder
        .arg("rc001-divergent-barrier.json")
        .arg("rc002-partial-mask.json")
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .expect("failed to spawn the witness debugger in a PTY")
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
    let status = t.wait_exit().expect("debugger did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
    assert!(
        !t.screen().alternate_screen(),
        "{context}: the debugger must restore the terminal on the way out"
    );
}

/// The §9 journey on the canonical RC001 replay: open → step to the
/// barrier → watch the hang verdict land → scrub back → jump to the
/// divergence → switch to the RC002 witness and read the mask panel →
/// quit, asserting rendered content at every step.
#[test]
fn witness_flow_journey() {
    let mut t = spawn((80, 24), &[], &[]);
    let frame = t
        .wait_frame(|s| {
            s.contains("witness 1/2")
                && s.contains("kernel `rc001_divergent_barrier`")
                && s.contains("0/3")
        })
        .expect("initial frame");
    assert_golden("witness-initial-80x24.txt", &frame.to_string(), "initial");

    // l ×2: to the barrier event — 16 lanes park, the strip becomes the
    // diagnostics' warp diagram.
    for step in 1..=2 {
        t.send(Key::Char('l')).expect("send Key::Char('l')");
        t.wait_until(move |s| s.contains(&format!("step {step}/3")))
            .expect("stepped forward");
    }
    // 16 even lanes park at the barrier while the odd 16 are still running.
    let frame = t
        .wait_frame(|s| {
            s.contains("WoWoWoWo WoWoWoWo WoWoWoWo WoWoWoWo")
                && s.contains("barrier: 16 of 32 threads arrived")
        })
        .expect("the barrier moment");
    assert_golden(
        "witness-barrier-80x24.txt",
        &frame.to_string(),
        "barrier moment",
    );

    // l: the odd lanes exit — the strip becomes the diagnostics' warp
    // diagram — and the verdict lands.
    t.send(Key::Char('l')).expect("send Key::Char('l')");
    t.wait_until(|s| {
        s.contains("step 3/3")
            && s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
            && s.contains("verdict: undefined behavior")
    })
    .expect("verdict reached");

    // h, h: scrub back in time.
    t.send(Key::Char('h')).expect("send Key::Char('h')");
    t.wait_until(|s| s.contains("step 2/3")).expect("back to 2");
    t.send(Key::Char('h')).expect("send Key::Char('h')");
    t.wait_until(|s| s.contains("step 1/3")).expect("back to 1");

    // g then d: launch instant, then straight to the divergence moment.
    t.send(Key::Char('g')).expect("send Key::Char('g')");
    t.wait_until(|s| s.contains("step 0/3")).expect("rewound");
    t.send(Key::Char('d')).expect("send Key::Char('d')");
    t.wait_until(|s| s.contains("step 2/3"))
        .expect("d jumps to the first warp split");

    // n: the RC002 witness, then l ×2 to the collective itself — where the
    // mask panel lives, and where the lane strip has to agree with it.
    //
    // This is the frame the golden existed for and could not catch: the
    // hand-written fixture put its lane deltas on the *branch* step, so it
    // rendered `oWoWoWoW …` and passed, while the artifact a real run wrote
    // carried no deltas at the call at all and showed all 32 lanes active
    // one row above `active 0x55555555`. Recorded from a driver run, the
    // strip reads `o.o.o.o. …` — sixteen lanes, the same sixteen the mask
    // row names.
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    t.wait_until(|s| s.contains("witness 2/2") && s.contains("rc002_divergent_collective"))
        .expect("switched witness");
    for step in 1..=2 {
        t.send(Key::Char('l')).expect("send Key::Char('l')");
        t.wait_until(move |s| s.contains(&format!("step {step}/3")))
            .expect("stepped forward");
    }
    let frame = t
        .wait_frame(|s| {
            s.contains("step 2/3")
                && s.contains("0xffffffff")
                && s.contains("named in the mask but not active: 0xaaaaaaaa")
                && s.contains("o.o.o.o. o.o.o.o. o.o.o.o. o.o.o.o.")
        })
        .expect("mask panel at the collective");
    assert_golden("witness-mask-80x24.txt", &frame.to_string(), "mask panel");

    // The coherence claim the comment above states and `contains` cannot
    // make: "the strip appears somewhere" is true of a strip drawn on the
    // wrong row, against the wrong mask, or twice. What the panel promises is
    // three rows in one order — lanes, mask, active — sharing a left edge, so
    // a reader can compare them column by column, with the strip's sixteen
    // active lanes being the same sixteen `0x55555555` names.
    let lanes = frame.find_all("o.o.o.o. o.o.o.o. o.o.o.o. o.o.o.o.");
    assert_eq!(
        lanes.len(),
        1,
        "one lane strip per replay, not none and not two:\n{frame}"
    );
    let (lanes_row, lanes_col) = lanes[0];
    let active = frame.find_all("#.#.#.#. #.#.#.#. #.#.#.#. #.#.#.#.");
    assert_eq!(active.len(), 1, "one active-mask row:\n{frame}");
    let (active_row, active_col) = active[0];
    assert_eq!(
        active_col, lanes_col,
        "the lane strip and the active mask must start in the same cell, or \
         the two rows cannot be read against each other:\n{frame}"
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

    quit(t, "journey");
}

/// `--ascii` renders the debugger without any non-ASCII glyph.
#[test]
fn witness_ascii_mode_is_pure_ascii() {
    let mut t = spawn((80, 24), &[], &["--ascii"]);
    t.wait_until(|s| s.contains("reconverge witness"))
        .expect("initial frame");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    let frame = t.wait_frame(|s| s.contains("step 3/3")).expect("verdict");
    let screen = frame.to_string();
    assert_golden("witness-verdict-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}

/// One matrix leg: drive to the verdict moment and golden the frame; the
/// NO_COLOR run of the same leg must produce the identical character grid.
fn matrix_leg(size: (u16, u16)) {
    let golden = format!("witness-verdict-{}x{}.txt", size.0, size.1);

    let mut screens = Vec::new();
    for extra_env in [&[][..], &[("NO_COLOR", "1")][..]] {
        let mut t = spawn(size, extra_env, &[]);
        t.wait_until(|s| s.contains("step 0/3"))
            .expect("initial frame");
        t.send(Key::Char('v')).expect("send Key::Char('v')");
        let frame = t
            .wait_frame(|s| {
                s.contains("step 3/3")
                    && s.contains("verdict")
                    && s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
            })
            .expect("verdict moment");
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

    // Two claims the grid comparison above cannot make on its own.
    //
    // The first is that the grids being equal means anything: if the emulator
    // had silently dropped a cursor-moving sequence in the coloured leg, both
    // grids would still match and this leg would stay green over a screen
    // neither run actually drew. `unsupported` names the one sequence that is
    // dropped, and it changes no cell — `SGR 59`, ratatui's underline-colour
    // reset (tests/emulation.rs explains why this list can be pinned exactly,
    // and why termlens#320's false positives cannot appear here).
    //
    // The second is that NO_COLOR did anything at all. Identical grids are
    // also what a view that hardcoded a colour produces.
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
        "the debugger styles cells when colour is allowed ({golden})"
    );
    assert_eq!(
        styled_cells(plain),
        0,
        "NO_COLOR must leave no styling on the grid, not merely the same \
         glyphs ({golden}):\n{}",
        plain.with_styles()
    );
}

#[test]
fn matrix_80x24() {
    matrix_leg((80, 24));
}

#[test]
fn matrix_120x40() {
    matrix_leg((120, 40));
}
