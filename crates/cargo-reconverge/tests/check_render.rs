//! What `check` actually puts on a terminal.
//!
//! Every other end-to-end test here reads `check`'s stdout as a string, so
//! it can only ask whether a substring is present. That cannot see the two
//! things this file exists for: an escape byte copied out of the analyzed
//! source **removing** diagnostics that were already printed, and a caret
//! landing in the wrong terminal cell. Both are properties of the rendered
//! grid, so both need a real PTY.
//!
//! `termlens` was already a dev-dependency of this crate and was pointed
//! exclusively at the TUI subcommands. This is the same harness aimed at the
//! CLI's own output.
//!
//! Sync policy: content-based waits only, never a sleep. The analysis runs
//! once through `Command` before the PTY run, so the terminal only ever sees
//! a warm, fast re-check.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{env, fs};

use termlens::Screen;

/// Generous: the PTY run is warm, but CI runners are not fast.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Tall enough to hold the whole report, which is the point: the erasure
/// under test wipes the *visible grid*, so nothing may scroll off on its own
/// or the assertion could not tell the two apart.
const COLS: u16 = 160;
const ROWS: u16 = 90;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
            // Backdate the copy: a source written in the same mtime tick as
            // the first build's fingerprint reads as dirty on the next run,
            // and these suites assert on cargo's freshness (a warm re-check,
            // an edit between two runs). CI hit exactly that — a fresh copy,
            // an out-of-band warm run, then an unexpected `Checking` line.
            let file = fs::File::options().write(true).open(&target).unwrap();
            let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
            file.set_modified(past).unwrap();
        }
    }
}

fn ensure_driver() -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    let driver = cli
        .parent()
        .unwrap()
        .join(format!("reconverge-driver{}", std::env::consts::EXE_SUFFIX));
    if !driver.is_file() {
        let status = Command::new(env::var("CARGO").unwrap())
            .args(["build", "-p", "reconverge-driver"])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .expect("failed to spawn cargo build for the driver");
        assert!(status.success(), "building reconverge-driver failed");
    }
    driver
}

/// The toolchain's own variables, and only those that are actually set:
/// an absent `CARGO_HOME` must stay absent, not become the empty string,
/// or cargo resolves a different home in one run than in the other.
fn toolchain_env() -> Vec<(String, String)> {
    ["PATH", "HOME", "CARGO", "CARGO_HOME", "RUSTUP_HOME"]
        .iter()
        .filter_map(|name| env::var(name).ok().map(|value| (name.to_string(), value)))
        .collect()
}

/// A warm copy of the render probe: analyzed once out of band, so the run
/// the terminal sees is a cached re-check rather than a dependency build.
fn warm_probe(driver: &Path) -> PathBuf {
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("r1-render-probe");
    let _ = fs::remove_dir_all(project.join("src"));
    let _ = fs::remove_file(project.join("Cargo.toml"));
    let _ = fs::remove_file(project.join("Cargo.lock"));
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/render-probe"),
        &project,
    );
    // The same environment the PTY run gets, so cargo's fingerprint agrees
    // between the two and the run the terminal sees is a cached re-check.
    // With the warm run inheriting the whole process environment and the
    // PTY run getting five variables, CI saw a `Checking` line on the second
    // run — one row, which was the difference between a report that fits
    // 160x90 and one that scrolls.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check"])
        .current_dir(&project)
        .env_clear()
        .envs(toolchain_env())
        .env("RECONVERGE_DRIVER", driver)
        .output()
        .expect("failed to spawn cargo-reconverge check");
    assert_eq!(
        output.status.code(),
        Some(1),
        "the probe's divergent barriers exit 1\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    project
}

/// The first column of `needle` on `row`, counted in **terminal cells** —
/// which is the whole question here, since a wide character occupies two and
/// a byte offset into the row's text occupies one.
fn col_of(screen: &Screen, row: u16, needle: &str) -> Option<u16> {
    let needle: Vec<char> = needle.chars().collect();
    (0..screen.cols()).find(|&start| {
        let mut col = start;
        for want in &needle {
            match screen.cell(row, col) {
                Some(cell) if cell.contents().starts_with(*want) => {
                    col += if cell.is_wide() { 2 } else { 1 };
                }
                _ => return false,
            }
        }
        true
    })
}

/// The `(snippet row, caret row)` of the diagnostic for `kernel`.
fn block(screen: &Screen, kernel: &str) -> (u16, u16) {
    let (header, _) = screen
        .find(&format!("kernel `{kernel}`"))
        .unwrap_or_else(|| panic!("no diagnostic for `{kernel}` on screen:\n{screen}"));
    for row in header..screen.rows() {
        if screen.row_text(row).contains("^^^") {
            return (row - 1, row);
        }
    }
    panic!("no caret row under the `{kernel}` header:\n{screen}");
}

/// Everything the rendered report has to get right, on one screen.
///
/// One PTY run rather than five: the report is a single frame, the erasure
/// case is about what survives *alongside* the other diagnostics, and a
/// second `check` in another terminal would be the same frame again.
#[test]
fn the_rendered_report_survives_its_own_source_and_points_where_it_says() -> termlens::Result<()> {
    let driver = ensure_driver();
    let project = warm_probe(&driver);

    // `bin!` pins the binary at compile time and clears the environment;
    // `check` shells out to cargo, so the toolchain's own variables are put
    // back explicitly — under `env_clear` there is no PATH at all. The warm
    // run above used exactly this set, so the two runs agree.
    let driver = driver.to_str().expect("utf-8 driver path").to_string();

    let mut t = termlens::bin!(
        "cargo-reconverge",
        size(COLS, ROWS),
        timeout(TIMEOUT),
        current_dir(&project),
        envs(toolchain_env()),
        env("RECONVERGE_DRIVER", &driver),
        args(["reconverge", "check"]),
    )?;

    // The summary is the last thing the run paints, so waiting on it means
    // the whole report is on the grid — and then settling, because a
    // predicate can fire on a half-painted row.
    let screen = t.snapshot_after(|s| s.contains("reconverge: 0 deny,"))?;
    let status = t.wait_exit()?;
    assert_eq!(status.code(), Some(1), "gating findings exit 1: {status}");

    // --- the erasure. An `ESC [ 2 J` copied out of the analyzed source used
    // to clear the display, taking every diagnostic printed before it. The
    // summary would still have counted them.
    assert!(
        screen.contains("kernel `aaa_first`"),
        "a diagnostic printed before the escaped one was erased from the \
         screen:\n{screen}"
    );
    assert!(screen.contains("kernel `zzz_escaped`"), "{screen}");
    // The escape is on screen as text, in the snippet, rather than acting.
    assert!(
        screen.contains("\u{241b}[2J\u{241b}[H"),
        "the escape must render as visible text:\n{screen}"
    );
    // Nothing anywhere on the grid is a control character: the emulator
    // would have consumed a real one, so this is belt and braces on the
    // substitution rather than on the terminal.
    assert!(
        !screen.text().chars().any(|c| c.is_control() && c != '\n'),
        "a control character reached the grid:\n{screen:?}"
    );
    // Every kernel's diagnostic is still there — five headers, five carets.
    for kernel in ["aaa_first", "zzz_escaped", "tabbed", "wide_cjk", "longline"] {
        assert!(
            screen.contains(&format!("kernel `{kernel}`")),
            "`{kernel}` is missing from the report:\n{screen}"
        );
    }

    // --- the caret. On every one of these lines the caret must occupy the
    // same terminal cell as the character the header names, whatever
    // precedes it: a tab, eight wide characters, or nothing at all.
    for kernel in ["aaa_first", "tabbed", "wide_cjk"] {
        let (snippet, carets) = block(&screen, kernel);
        let span = col_of(&screen, snippet, "thread::sync_threads")
            .unwrap_or_else(|| panic!("no span on the `{kernel}` snippet row:\n{screen}"));
        let caret = col_of(&screen, carets, "^")
            .unwrap_or_else(|| panic!("no caret on the `{kernel}` caret row:\n{screen}"));
        assert_eq!(
            caret, span,
            "`{kernel}`: the caret sits at cell {caret}, the span it names at \
             cell {span}\n{screen}"
        );
        // And the run is as wide as the text it underlines: `^` repeated
        // over `thread::sync_threads();`, which is 22 cells of ASCII.
        let width = screen.row_text(carets).matches('^').count();
        assert_eq!(width, 22, "`{kernel}` caret run:\n{screen}");
    }

    // --- the long line. Trimmed around its span rather than printed at 830
    // columns, so the report still fits a terminal.
    let (snippet, carets) = block(&screen, "longline");
    let row = screen.row_text(snippet);
    assert!(
        row.trim_end().len() < usize::from(COLS),
        "the snippet row must fit the terminal, got {} cells:\n{screen}",
        row.trim_end().len()
    );
    assert!(row.contains("..."), "the trim must be marked:\n{row}");
    let span =
        col_of(&screen, snippet, "thread::sync_threads").expect("the span survives the trim");
    let caret = col_of(&screen, carets, "^").expect("a caret");
    assert_eq!(
        caret, span,
        "the caret follows the trimmed window:\n{screen}"
    );

    // --- the gutter. Every `= note:` line is indented one past its own
    // line-number gutter, whatever that number's width.
    for kernel in ["aaa_first", "longline"] {
        let (header, _) = screen.find(&format!("kernel `{kernel}`")).expect("header");
        let arrow = screen.row_text(header + 1);
        let digits = arrow
            .trim_start()
            .rsplit(':')
            .nth(1)
            .expect("a line number in the --> line")
            .len();
        assert_eq!(
            arrow.len() - arrow.trim_start().len(),
            digits,
            "`{kernel}`: the --> line is indented by the line number's width\n{arrow:?}"
        );
    }

    assert!(
        !screen.alternate_screen(),
        "check is not a full-screen view"
    );

    // --- the premise this whole file rests on, which was written at the top
    // as a comment and asserted nowhere. The erasure case above distinguishes
    // "the escape wiped the visible grid" from "the report is intact"; if the
    // report were tall enough to scroll on its own, a diagnostic missing from
    // the grid would be indistinguishable from one that had simply scrolled
    // past, and the assertion could not tell the two apart. At 100x24 this
    // same run puts 83 rows into history; at 160x90 it must put none.
    assert_eq!(
        screen.scrollback_rows(),
        0,
        "nothing may scroll off on its own at {COLS}x{ROWS}, or the erasure \
         assertions above cannot tell an erased diagnostic from one that \
         merely scrolled past\nscrolled off:\n{}\nscreen:\n{screen}",
        screen.scrollback_text()
    );

    // --- and the grid those assertions read. `check` emits no sequence
    // termlens fails to implement, so every claim above is made against a
    // stream the emulator honoured in full rather than a plausible-looking
    // reconstruction of one.
    assert!(
        screen.unsupported().is_empty(),
        "`check` emitted a sequence termlens does not model, so this grid may \
         be wrong: {:?}",
        screen.unsupported()
    );
    assert_eq!(screen.unsupported_overflow(), 0);
    assert!(!screen.insert_mode(), "IRM would shift every row right");
    assert_eq!(screen.bells(), 0, "a report is text, never a beep");
    assert_eq!(screen.visual_bells(), 0);

    // --- why this file synchronizes with `snapshot_after` and the TUI suite
    // with `wait_frame`. `check` is not a full-screen view and brackets
    // nothing in DEC 2026, so it emits no complete frames at all: pointed at
    // this binary, `wait_frame` would run out its whole deadline. Pinning the
    // count keeps the two sync policies from being swapped by mistake.
    assert_eq!(
        screen.repaints(),
        0,
        "`check` emits no synchronized updates, which is why this file uses \
         `snapshot_after` rather than `wait_frame` (AGENTS.md, the termlens \
         skill rule 8)"
    );

    // --- one report, not two. The erasure bug removed diagnostics; a retry
    // loop or a doubled writer would add them, and `contains` cannot tell.
    assert_eq!(
        screen.find_all("kernel `aaa_first`").len(),
        1,
        "each kernel is reported once:\n{screen}"
    );

    // --- the note text, whole. The `= note:` lines are longer than the
    // terminal and rely on it to wrap them, so `contains` — which reads one
    // row at a time — cannot see a sentence that crosses the wrap, and a
    // reader who tried would wrongly conclude the note had been truncated.
    // `logical_text` rejoins the wrapped rows, which is the only way to
    // assert the note reached the grid in full.
    assert!(
        screen.logical_text().contains("usually a permanent hang"),
        "the RC001 note must reach the terminal whole; wrapped rows: {:?}\n{screen}",
        (0..screen.rows())
            .filter(|&row| screen.row_wrapped(row))
            .collect::<Vec<_>>()
    );

    Ok(())
}
