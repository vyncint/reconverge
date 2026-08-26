//! Triage flow tests: keyboard journeys through a real PTY, plus the size
//! and color matrix — {80×24, 120×40} × {color, NO_COLOR}.
//!
//! One journey writes for real, into a temp baseline: this is the only
//! view that touches the filesystem, so the test follows the bytes all the
//! way to disk and back through the schema binding.
//!
//! Sync policy: `wait_frame` after every key, and the frame it returns is
//! the one asserted on — never `wait_idle`, never sleep. The shell brackets
//! repaints in DEC 2026 synchronized updates, so a frame is only ever
//! observed whole. Waiting for a 150ms quiet period instead was a guess at
//! how long a repaint takes, and on a loaded macOS runner it was wrong.
//! Regenerate goldens after an intentional UI change with
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test triage_flow`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use reconverge_artifacts::baseline::BaselineArtifact;
use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// A scratch directory for a test that writes.
fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("triage-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn(size: (u16, u16), extra_env: &[(&str, &str)], args: &[&str], baseline: &Path) -> Terminal {
    let mut builder = Terminal::builder()
        .size(size.0, size.1)
        .env_clear()
        .timeout(TIMEOUT)
        .arg("triage")
        .arg("--baseline")
        .arg(baseline.to_str().unwrap());
    for (key, value) in extra_env {
        builder = builder.env(key, value);
    }
    for arg in args {
        builder = builder.arg(arg);
    }
    builder
        .arg(fixture("findings/rc003-minimal.json").to_str().unwrap())
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .expect("failed to spawn triage in a PTY")
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

fn type_text(t: &mut Terminal, text: &str) {
    for c in text.chars() {
        t.send(Key::Char(c))
            .unwrap_or_else(|e| panic!("could not type {c:?}: {e}"));
    }
}

fn quit(mut t: Terminal, context: &str) {
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    let status = t.wait_exit().expect("triage did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
}

/// The §9 journey: open on an empty baseline → accept a finding with a
/// typed reason → write it → verify the bytes on disk → withdraw the
/// acceptance → quit with unsaved edits and get asked about it.
#[test]
fn triage_flow_journey() {
    let dir = scratch("journey");
    let baseline = dir.join("reconverge-baseline.json");
    let mut t = spawn((80, 24), &[], &[], &baseline);

    let frame = t
        .wait_frame(|s| s.contains("reconverge triage") && s.contains("2 findings — 0 suppressed"))
        .expect("initial frame");
    assert_golden("triage-initial-80x24.txt", &frame.to_string(), "initial");

    // s: accept the selected finding; the editor asks for a reason and
    // refuses to record an empty one.
    t.send(Key::Char('s')).expect("send Key::Char('s')");
    t.wait_until(|s| s.contains("why is this acceptable?"))
        .expect("reason editor");
    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("a reason is required"))
        .expect("empty reasons are refused");

    // Non-ASCII text in a reason: the editor is a unicode surface, and the
    // grapheme-aware backspace depends on it.
    type_text(&mut t, "reviewed \u{2014} host owns it");
    t.wait_until(|s| s.contains("reviewed \u{2014} host owns it"))
        .expect("reason echoes as typed");
    t.send(Key::Enter).expect("send Key::Enter");
    let frame = t
        .wait_frame(|s| s.contains("2 findings — 1 suppressed") && s.contains("(unsaved)"))
        .expect("acceptance recorded, not yet written");
    assert_golden("triage-accepted-80x24.txt", &frame.to_string(), "accepted");

    // w: write, and check the bytes that landed.
    t.send(Key::Char('w')).expect("send Key::Char('w')");
    t.wait_until(|s| s.contains("baseline written — 1 entry(ies)") && !s.contains("(unsaved)"))
        .expect("baseline written");
    let written: BaselineArtifact =
        serde_json::from_str(&fs::read_to_string(&baseline).expect("baseline file exists"))
            .expect("the written file is a baseline.v1 document");
    assert_eq!(written.entries.len(), 1);
    assert_eq!(written.entries[0].krate, "sample_kernels");
    assert_eq!(written.entries[0].code, "RC003");
    assert_eq!(written.entries[0].kernel.as_deref(), Some("bad_mut_slice"));
    assert_eq!(written.entries[0].reason, "reviewed \u{2014} host owns it");

    // u: withdraw the acceptance — an edit again, so q asks before losing it.
    t.send(Key::Char('u')).expect("send Key::Char('u')");
    t.wait_until(|s| s.contains("2 findings — 0 suppressed") && s.contains("(unsaved)"))
        .expect("acceptance withdrawn");
    t.send(Key::Char('q')).expect("send Key::Char('q')");
    t.wait_until(|s| s.contains("unsaved edits — press w to write"))
        .expect("quitting asks once");
    // The file on disk is untouched by the withdrawal we never wrote.
    assert_eq!(
        serde_json::from_str::<BaselineArtifact>(&fs::read_to_string(&baseline).unwrap())
            .unwrap()
            .entries
            .len(),
        1
    );

    quit(t, "journey");
    fs::remove_dir_all(&dir).ok();
}

/// `Q` discards unsaved edits deliberately, and never writes.
#[test]
fn force_quit_discards_without_writing() {
    let dir = scratch("force-quit");
    let baseline = dir.join("reconverge-baseline.json");
    let mut t = spawn((80, 24), &[], &[], &baseline);
    t.wait_until(|s| s.contains("reconverge triage"))
        .expect("initial frame");

    t.send(Key::Char('s')).expect("send Key::Char('s')");
    t.wait_until(|s| s.contains("why is this acceptable?"))
        .expect("reason editor");
    type_text(&mut t, "typed but discarded");
    t.send(Key::Enter).expect("send Key::Enter");
    t.wait_until(|s| s.contains("(unsaved)")).expect("edited");

    t.send(Key::Char('Q')).expect("send Key::Char('Q')");
    let status = t.wait_exit().expect("Q did not exit");
    assert!(status.success());
    assert!(!baseline.exists(), "Q must never write the baseline");
    fs::remove_dir_all(&dir).ok();
}

/// `--ascii` renders triage without any non-ASCII glyph.
#[test]
fn triage_ascii_mode_is_pure_ascii() {
    let mut t = spawn(
        (80, 24),
        &[],
        &["--ascii"],
        &fixture("baseline/minimal.json"),
    );
    let frame = t
        .wait_frame(|s| s.contains("reconverge triage") && s.contains("1 suppressed"))
        .expect("initial frame");
    let screen = frame.to_string();
    assert_golden("triage-reviewed-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}

/// One matrix leg over the checked-in reviewed baseline (read-only: no key
/// in this journey writes). The NO_COLOR run must be grid-identical.
fn matrix_leg(size: (u16, u16)) {
    let golden = format!("triage-reviewed-{}x{}.txt", size.0, size.1);

    let mut screens = Vec::new();
    for extra_env in [&[][..], &[("NO_COLOR", "1")][..]] {
        let mut t = spawn(size, extra_env, &[], &fixture("baseline/minimal.json"));
        t.wait_until(|s| s.contains("1 suppressed"))
            .expect("initial frame");
        // The accepted finding shows its recorded reason.
        let frame = t
            .wait_frame(|s| s.contains("reason: reviewed:"))
            .expect("reason shown for the accepted finding");
        screens.push(normalize(&frame.to_string()));
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
