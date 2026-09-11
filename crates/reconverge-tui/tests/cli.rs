//! `termlens-cli` — the command that ships beside the harness this suite
//! already uses, pointed at this repository's own goldens.
//!
//! The rest of the suite asks whether the TUI draws the right thing. This
//! asks what a maintainer does *after* a golden fails in CI: read the two
//! screens somewhere other than the machine that ran the tests. That works
//! here only because `assert_golden` writes `Screen::to_string()` — every
//! file under `tests/golden/` is already a saved screen in termlens' snapshot
//! text format, so `termlens diff` and `termlens render` read them with no
//! conversion. Nothing asserted that until now, and the way it breaks is
//! silent: a hand-edited golden, or a bless path that stopped going through
//! `Display`, and the goldens are still self-consistent while no tool can
//! open them.
//!
//! Ignored by default. These need `termlens-cli` on the machine, and both
//! crates here are published to crates.io — a `cargo test` that quietly
//! `cargo install`s something is not a surprise to spring on a contributor
//! or on a packager. CI runs them by name with `--ignored`.
//!
//! ```sh
//! cargo test -p reconverge-tui --test cli -- --ignored
//! ```
//!
//! `$TERMLENS_CLI` short-circuits the install when a `termlens` binary is
//! already on the machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::{env, fs};

/// The termlens version this suite is measured against, read from the
/// workspace lockfile so the tool and the library can never be two different
/// releases. `Cargo.lock` is committed and `crates/*/Cargo.toml` pins a
/// `"0.10"` range, so the lockfile is the only place the exact version is
/// written down.
fn version_under_test() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let lock = fs::read_to_string(workspace_root().join("Cargo.lock"))
            .expect("Cargo.lock is committed at the workspace root");
        let mut lines = lock.lines();
        while let Some(line) = lines.next() {
            if line.trim() == "name = \"termlens\"" {
                for next in lines.by_ref() {
                    if let Some(rest) = next.trim().strip_prefix("version = \"") {
                        return rest.trim_end_matches('"').to_owned();
                    }
                }
            }
        }
        panic!("no termlens version in Cargo.lock");
    })
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn fixture(rel: &str) -> PathBuf {
    workspace_root().join("fixtures").join(rel)
}

/// The `termlens` binary: `$TERMLENS_CLI` if the environment provides one,
/// otherwise installed once into `target/` at the version under test.
fn cli() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(given) = env::var_os("TERMLENS_CLI") {
            return PathBuf::from(given);
        }
        let root = workspace_root().join("target").join("termlens-cli");
        let bin = root
            .join("bin")
            .join(format!("termlens{}", env::consts::EXE_SUFFIX));
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["install", "termlens-cli", "--version", version_under_test()])
            .args(["--locked", "--root"])
            .arg(&root)
            .status()
            .expect("cargo install termlens-cli");
        assert!(
            status.success(),
            "cargo install termlens-cli --version {} failed. It is published \
             alongside the library; if this version of termlens exists on \
             crates.io and termlens-cli does not, the two releases went out \
             of lockstep.",
            version_under_test()
        );
        bin
    })
}

fn run(args: &[&str]) -> Output {
    Command::new(cli())
        .args(args)
        .output()
        .expect("run the termlens command")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The grid half of `termlens render --text`, without the `styles:` block it
/// appends. `rsplit_once` because a grid row can be blank, but the styles
/// block is always last.
fn grid_of(rendered: &str) -> &str {
    rendered
        .rsplit_once("\n\nstyles:")
        .map_or(rendered, |(grid, _)| grid)
}

fn trim_lines(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

#[test]
#[ignore = "needs termlens-cli, and both crates here are published; CI runs it with --ignored"]
fn the_tool_and_the_library_are_one_release() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("termlens {}", version_under_test()),
        "the installed CLI is not the version Cargo.lock names for the library"
    );
}

/// Every checked-in golden is a screen the tool can open, and reading one
/// back reproduces it exactly.
///
/// This is the property `assert_golden` depends on without saying so: it
/// writes `Screen::to_string()`, which is the snapshot text format, and the
/// header line (`size: 80x24  cursor: hidden`) is part of the comparison. A
/// golden hand-edited into something `Screen::parse` cannot read still
/// compares fine against itself, so the whole-suite failure mode is that the
/// day a golden breaks is the day nobody can diff it.
#[test]
#[ignore = "needs termlens-cli, and both crates here are published; CI runs it with --ignored"]
fn every_checked_in_golden_is_a_screen_the_tool_can_read() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("the golden directory exists") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let out = run(&["render", "--text", path.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "{name}: termlens could not read this golden: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let rendered = stdout(&out);
        assert_eq!(
            trim_lines(grid_of(&rendered)),
            trim_lines(&fs::read_to_string(&path).unwrap()),
            "{name}: reading the golden back did not reproduce it"
        );
        // The header the format needs, which is also the assertion
        // `assert_golden` makes without naming it: the geometry and the
        // cursor are part of a golden, not decoration.
        assert!(
            rendered.starts_with("size: "),
            "{name}: a golden carries its size and cursor:\n{rendered}"
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "only {checked} goldens found; expected the full set"
    );
}

/// The workflow after a golden fails: run the real binary through the tool
/// and compare with the checked-in file. `inspect` needs no test written for
/// it, which is exactly why a contributor reaches for it first — and the two
/// paths into a screen (this suite's `Terminal`, and the CLI's) have to agree
/// or a maintainer chasing a CI failure is looking at a different picture
/// from the one that failed.
#[test]
#[ignore = "needs termlens-cli, and both crates here are published; CI runs it with --ignored"]
fn inspect_reproduces_the_checked_in_shell_golden() {
    let findings = fixture("findings/rc003-minimal.json");
    let unimap = fixture("unimap/divergent-barrier.json");
    let witness = fixture("witness/rc001-divergent-barrier.json");
    let out = run(&[
        "inspect",
        "--size",
        "80x24",
        "--idle",
        "600",
        "--timeout",
        "10",
        env!("CARGO_BIN_EXE_reconverge-tui"),
        findings.to_str().unwrap(),
        unimap.to_str().unwrap(),
        witness.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = stdout(&out);
    assert!(printed.starts_with("size: 80x24"), "{printed}");
    // The shell is a TUI, so inspect reports the deadline rather than an
    // exit — on **stderr** since termlens 0.11 (termlens#340), so that what
    // stdout carries is a saved screen and needs no filtering here. Against
    // 0.10.x the trailer followed the screen on stdout and this test dropped
    // the line itself.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("still running at the deadline"),
        "the shell does not exit on its own: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !printed.contains("--- "),
        "stdout is the screen alone:\n{printed}"
    );
    let screen = printed.clone();
    let live = env::temp_dir().join(format!("reconverge-inspect-{}.snap", std::process::id()));
    fs::write(&live, &screen).expect("write the inspected screen");

    let out = run(&[
        "diff",
        "--color",
        "never",
        live.to_str().unwrap(),
        golden("shell-80x24.txt").to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the real binary through `termlens inspect` is not the checked-in \
         golden:\n{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("no difference"));
    let _ = fs::remove_file(&live);
}

/// `diff` and its three exit codes, which are what a script reads: 0 the same
/// picture, 1 a difference, 2 the tool could not run. Driven over two real
/// goldens whose difference is a known one — the barrier moment against the
/// verdict moment of the same replay — so the rendering is checked against
/// something a reader can recognise rather than against itself.
#[test]
#[ignore = "needs termlens-cli, and both crates here are published; CI runs it with --ignored"]
fn diff_says_what_changed_between_two_goldens_and_reports_it_in_its_exit_code() {
    let verdict = golden("witness-verdict-80x24.txt");
    let barrier = golden("witness-barrier-80x24.txt");

    let same = run(&[
        "diff",
        "--color",
        "never",
        verdict.to_str().unwrap(),
        verdict.to_str().unwrap(),
    ]);
    assert_eq!(same.status.code(), Some(0), "a screen equals itself");
    assert!(stdout(&same).contains("no difference"));

    let out = run(&[
        "diff",
        "--color",
        "never",
        barrier.to_str().unwrap(),
        verdict.to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stepping to the verdict changes the picture"
    );
    let rendered = stdout(&out);
    assert!(rendered.contains("size: 80x24"), "the header:\n{rendered}");
    assert!(
        rendered.contains("rows unchanged"),
        "and a count of what did not move:\n{rendered}"
    );
    // The lane strip is the row a reader would look at first, and it is the
    // one AGENTS.md calls load-bearing: `WoWoWoWo` (waiting/active) becomes
    // `W.W.W.W.` (waiting/exited) when the odd lanes leave.
    assert!(
        rendered.contains("WoWoWoWo") && rendered.contains("W.W.W.W."),
        "the diff shows the warp diagram changing:\n{rendered}"
    );

    let missing = run(&[
        "diff",
        "--color",
        "never",
        verdict.to_str().unwrap(),
        "no-such-golden.txt",
    ]);
    assert_eq!(
        missing.status.code(),
        Some(2),
        "an unreadable file is the tool failing, not a difference"
    );
}

/// `render --svg` is what the CI report action puts in a step summary, so a
/// failing frame is a picture rather than a wall of log. For this repository
/// that has to carry the lane glyph language — AGENTS.md: the ASCII warp
/// diagram a CI log prints *is* a frame of the witness view — and the box
/// drawing the panels are made of.
#[test]
#[ignore = "needs termlens-cli, and both crates here are published; CI runs it with --ignored"]
fn render_svg_keeps_the_lane_glyphs_and_the_box_drawing() {
    let out = run(&[
        "render",
        "--svg",
        golden("witness-verdict-80x24.txt").to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = stdout(&out);
    assert!(svg.starts_with("<svg "), "an SVG document:\n{svg}");
    for needle in [
        "W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W.",
        "verdict: undefined behavior",
        "o active   W waiting   . exited",
        "\u{250c}", // the panel's top-left corner
    ] {
        assert!(svg.contains(needle), "{needle:?} is missing from the SVG");
    }
}
