//! Witness-debugger flow tests: multi-step keyboard journeys through a
//! real PTY on the canonical RC001/RC002 fixtures, plus the size and color
//! matrix — {80×24, 120×40} × {color, NO_COLOR}.
//!
//! Sync policy: content-based `wait_until` after every key — never sleep.
//! Regenerate goldens after an intentional UI change with
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test witness_flow`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(150);

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

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("debugger did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
}

/// The §9 journey on the canonical RC001 replay: open → step to the
/// barrier → watch the hang verdict land → scrub back → jump to the
/// divergence → switch to the RC002 witness and read the mask panel →
/// quit, asserting rendered content at every step.
#[test]
fn witness_flow_journey() {
    let mut t = spawn((80, 24), &[], &[]);
    t.wait_until(|s| {
        s.contains("witness 1/2") && s.contains("kernel `divergent_barrier`") && s.contains("0/5")
    })
    .expect("initial frame");
    t.wait_idle(QUIET).expect("initial paint settles");
    assert_golden(
        "witness-initial-80x24.txt",
        &t.screen().to_string(),
        "initial",
    );

    // l ×4: to the barrier event — 16 lanes park, the strip becomes the
    // diagnostics' warp diagram.
    for step in 1..=4 {
        t.send(Key::Char('l')).expect("send Key::Char('l')");
        t.wait_until(move |s| s.contains(&format!("step {step}/5")))
            .expect("stepped forward");
    }
    // 16 even lanes park at the barrier while the odd 16 are still running.
    t.wait_until(|s| {
        s.contains("WoWoWoWo WoWoWoWo WoWoWoWo WoWoWoWo")
            && s.contains("barrier: 16 of 32 threads arrived")
    })
    .expect("the barrier moment");
    t.wait_idle(QUIET).expect("barrier paint settles");
    assert_golden(
        "witness-barrier-80x24.txt",
        &t.screen().to_string(),
        "barrier moment",
    );

    // l: the odd lanes exit — the strip becomes the diagnostics' warp
    // diagram — and the verdict lands.
    t.send(Key::Char('l')).expect("send Key::Char('l')");
    t.wait_until(|s| {
        s.contains("step 5/5")
            && s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
            && s.contains("verdict: undefined behavior")
    })
    .expect("verdict reached");

    // h, h: scrub back in time.
    t.send(Key::Char('h')).expect("send Key::Char('h')");
    t.wait_until(|s| s.contains("step 4/5")).expect("back to 4");
    t.send(Key::Char('h')).expect("send Key::Char('h')");
    t.wait_until(|s| s.contains("step 3/5")).expect("back to 3");

    // g then d: launch instant, then straight to the divergence moment.
    t.send(Key::Char('g')).expect("send Key::Char('g')");
    t.wait_until(|s| s.contains("step 0/5")).expect("rewound");
    t.send(Key::Char('d')).expect("send Key::Char('d')");
    t.wait_until(|s| s.contains("step 4/5"))
        .expect("d jumps to the first warp split");

    // n: the RC002 witness; v: its verdict, where the mask panel shows the
    // named-but-inactive lanes.
    t.send(Key::Char('n')).expect("send Key::Char('n')");
    t.wait_until(|s| s.contains("witness 2/2") && s.contains("rc002_divergent_collective"))
        .expect("switched witness");
    t.send(Key::Char('v')).expect("send Key::Char('v')");
    t.wait_until(|s| {
        s.contains("step 3/3")
            && s.contains("0xffffffff")
            && s.contains("named in the mask but not active: 0xaaaaaaaa")
    })
    .expect("mask panel at the collective");
    t.wait_idle(QUIET).expect("mask paint settles");
    assert_golden(
        "witness-mask-80x24.txt",
        &t.screen().to_string(),
        "mask panel",
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
    t.wait_until(|s| s.contains("step 5/5")).expect("verdict");
    t.wait_idle(QUIET).expect("paint settles");
    let screen = t.screen().to_string();
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
        t.wait_until(|s| s.contains("step 0/5"))
            .expect("initial frame");
        t.send(Key::Char('v')).expect("send Key::Char('v')");
        t.wait_until(|s| {
            s.contains("step 5/5")
                && s.contains("verdict")
                && s.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.")
        })
        .expect("verdict moment");
        t.wait_idle(QUIET).expect("paint settles");
        screens.push(normalize(&t.screen().to_string()));
        quit(t, "matrix leg");
    }

    assert_golden(&golden, &screens[0], "matrix color leg");
    assert_eq!(
        screens[0], screens[1],
        "NO_COLOR must not change the character grid ({golden})"
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
