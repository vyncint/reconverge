//! End-to-end: the review loop over the real lint samples — `check`
//! finds and gates, a baseline accepts, `check` stops gating and says so,
//! SARIF reports the suppression the standard way, a stale entry is
//! surfaced, and `cargo reconverge triage` opens on the real artifacts and
//! writes the real file.
//!
//! Sync policy per §9 for the PTY leg: content-based `wait_until`, never
//! sleep.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use std::{env, fs};

use reconverge_artifacts::baseline::{BaselineArtifact, Entry};
use reconverge_artifacts::findings::FindingsArtifact;
use termlens::{Key, Terminal};

const TIMEOUT: Duration = Duration::from_secs(30);

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

fn ensure_sibling(package: &str, binary: &str) -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    let path = cli
        .parent()
        .unwrap()
        .join(format!("{binary}{}", std::env::consts::EXE_SUFFIX));
    if !path.is_file() {
        let status = Command::new(env::var("CARGO").unwrap())
            .args(["build", "-p", package])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .unwrap_or_else(|e| panic!("failed to spawn cargo build for {package}: {e}"));
        assert!(status.success(), "building {package} failed");
    }
    path
}

fn check(project: &Path, driver: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .arg("reconverge")
        .arg("check")
        .args(args)
        .current_dir(project)
        .env("RECONVERGE_DRIVER", driver)
        .output()
        .expect("failed to spawn cargo-reconverge check")
}

#[test]
fn the_review_loop_gates_accepts_and_reports() {
    let driver = ensure_sibling("reconverge-driver", "reconverge-driver");
    let tui = ensure_sibling("reconverge-tui", "reconverge-tui");

    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("t4-triage-samples");
    let _ = fs::remove_dir_all(project.join("src"));
    let _ = fs::remove_file(project.join("Cargo.toml"));
    let _ = fs::remove_file(project.join("Cargo.lock"));
    let _ = fs::remove_file(project.join("reconverge-baseline.json"));
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
        &project,
    );

    // --- 1. Unreviewed: gating findings, exit 1, nothing suppressed.
    let output = check(&project, &driver, &["--message-format", "json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifacts: Vec<FindingsArtifact> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json findings document"))
        .collect();
    let gating: Vec<(String, Option<String>, String)> = artifacts
        .iter()
        .flat_map(|a| {
            a.findings
                .iter()
                .filter(|f| f.confidence.gates_exit_code())
                .map(|f| (a.krate.clone(), f.kernel.clone(), f.code.clone()))
        })
        .collect();
    assert!(
        gating.len() >= 3,
        "the samples must carry several gating findings: {gating:?}"
    );
    let warnings = artifacts
        .iter()
        .flat_map(|a| &a.findings)
        .filter(|f| !f.confidence.gates_exit_code())
        .count();

    // --- 2. A baseline accepting every gating finding, with reasons.
    let mut baseline = BaselineArtifact::empty();
    for (krate, kernel, code) in &gating {
        baseline.entries.push(Entry {
            krate: krate.clone(),
            kernel: kernel.clone(),
            code: code.clone(),
            reason: format!("reviewed by the end-to-end test: {code} is expected here"),
        });
    }
    baseline.normalize();
    let entries = baseline.entries.len();
    let baseline_path = project.join("reconverge-baseline.json");
    baseline.write_to(&baseline_path).unwrap();

    // The default path is the workspace root, so no flag is needed.
    let output = check(&project, &driver, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "accepted findings must not gate\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "0 deny, 0 confirmed, {warnings} warning finding(s) ({warnings} hidden; rerun with \
             --strict to see them); {} suppressed by the baseline",
            gating.len()
        )),
        "summary must report the suppressions: {stdout}"
    );
    assert!(
        !stdout.contains("error[RC"),
        "suppressed findings are hidden by default: {stdout}"
    );
    assert!(
        !stderr.contains("no longer matches"),
        "nothing is stale yet: {stderr}"
    );

    // --- 3. --show-suppressed brings them back, with their reasons.
    let output = check(&project, &driver, &["--show-suppressed"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("suppressed[RC003]:"), "{stdout}");
    assert!(
        stdout.contains("= baseline: reviewed by the end-to-end test"),
        "{stdout}"
    );

    // --- 4. SARIF marks them the standard way instead of dropping them.
    let sarif_path = project.join("reconverge.sarif");
    let output = check(
        &project,
        &driver,
        &["--sarif", sarif_path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0));
    let sarif: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sarif_path).unwrap()).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    let suppressed: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| r.get("suppressions").is_some())
        .collect();
    assert_eq!(
        suppressed.len(),
        gating.len(),
        "every accepted finding carries a SARIF suppression"
    );
    assert_eq!(suppressed[0]["suppressions"][0]["kind"], "external");
    assert!(
        suppressed[0]["suppressions"][0]["justification"]
            .as_str()
            .unwrap()
            .contains("reviewed by the end-to-end test")
    );
    let open: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| r.get("suppressions").is_none())
        .collect();
    assert!(!open.is_empty(), "unsuppressed findings stay plain results");

    // --- 5. A stale entry is surfaced, never silently kept.
    let mut with_stale = baseline.clone();
    with_stale.entries.push(Entry {
        krate: "lint_samples".into(),
        kernel: Some("kernel_that_no_longer_exists".into()),
        code: "RC001".into(),
        reason: "left behind after a refactor".into(),
    });
    with_stale.write_to(&baseline_path).unwrap();
    let output = check(&project, &driver, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stale entries never gate");
    assert!(
        stderr.contains("lint_samples RC001 in `kernel_that_no_longer_exists`")
            && stderr.contains("no longer matches any finding"),
        "{stderr}"
    );
    baseline.write_to(&baseline_path).unwrap();

    // --- 6. An explicitly named baseline that does not exist is an error,
    //        never a silent "everything is fine".
    let output = check(&project, &driver, &["--baseline", "nope.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read"));

    // --- 7. `cargo reconverge triage` opens on the real artifacts and
    //        writes the real file.
    fs::remove_file(&baseline_path).unwrap();
    let mut builder = Terminal::builder()
        .size(100, 30)
        .env_clear()
        .timeout(TIMEOUT)
        .current_dir(&project)
        .env("RECONVERGE_TUI", tui.to_str().unwrap())
        .arg("reconverge")
        .arg("triage");
    for var in [
        "PATH",
        "HOME",
        "CARGO",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "RUSTC",
    ] {
        if let Ok(value) = env::var(var) {
            builder = builder.env(var, &value);
        }
    }
    let mut t = builder
        .spawn(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .expect("failed to spawn cargo reconverge triage in a PTY");

    t.wait_until(|s| s.contains("reconverge triage") && s.contains("0 suppressed"))
        .expect("triage opens on this run's findings");
    t.send(Key::Char('s'));
    t.wait_until(|s| s.contains("why is this acceptable?"))
        .expect("reason editor");
    for c in "accepted from the CLI".chars() {
        t.send(Key::Char(c));
    }
    t.send(Key::Enter);
    t.wait_until(|s| s.contains("1 suppressed"))
        .expect("accepted");
    t.send(Key::Char('w'));
    t.wait_until(|s| s.contains("baseline written"))
        .expect("written");
    t.send(Key::Char('q'));
    assert!(t.wait_exit().expect("triage did not exit").success());

    let written: BaselineArtifact =
        serde_json::from_str(&fs::read_to_string(&baseline_path).unwrap()).unwrap();
    assert_eq!(written.entries.len(), 1);
    assert_eq!(written.entries[0].reason, "accepted from the CLI");
    assert_eq!(written.entries[0].krate, "lint_samples");

    // The written entry is honored by the very next check.
    let output = check(&project, &driver, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 suppressed by the baseline"),
        "triage's own output must round-trip into check: {stdout}"
    );
    assert_eq!(
        entries,
        gating.len(),
        "sanity: one entry per gating finding"
    );
}

/// `watch` runs the check, waits for a save, and runs it again.
#[test]
fn watch_reruns_the_check_when_a_source_file_changes() {
    let driver = ensure_sibling("reconverge-driver", "reconverge-driver");
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("t4-watch-samples");
    let _ = fs::remove_dir_all(project.join("src"));
    let _ = fs::remove_file(project.join("Cargo.toml"));
    let _ = fs::remove_file(project.join("Cargo.lock"));
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
        &project,
    );

    let mut builder = Terminal::builder()
        .size(100, 30)
        .env_clear()
        .timeout(Duration::from_secs(180))
        .current_dir(&project)
        .env("RECONVERGE_DRIVER", driver.to_str().unwrap())
        .arg("reconverge")
        .arg("watch")
        .arg("--max-runs")
        .arg("2");
    for var in [
        "PATH",
        "HOME",
        "CARGO",
        "CARGO_HOME",
        "RUSTUP_HOME",
        // Without the toolchain pin the child's cargo and rustc can
        // resolve differently from the caller's, and the sample's cached
        // dependencies then look "compiled by an incompatible version".
        "RUSTUP_TOOLCHAIN",
        "RUSTC",
    ] {
        if let Ok(value) = env::var(var) {
            builder = builder.env(var, &value);
        }
    }
    let mut t = builder
        .spawn(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .expect("failed to spawn cargo reconverge watch in a PTY");

    // Run #1 completes — with a real analysis, not a build error — and the
    // loop settles into waiting.
    t.wait_until(|s| s.contains("run #1") && s.contains("file(s) watched"))
        .expect("first run announced");
    t.wait_until(|s| s.contains("reconverge: ") && s.contains("deny,"))
        .expect("the first run analyzed the samples");
    t.wait_until(|s| s.contains("waiting for changes"))
        .expect("first run finished and the loop is watching");

    // A save triggers run #2 — the whole point of the subcommand.
    let source = project.join("src/lib.rs");
    let text = fs::read_to_string(&source).unwrap();
    fs::write(&source, format!("{text}\n// touched by the watch test\n")).unwrap();

    t.wait_until(|s| s.contains("run #2"))
        .expect("the save triggered a re-run");

    let status = t
        .wait_exit()
        .expect("watch did not stop after --max-runs 2");
    // The visible tail is the second run's own report — the loop analyzed
    // again rather than exiting on the trigger. (The first run's output has
    // scrolled off by now: this is a terminal, not a transcript.)
    let tail = t.screen().to_string();
    assert!(
        tail.contains("reconverge: ") && tail.contains("deny,"),
        "the second run must end in its own summary:\n{tail}"
    );
    assert!(
        !status.success(),
        "the exit code is the last check's: the samples still have findings          ({status:?})"
    );
}
