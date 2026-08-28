//! End-to-end tests of the CLI contract (README.md) over the
//! lint-sample kernels: text/JSON/SARIF rendering, `--strict`, `--cc`, and
//! the 0/1 exit codes. The samples are copied into the target tmp dir so
//! checked-in sources stay untouched; both copies live under the repo, so
//! the pinned toolchain applies.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

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

/// The driver binary the CLI shells out to. `cargo test --workspace` builds
/// it for the driver crate's own tests; an isolated `cargo test -p
/// cargo-reconverge` builds it here on demand.
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

fn check(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .arg("reconverge")
        .arg("check")
        .args(args)
        .current_dir(project)
        .env("RECONVERGE_DRIVER", ensure_driver())
        .output()
        .expect("failed to spawn cargo-reconverge")
}

fn prepared_copy(name: &str, source: &Path) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    // Refresh sources but keep the target dir: dependency builds stay
    // cached across runs while the samples always re-lint.
    let _ = fs::remove_dir_all(dir.join("src"));
    let _ = fs::remove_file(dir.join("Cargo.toml"));
    let _ = fs::remove_file(dir.join("Cargo.lock"));
    copy_dir(source, &dir);
    dir
}

#[test]
fn lint_samples_report_all_codes_and_gate_the_exit() {
    let project = prepared_copy(
        "m1-lint-samples",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );

    // --- default text mode: deny findings shown, warnings hidden, exit 1.
    let output = check(&project, &["--cc", "8.6"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "deny findings must exit 1\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("error[RC003]"), "stdout:\n{stdout}");
    assert!(stdout.contains("error[RC004]"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("RC005"),
        "warnings hidden by default:\n{stdout}"
    );
    assert!(stdout.contains("rerun with --strict"), "stdout:\n{stdout}");
    // Witness-confirmed findings show by default, with the concrete
    // configuration and the ASCII warp diagram (§7).
    assert!(stdout.contains("error[RC001]"), "stdout:\n{stdout}");
    assert!(stdout.contains("error[RC002]"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("witness: replayed with grid (1,1,1) x block (32,1,1)"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("W.W.W.W. W.W.W.W. W.W.W.W. W.W.W.W."),
        "the warp diagram must render:\n{stdout}"
    );
    // The barrier case counts 49148 + 8 bytes.
    assert!(stdout.contains("49156 bytes"), "stdout:\n{stdout}");
    // The at-limit kernel must NOT be flagged.
    assert!(!stdout.contains("rc004_ok_at_limit"), "stdout:\n{stdout}");

    // A length given as a named const is read, and read correctly. Before
    // this was fixed the static vanished from the budget with no finding and
    // no diagnostic, so the over-budget kernel below came back clean — and a
    // tuner rewriting named consts per candidate hit that path every time.
    assert!(
        stdout.contains("kernel `rc004_named_const_over_budget` declares 65536 bytes"),
        "a named-const length must reach RC004:\n{stdout}"
    );
    // ...and the under-budget one through the same path must not be flagged,
    // or "resolved" would only mean "reported".
    assert!(
        !stdout.contains("rc004_ok_named_const_under"),
        "a named const under the cap is not a finding:\n{stdout}"
    );

    // --- strict text mode: warnings appear, exit still 1.
    let output = check(&project, &["--strict"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("warning[RC005]"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("declares `domain = 2` but calls `index_1d()`"),
        "stdout:\n{stdout}"
    );

    // --- JSON mode: exact (code, kernel) inventory.
    let sarif_path = project.join("report.sarif");
    let output = check(
        &project,
        &[
            "--message-format",
            "json",
            "--sarif",
            sarif_path.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut inventory: BTreeMap<(String, String), usize> = BTreeMap::new();
    for line in stdout.lines() {
        let artifact: serde_json::Value = serde_json::from_str(line).expect("JSON line");
        assert_eq!(artifact["schema"], "findings.v1");
        assert_eq!(artifact["crate"], "lint_samples");
        for finding in artifact["findings"].as_array().unwrap() {
            let key = (
                finding["code"].as_str().unwrap().to_string(),
                finding["kernel"].as_str().unwrap().to_string(),
            );
            *inventory.entry(key).or_default() += 1;
        }
    }
    let expected: BTreeMap<(String, String), usize> = [
        // The canonical pair: the divergent barrier is flagged, the
        // block-uniform one and the reconverged one are not.
        ("RC001", "rc001_divergent_barrier"),
        ("RC001", "rc001_divergent_call"),
        ("RC002", "rc002_divergent_collective"),
        ("RC002", "rc002_divergent_call"),
        ("RC003", "rc003_mut_slice"),
        ("RC004", "rc004_over_budget"),
        ("RC004", "rc004_barrier_pushes_over"),
        ("RC004", "rc004_named_const_over_budget"),
        ("RC005", "rc001_divergent_barrier"),
        ("RC005", "rc001_divergent_call"),
        ("RC005", "rc001_ok_block_uniform"),
        ("RC005", "rc001_ok_reconverged"),
        ("RC005", "rc002_divergent_collective"),
        ("RC005", "rc002_divergent_call"),
        ("RC005", "rc002_ok_convergent"),
        ("RC005", "rc003_ok_shared_ref"),
        ("RC005", "rc004_over_budget"),
        ("RC005", "rc004_barrier_pushes_over"),
        ("RC005", "rc004_ok_at_limit"),
        ("RC005", "rc004_named_const_over_budget"),
        ("RC005", "rc004_ok_named_const_under"),
        ("RC005", "rc005_mismatch"),
        ("RC005", "rc005_missing_contract"),
    ]
    .into_iter()
    .map(|(code, kernel)| ((code.to_string(), kernel.to_string()), 1))
    .collect();
    assert_eq!(inventory, expected);

    // The canonical RC001 is witness-confirmed (M4) and still carries a
    // provenance chain ending at the thread-index witness (§5).
    let findings: Vec<serde_json::Value> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .flat_map(|artifact| artifact["findings"].as_array().cloned().unwrap_or_default())
        .collect();
    let by = |code: &str, kernel: &str| {
        findings
            .iter()
            .find(|f| f["code"] == code && f["kernel"] == kernel)
            .unwrap_or_else(|| panic!("missing {code} on {kernel}"))
            .clone()
    };
    let rc001 = by("RC001", "rc001_divergent_barrier");
    assert_eq!(rc001["confidence"], "confirmed");
    let provenance = rc001["provenance"].as_array().unwrap();
    assert!(
        provenance.last().unwrap()["what"]
            .as_str()
            .unwrap()
            .contains("index_1d"),
        "provenance must end at the witness: {provenance:?}"
    );
    assert_eq!(
        by("RC002", "rc002_divergent_collective")["confidence"],
        "confirmed"
    );
    // An interprocedural site is promoted when the callee can be inlined,
    // which turns "the summary says it may reach a barrier" into an actual
    // path. Both helpers here reach their site unconditionally and are
    // called under a divergent guard, so both are real hangs — the sample
    // comments have always called them true positives, and they now carry
    // the witness to prove it.
    assert_eq!(
        by("RC001", "rc001_divergent_call")["confidence"],
        "confirmed"
    );
    assert_eq!(
        by("RC002", "rc002_divergent_call")["confidence"],
        "confirmed"
    );

    // The witness artifacts themselves parse as witness.v1 with concrete
    // undefined-behavior verdicts.
    let witness_dir = project.join("target/reconverge");
    let mut witness_count = 0;
    for entry in fs::read_dir(&witness_dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("witness-") {
            continue;
        }
        let artifact: reconverge_artifacts::witness::WitnessArtifact =
            serde_json::from_str(&fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|e| panic!("{name} is not witness.v1: {e}"));
        assert_eq!(artifact.schema, "witness.v1");
        assert_eq!(artifact.lanes, 32);
        assert_eq!(
            artifact.verdict.kind,
            reconverge_artifacts::witness::VerdictKind::UndefinedBehavior
        );
        witness_count += 1;
    }
    // Four: the two direct sites, plus the two interprocedural ones that
    // inlining turned into concrete paths.
    assert_eq!(witness_count, 4, "one witness per confirmed finding");

    // --- SARIF: the full registry of rules, one result per finding.
    let sarif: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sarif_path).unwrap()).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let run = &sarif["runs"][0];
    let rule_ids: Vec<&str> = run["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(rule_ids, ["RC001", "RC002", "RC003", "RC004", "RC005"]);
    assert_eq!(run["results"].as_array().unwrap().len(), expected.len());
}

/// A bad flag *value* answers in one line; an unrecognised *argument* gets
/// the reference.
///
/// The distinction is what a calling tool depends on. launchbound shells out
/// to `check` and, on a tool error, reports the tail of stderr — a reasonable
/// default, since a failing tool usually fails last. When every argument
/// error printed the whole usage text, that tail was the exit-code legend,
/// and the reason (`80` is not a compute capability) was forty-four lines
/// above it, out of view. It reported the legend as the cause. Eleven times,
/// once per candidate.
#[test]
fn a_bad_value_answers_in_one_line_and_an_unknown_argument_gets_the_usage() {
    let bad_value = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check", "--cc", "80"])
        .output()
        .unwrap();
    assert_eq!(bad_value.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_value.stderr);
    assert_eq!(
        stderr.trim(),
        "error: `80` is not a compute capability; expected e.g. `8.6`",
        "a value error is the whole of stderr:\n{stderr}"
    );

    // The property the caller actually relies on: the last line is the reason.
    assert!(
        stderr
            .trim()
            .lines()
            .next_back()
            .unwrap()
            .starts_with("error:"),
        "the tail of stderr must be the diagnosis:\n{stderr}"
    );

    for argv in [
        ["reconverge", "check", "--bogus"],
        ["reconverge", "frobnicate", ""],
    ] {
        let argv: Vec<&str> = argv.iter().copied().filter(|a| !a.is_empty()).collect();
        let unknown = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .args(&argv)
            .output()
            .unwrap();
        assert_eq!(unknown.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&unknown.stderr);
        assert!(
            stderr.starts_with("error: unrecognized argument"),
            "{argv:?} stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("cargo reconverge check [OPTIONS]"),
            "an unrecognised argument still gets the reference: {argv:?}\n{stderr}"
        );
    }
}

#[test]
fn help_explains_that_text_filters_do_not_change_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--strict and --show-suppressed affect text output only"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("JSON is always the unfiltered record"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn inspect_reports_missing_artifacts_and_bad_flags() {
    // Bad flag: tool error.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "inspect", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));

    // A project that has never run `check`: tool error with the hint.
    let project = prepared_copy(
        "m1-inspect-no-artifacts",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    let _ = fs::remove_dir_all(project.join("target/reconverge"));
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "inspect"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("run `cargo reconverge check` first"),
        "stderr:\n{stderr}"
    );
}

/// The driver sample crate carries the canonical hang; the witness
/// confirms it, so the sample gates the exit code by default — while its
/// clean `scale` kernel contributes nothing.
#[test]
fn sample_hang_is_confirmed_and_gates_the_exit() {
    let project = prepared_copy(
        "m1-clean-sample",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../reconverge-driver/tests/sample-kernels"),
    );
    let output = check(&project, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "the confirmed hang must exit 1\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("error[RC001]"), "stdout:\n{stdout}");
    assert!(stdout.contains("witness: replayed"), "stdout:\n{stdout}");
    assert!(!stdout.contains("`scale`"), "scale stays clean:\n{stdout}");
}
