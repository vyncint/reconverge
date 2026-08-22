//! Inspector flow tests (§9 layer 3): multi-step keyboard journeys driven
//! through a real PTY on the checked-in `fixtures/inspect` scenario.
//! Sync policy: content-based `wait_until` after every key — never sleep.
//!
//! Regenerate goldens after an intentional UI change with
//! `RECONVERGE_BLESS=1 cargo test -p reconverge-tui --test inspect_flow`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(150);

fn scenario_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/inspect")
}

fn spawn(extra_args: &[&str]) -> Terminal {
    let mut builder = Terminal::builder()
        .size(80, 24)
        .env_clear()
        .timeout(TIMEOUT)
        // Artifact spans are relative to the scenario dir, so the source
        // pane resolves `src/lib.rs` from there.
        .current_dir(scenario_dir())
        .arg("inspect");
    for arg in extra_args {
        builder = builder.arg(arg);
    }
    builder
        .arg("unimap.json")
        .arg("findings.json")
        .spawn(env!("CARGO_BIN_EXE_reconverge-tui"))
        .expect("failed to spawn the inspector in a PTY")
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
    let status = t.wait_exit().expect("inspector did not exit after q");
    assert!(status.success(), "{context}: exited with {status:?}");
}

/// The §9 journey: open → step values → walk provenance → back → jump to
/// the RC001 finding → quit, asserting on rendered content at every step.
#[test]
fn inspector_flow_journey() {
    let mut t = spawn(&[]);
    t.wait_until(|s| s.contains("reconverge inspect") && s.contains("kernel divergent_barrier"))
        .expect("initial frame");
    t.wait_idle(QUIET).expect("initial paint settles");
    assert_golden(
        "inspect-initial-80x24.txt",
        &t.screen().to_string(),
        "initial",
    );

    // j: select `i`, the thread-index witness.
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.wait_until(|s| s.contains("provenance of `i`"))
        .expect("selection moved to `i`");

    // j: select `e`.
    t.send(Key::Char('j')).expect("send Key::Char('j')");
    t.wait_until(|s| s.contains("provenance of `e`"))
        .expect("selection moved to `e`");

    // p: one hop toward the source.
    t.send(Key::Char('p')).expect("send Key::Char('p')");
    t.wait_until(|s| s.contains("provenance of _11"))
        .expect("walked one hop to _11");

    // u: back to `e`.
    t.send(Key::Char('u')).expect("send Key::Char('u')");
    t.wait_until(|s| s.contains("provenance of `e`"))
        .expect("walked back to `e`");

    // n ×3: land on the RC001 finding; selection jumps to the branch
    // condition and the source pane focuses its line.
    for _ in 0..3 {
        t.send(Key::Char('n')).expect("send Key::Char('n')");
    }
    t.wait_until(|s| s.contains("finding 3/3 [RC001]"))
        .expect("RC001 finding selected");
    t.wait_idle(QUIET).expect("jump paint settles");
    assert_golden(
        "inspect-rc001-80x24.txt",
        &t.screen().to_string(),
        "rc001 jump",
    );

    quit(t, "journey");
}

/// `--ascii` renders the inspector without any non-ASCII glyph.
#[test]
fn inspector_ascii_mode_is_pure_ascii() {
    let mut t = spawn(&["--ascii"]);
    t.wait_until(|s| s.contains("reconverge inspect"))
        .expect("initial frame");
    t.wait_idle(QUIET).expect("paint settles");
    let screen = t.screen().to_string();
    assert_golden("inspect-initial-80x24-ascii.txt", &screen, "--ascii");
    for line in normalize(&screen).lines() {
        assert!(line.is_ascii(), "non-ASCII glyph in --ascii mode: {line:?}");
    }
    quit(t, "--ascii");
}
