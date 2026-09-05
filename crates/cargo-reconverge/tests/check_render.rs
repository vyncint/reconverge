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
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check"])
        .current_dir(&project)
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
                Some(cell) if cell.contents().chars().next() == Some(*want) => {
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
    // back explicitly — under `env_clear` there is no PATH at all.
    let path = env::var("PATH").unwrap_or_default();
    let home = env::var("HOME").unwrap_or_default();
    let cargo = env::var("CARGO").unwrap_or_default();
    let cargo_home = env::var("CARGO_HOME").unwrap_or_default();
    let rustup_home = env::var("RUSTUP_HOME").unwrap_or_default();
    let driver = driver.to_str().expect("utf-8 driver path").to_string();

    let mut t = termlens::bin!(
        "cargo-reconverge",
        size(COLS, ROWS),
        timeout(TIMEOUT),
        current_dir(&project),
        env("PATH", &path),
        env("HOME", &home),
        env("CARGO", &cargo),
        env("CARGO_HOME", &cargo_home),
        env("RUSTUP_HOME", &rustup_home),
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
    Ok(())
}
