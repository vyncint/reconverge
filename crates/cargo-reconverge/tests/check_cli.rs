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
    assert!(
        stdout.contains("use an index formula that covers two axes, or narrow the contract"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("narrow the contract; no recognized index formula covers three axes"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("covers 2 two axes") && !stdout.contains("covers 3 two axes"),
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
        ("RC005", "rc005_domain3_index1d"),
        ("RC005", "rc005_mismatch"),
        ("RC005", "rc005_missing_contract"),
        // The unmasked wrapper: `warp::ballot(x)` delegates to
        // `ballot_sync(0xffff_ffff, x)` inside cuda-device, so it is
        // analyzed exactly as the explicit full mask. `--explain RC002`
        // called it "not yet checked" for two releases after the
        // recognizer learned it, and no fixture held the page to the code.
        ("RC002", "rc002_unmasked_wrapper"),
        ("RC005", "rc002_unmasked_wrapper"),
        // The declared two-warp block, with and without a collective on the
        // path. Both promote: a collective stopped the multi-warp replay
        // until #30 and the README still said it did.
        ("RC001", "rc001_multiwarp_barrier"),
        ("RC001", "rc001_multiwarp_barrier_after_collective"),
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
    assert_eq!(
        by("RC005", "rc005_mismatch")["help"],
        "use an index formula that covers two axes, or narrow the contract"
    );
    assert_eq!(
        by("RC005", "rc005_domain3_index1d")["help"],
        "narrow the contract; no recognized index formula covers three axes"
    );
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
    let mut multiwarp = 0;
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
        // Whole warps up to 128, not "always 32": the declared-block
        // replay has written 64, 96 and 128 since 0.1.12 while the
        // published schema pinned `lanes` at 32, so the artifacts that
        // violated the project's own contract were exactly the gating ones.
        assert!(
            matches!(artifact.lanes, 32 | 64 | 96 | 128),
            "{name}: lanes = {}",
            artifact.lanes
        );
        assert_eq!(
            artifact.initial_lane_states.len(),
            usize::from(artifact.lanes)
        );
        assert_eq!(
            artifact.verdict.kind,
            reconverge_artifacts::witness::VerdictKind::UndefinedBehavior
        );
        // At a collective, the lanes the strip shows present are exactly
        // the set bits of `warp_op.active`. Asserted on a *driver* artifact:
        // the hand-written fixtures obeyed it all along, which is why the
        // golden frame stayed coherent while the shipping one did not.
        assert_eq!(
            artifact.first_collective_disagreeing_with_its_mask(),
            None,
            "{name}: a collective's lane strip contradicts its own mask"
        );
        if artifact.lanes > 32 {
            multiwarp += 1;
        }
        witness_count += 1;
    }
    // Seven: the two direct sites, the two interprocedural ones that
    // inlining turned into concrete paths, the unmasked wrapper, and the
    // two multi-warp barriers.
    assert_eq!(witness_count, 7, "one witness per confirmed finding");
    assert_eq!(
        multiwarp, 2,
        "an ordinary run must emit a witness wider than one warp; without \
         one, nothing in the suite ever serializes the shape the schema \
         used to reject"
    );

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
    let sarif_messages = run["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["message"]["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        sarif_messages
            .contains("use an index formula that covers two axes, or narrow the contract"),
        "sarif messages:\n{sarif_messages}"
    );
    assert!(
        sarif_messages
            .contains("narrow the contract; no recognized index formula covers three axes"),
        "sarif messages:\n{sarif_messages}"
    );
    assert!(
        !sarif_messages.contains("covers 2 two axes")
            && !sarif_messages.contains("covers 3 two axes"),
        "sarif messages:\n{sarif_messages}"
    );
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

    // A value-taking flag must not swallow the next flag as its value.
    // `--sarif --strict` used to write a SARIF report to a file named
    // `--strict` and silently drop strict mode.
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flag-eats-flag");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let eaten = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check", "--sarif", "--strict"])
        .current_dir(&scratch)
        .output()
        .unwrap();
    assert_eq!(eaten.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&eaten.stderr);
    assert_eq!(
        stderr.trim(),
        "error: `--sarif` requires a value (got the flag `--strict`)",
        "a swallowed flag is a value error, not usage:\n{stderr}"
    );
    assert!(
        !scratch.join("--strict").exists(),
        "must not write a report to a file named --strict"
    );

    let boolean = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check", "--strict=false"])
        .output()
        .unwrap();
    assert_eq!(boolean.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&boolean.stderr);
    assert_eq!(
        stderr.trim(),
        "error: `--strict` takes no value",
        "a boolean with a value is a value error, not usage:\n{stderr}"
    );

    let baseline = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check", "--baseline", "--sarif", "out.json"])
        .output()
        .unwrap();
    assert_eq!(baseline.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&baseline.stderr);
    assert_eq!(
        stderr.trim(),
        "error: `--baseline` requires a value (got the flag `--sarif`)",
        "a swallowed flag is a value error, not usage:\n{stderr}"
    );
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
    assert!(
        stdout.contains("--sarif=--weird"),
        "help must say how a path beginning with -- is passed:\n{stdout}"
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

/// Source spans are workspace-root-relative, so the caret snippet must
/// render no matter which directory inside the workspace `check` runs from.
/// Before the fix the reader resolved spans against the process cwd, so a
/// run from a member subdirectory read the wrong path and silently dropped
/// every snippet — the existing CLI tests never caught it because they all
/// run from the crate root.
#[test]
fn source_snippet_survives_a_subdirectory_cwd() {
    let project = prepared_copy(
        "m1-subdir-snippet",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    let driver = ensure_driver();

    let run = |dir: &Path| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .args(["reconverge", "check", "--cc", "8.6"])
            .current_dir(dir)
            .env("RECONVERGE_DRIVER", &driver)
            .output()
            .expect("failed to spawn cargo-reconverge");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    // The caret lines are the part that depends on reading the source file.
    let carets = |stdout: &str| -> Vec<String> {
        stdout
            .lines()
            .filter(|l| l.trim_start().starts_with('|') && l.contains('^'))
            .map(|l| l.trim().to_string())
            .collect()
    };

    let from_root = carets(&run(&project));
    assert!(
        !from_root.is_empty(),
        "the root run must render at least one source snippet"
    );
    let from_subdir = carets(&run(&project.join("src")));
    assert_eq!(
        from_subdir, from_root,
        "the source snippet must survive a subdirectory cwd, not be dropped"
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

/// A two-member workspace, built from scratch: `alpha` clean, `beta`
/// carrying a deny-tier RC003 and a barrier under a divergent guard.
///
/// Nothing in this repository had ever run `check` over a workspace with
/// two members — `tests/lint-samples` and the driver's sample crate both
/// carry a bare `[workspace]` marker to stand alone — and `default-members`
/// appeared nowhere in it. That is the whole coverage gap: the report has
/// always been workspace-wide while the build was whatever cargo picked
/// from the cwd, and no test could see the two disagree.
fn two_member_workspace(name: &str, default_members: Option<&str>) -> PathBuf {
    const DEP: &str = "cuda-device = { git = \"https://github.com/NVlabs/cuda-oxide\", \
                       rev = \"a766fc2650ea8e9e56c1481698b5dfdf01c31ded\" }";
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(dir.join("crates"));
    let _ = fs::remove_file(dir.join("Cargo.toml"));
    fs::create_dir_all(dir.join("crates/alpha/src")).unwrap();
    fs::create_dir_all(dir.join("crates/beta/src")).unwrap();

    let members = default_members.map_or_else(String::new, |m| format!("default-members = {m}\n"));
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[workspace]\nresolver = \"3\"\n\
             members = [\"crates/alpha\", \"crates/beta\"]\n{members}"
        ),
    )
    .unwrap();
    for member in ["alpha", "beta"] {
        fs::write(
            dir.join(format!("crates/{member}/Cargo.toml")),
            format!(
                "[package]\nname = \"{member}\"\nversion = \"0.0.0\"\n\
                 edition = \"2024\"\npublish = false\n\n[dependencies]\n{DEP}\n"
            ),
        )
        .unwrap();
    }
    fs::write(
        dir.join("crates/alpha/src/lib.rs"),
        "use cuda_device::{DisjointSlice, kernel, thread};\n\
         #[kernel]\npub fn alpha_kernel(mut out: DisjointSlice<u32>) {\n\
         \x20   let i = thread::index_1d();\n\
         \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("crates/beta/src/lib.rs"),
        "use cuda_device::{kernel, thread};\n\
         #[kernel]\npub fn beta_kernel(data: &mut [f32]) {\n\
         \x20   let i = thread::index_1d();\n\
         \x20   if i.get() % 2 == 0 { thread::sync_threads(); }\n\
         \x20   let _ = data;\n}\n",
    )
    .unwrap();
    dir
}

/// From a member directory, the gate must answer what it answers from the
/// root — and `default-members` must not narrow the analysis.
///
/// Before 0.5.0 the wrapped build was a bare `cargo check`, so cargo's own
/// package selection applied while the report and the exit code came from
/// every member `cargo metadata` lists. Two supported shapes reached it,
/// neither exotic: the action's own `working-directory` input points the
/// gate at a subdirectory, and `default-members` is ordinary workspace
/// hygiene. Both printed a deny-tier RC003 and a confirmed RC001 as
/// `0 deny, 0 confirmed`, exit 0 — or dropped the member from the report
/// entirely, which reads as a clean two-crate workspace.
#[test]
fn the_gate_covers_every_member_from_any_directory() {
    let root = two_member_workspace("m6-workspace", None);
    let from_root = check(&root, &["--strict"]);
    let root_out = String::from_utf8_lossy(&from_root.stdout).into_owned();
    assert_eq!(
        from_root.status.code(),
        Some(1),
        "beta's deny finding gates:\n{root_out}"
    );
    assert!(
        root_out.contains("error[RC003]: kernel `beta_kernel`"),
        "{root_out}"
    );
    assert!(
        root_out.contains("error[RC001]: kernel `beta_kernel`"),
        "{root_out}"
    );

    // The same tree, the same command, one directory apart.
    let from_member = check(&root.join("crates/alpha"), &["--strict"]);
    let member_out = String::from_utf8_lossy(&from_member.stdout).into_owned();
    assert_eq!(
        from_member.status.code(),
        from_root.status.code(),
        "the exit code must not depend on the cwd\n--- root ---\n{root_out}\n\
         --- crates/alpha ---\n{member_out}"
    );
    assert_eq!(
        summary_of(&member_out),
        summary_of(&root_out),
        "the summary must not depend on the cwd"
    );

    // And `default-members`, which narrows the build and not the report.
    let narrowed = two_member_workspace("m6-default-members", Some("[\"crates/alpha\"]"));
    let output = check(&narrowed, &["--strict"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        output.status.code(),
        Some(1),
        "a default-members line must not hide beta:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("beta_kernel"), "{stdout}");
}

/// The second identical run performs one wrapped build, not two.
///
/// The self-heal computed the set of members with no artifact, forced a
/// full re-lint with it, and then discarded it unread — so when a member
/// legitimately produced nothing the re-lint never helped and every run
/// paid for it forever, at a cost that scales with the dependency tree.
#[test]
fn a_second_identical_run_does_not_rebuild_the_world() {
    let project = prepared_copy(
        "m7-warm",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    check(&project, &[]);
    let second = check(&project, &[]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        compiled_lines(&second.stderr) <= 1,
        "a warm re-run must not re-lint twice:\n{stderr}"
    );
}

fn summary_of(stdout: &str) -> String {
    stdout
        .lines()
        .find(|l| l.starts_with("reconverge: "))
        .unwrap_or("<no summary>")
        .to_string()
}

/// A package with a lib and a bin writes one document per target, and the
/// two are told apart by something inside them.
///
/// `crate` looks like a primary key, is documented as one, and the driver's
/// own comment told consumers to key on it — so the natural consumer is a
/// dictionary keyed by `crate`, which drops half the findings. In the most
/// ordinary shape a GPU project takes, kernels in the lib and a thin host
/// binary beside them, the document that survives can be the empty one.
#[test]
fn a_lib_and_a_bin_are_two_documents_that_name_their_targets() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("m8-libbin");
    let _ = fs::remove_dir_all(dir.join("src"));
    let _ = fs::remove_file(dir.join("Cargo.toml"));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\n\n[package]\nname = \"libbin\"\nversion = \"0.0.0\"\n\
         edition = \"2024\"\npublish = false\n\n[dependencies]\n\
         cuda-device = { git = \"https://github.com/NVlabs/cuda-oxide\", \
         rev = \"a766fc2650ea8e9e56c1481698b5dfdf01c31ded\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "use cuda_device::{DisjointSlice, kernel, thread};\n\
         #[kernel]\npub fn lib_divergent(mut out: DisjointSlice<u32>) {\n\
         \x20   let i = thread::index_1d();\n\
         \x20   if i.get() % 2 == 0 { thread::sync_threads(); }\n\
         \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n",
    )
    .unwrap();
    // An ordinary host main, no kernel at all — the empty document.
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    let output = check(&dir, &["--message-format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let documents: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON document per line"))
        .collect();
    assert_eq!(
        documents.len(),
        2,
        "one document per compiled target:\n{stdout}"
    );
    let targets: Vec<&str> = documents
        .iter()
        .map(|d| {
            d["target"]
                .as_str()
                .expect("every document names its target")
        })
        .collect();
    assert_eq!(
        targets,
        ["bin", "lib"],
        "and the order is total, not read_dir's"
    );
    for document in &documents {
        assert_eq!(document["crate"], "libbin");
    }
    let lib = documents.iter().find(|d| d["target"] == "lib").unwrap();
    assert!(
        !lib["findings"].as_array().unwrap().is_empty(),
        "the lib target carries the kernel's findings:\n{stdout}"
    );
}

/// `check` into a reader that closes early is not a crash.
///
/// Rust's runtime sets `SIGPIPE` to `SIG_IGN`, so `println!` into a closed
/// pipe panics and the process exits 101 — outside the documented set (0
/// clean, 1 findings, 2 tool error) — and reports a rustc panic notice as
/// though the analyzer had crashed on the code under test.
/// `check --strict | head -40` is the most ordinary thing anyone does to a
/// long report.
#[test]
fn a_reader_that_closes_early_costs_the_report_and_not_the_verdict() {
    use std::process::Stdio;

    let project = prepared_copy(
        "m9-brokenpipe",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    // Warm, so the run under test is a cached re-check.
    check(&project, &["--strict"]);

    for args in [&["--strict"][..], &["--message-format", "json"][..]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .arg("reconverge")
            .arg("check")
            .args(args)
            .current_dir(&project)
            .env("RECONVERGE_DRIVER", ensure_driver())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn cargo-reconverge");
        // Close the read end while the report is still being written.
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("wait");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(1),
            "the verdict is computed before anything is rendered, so it \
             survives the reader going away ({args:?})\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "a closed reader is not a panic ({args:?}):\n{stderr}"
        );
    }
}

/// Every subcommand answers `--help` with usage and exit 0.
///
/// `<tool> <subcommand> --help` is the first thing anyone types, and being
/// told `error: unrecognized argument` is a poor greeting. The exit code
/// was 2 and the global usage did print, so nobody was stranded — but a
/// documented flag reported as an error is a documentation bug the user
/// has to disbelieve to get past.
#[test]
fn every_subcommand_answers_help() {
    for subcommand in [
        "check", "setup", "learn", "triage", "inspect", "witness", "watch",
    ] {
        for flag in ["--help", "-h"] {
            let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
                .args(["reconverge", subcommand, flag])
                .output()
                .expect("failed to spawn cargo-reconverge");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(0),
                "`{subcommand} {flag}` must not be an error\n{stderr}"
            );
            assert!(
                stdout.contains("cargo-reconverge:"),
                "`{subcommand} {flag}` must print usage:\n{stdout}"
            );
            assert!(
                !stderr.contains("unrecognized"),
                "`{subcommand} {flag}`:\n{stderr}"
            );
        }
    }
    // And a value-taking flag whose value is missing is still that, not
    // help: `--baseline --help` asked for a baseline named `--help`.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args(["reconverge", "check", "--baseline", "--help"])
        .output()
        .expect("failed to spawn cargo-reconverge");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires a value"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A witness for a finding that no longer exists must not survive a clean
/// run, and must not be replayed as current.
///
/// A witness was only ever created, never removed: `write_witnesses`
/// returned before touching the directory on exactly the run that should
/// clean it. Fix the kernel, re-run, get `0 confirmed`, open the debugger
/// and watch it replay `verdict: undefined behavior` on the barrier you
/// just hoisted. Nothing on that screen dates the artifact.
///
/// The coverage gap: every witness test deletes `src`, `Cargo.toml` and
/// `Cargo.lock` before each run but leaves `target/`, so they all start
/// from sources that have never changed under a warm artifacts directory —
/// the one shape that triggers this.
#[test]
fn a_witness_does_not_outlive_the_finding_it_replays() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("m10-stale-witness");
    let _ = fs::remove_dir_all(dir.join("src"));
    let _ = fs::remove_file(dir.join("Cargo.toml"));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\n\n[package]\nname = \"stale\"\nversion = \"0.0.0\"\n\
         edition = \"2024\"\npublish = false\n\n[dependencies]\n\
         cuda-device = { git = \"https://github.com/NVlabs/cuda-oxide\", \
         rev = \"a766fc2650ea8e9e56c1481698b5dfdf01c31ded\" }\n",
    )
    .unwrap();
    let divergent = "use cuda_device::{DisjointSlice, kernel, thread};\n\
         #[kernel]\npub fn single_kernel(mut out: DisjointSlice<u32>) {\n\
         \x20   let i = thread::index_1d();\n\
         \x20   if i.get() % 2 == 0 { thread::sync_threads(); }\n\
         \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n";
    fs::write(dir.join("src/lib.rs"), divergent).unwrap();

    assert_eq!(check(&dir, &["--strict"]).status.code(), Some(1));
    assert!(
        witness_files(&dir)
            .iter()
            .any(|n| n.contains("single_kernel")),
        "the confirmed finding writes a witness: {:?}",
        witness_files(&dir)
    );

    // Hoist the barrier out of the guard: the kernel is now clean.
    fs::write(
        dir.join("src/lib.rs"),
        "use cuda_device::{DisjointSlice, kernel, thread};\n\
         #[kernel]\npub fn single_kernel(mut out: DisjointSlice<u32>) {\n\
         \x20   let i = thread::index_1d();\n\
         \x20   thread::sync_threads();\n\
         \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n",
    )
    .unwrap();
    let output = check(&dir, &["--strict"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("error[RC001]"),
        "the hoisted barrier is clean:\n{stdout}"
    );
    assert_eq!(
        witness_files(&dir),
        Vec::<String>::new(),
        "a run that confirms nothing leaves no witness behind"
    );
}

/// How many times cargo says it built the sample crate.
///
/// The two words are matched separately on purpose: CI sets
/// `CARGO_TERM_COLOR: always`, so the line arrives as
/// `\x1b[1m\x1b[92m    Checking\x1b[0m lint-samples v0.0.0 (…)` and the
/// literal `"Checking lint-samples"` is not a substring of it. Asserting on
/// a colored stream with an uncolored needle passes locally and is a
/// tautology in CI — which is the shape of test this release exists to
/// stop shipping.
fn compiled_lines(stderr: &[u8]) -> usize {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|l| {
            l.contains("lint-samples") && (l.contains("Checking") || l.contains("Compiling"))
        })
        .count()
}

fn witness_files(project: &Path) -> Vec<String> {
    let dir = project.join("target/reconverge");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("witness-"))
        .collect();
    names.sort();
    names
}

/// A findings artifact this build did not produce is one it cannot vouch
/// for: refused by its schema tag, and named on stderr when its version
/// differs.
#[test]
fn a_foreign_artifact_is_refused_rather_than_gated_on() {
    let project = prepared_copy(
        "m11-skew",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    check(&project, &[]); // warm, so the build stays fresh

    let artifact = project.join("target/reconverge/findings-lint_samples-lib.json");
    let original = fs::read_to_string(&artifact).unwrap();

    // A tag out of range. The precondition is not a quirk: the edit is
    // between two runs with nothing else touched, so cargo's freshness
    // leaves the build alone and the driver is never asked to rewrite it.
    let mut document: serde_json::Value = serde_json::from_str(&original).unwrap();
    document["schema"] = serde_json::json!("findings.v99");
    fs::write(&artifact, document.to_string()).unwrap();
    let output = check(&project, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a document naming another schema is a tool error, not a report"
    );
    assert!(stderr.contains("findings.v99"), "{stderr}");
    assert!(stderr.contains("is not a findings.v1 artifact"), "{stderr}");

    // A plain version skew: valid, still gated on, but never silently.
    let mut document: serde_json::Value = serde_json::from_str(&original).unwrap();
    document["tool"]["version"] = serde_json::json!("0.3.0");
    fs::write(&artifact, document.to_string()).unwrap();
    let output = check(&project, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("0.3.0") && stderr.contains(env!("CARGO_PKG_VERSION")),
        "the skew must name both versions:\n{stderr}"
    );
    assert!(stderr.contains("cargo reconverge setup"), "{stderr}");

    fs::write(&artifact, original).unwrap();
}

/// Replacing the driver binary in place makes the next run re-lint.
///
/// The driver goes in as `RUSTC_WORKSPACE_WRAPPER`, and cargo does not
/// notice a same-path wrapper whose contents changed. So
/// `cargo install cargo-reconverge reconverge-driver` over a warm tree
/// re-analyzed nothing, and where the old driver's verdict was clean the
/// build stayed green on a crate that now has a finding.
#[test]
fn replacing_the_driver_in_place_forces_a_relint() {
    let project = prepared_copy(
        "m12-driver-swap",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lint-samples"),
    );
    // A private copy of the driver, so the swap is ours to make.
    let driver = project.join(format!("driver{}", env::consts::EXE_SUFFIX));
    fs::copy(ensure_driver(), &driver).unwrap();

    let run = |driver: &Path| {
        Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .args(["reconverge", "check"])
            .current_dir(&project)
            .env("RECONVERGE_DRIVER", driver)
            .output()
            .expect("failed to spawn cargo-reconverge")
    };
    // Cargo's `Checking` line, not the driver's own: on a fresh build cargo
    // *replays* the previous run's cached compiler output, so the driver's
    // stderr lines appear whether or not it was invoked. That replay is
    // exactly what made this bug look like a re-analysis.
    let relints = |output: &Output| compiled_lines(&output.stderr);

    run(&driver);
    assert_eq!(
        relints(&run(&driver)),
        0,
        "an unchanged tree with an unchanged driver re-lints nothing"
    );

    // The same path now holds a different build. Nothing else changed.
    let bytes = fs::read(&driver).unwrap();
    fs::remove_file(&driver).unwrap();
    fs::write(&driver, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = run(&driver);
    assert!(
        relints(&output) > 0,
        "a driver replaced in place must force a re-lint\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--reconverge-version` names the build that did the analysis, and every
/// other argv still reaches rustc untouched.
///
/// `reconverge-driver --version` prints rustc's version, by design — it is
/// a rustc-driver and cargo sends `-vV` and `--print` probes through the
/// wrapper. The defect was that no flag existed to ask it anything else,
/// so the half doing the analysis could not be identified at all while
/// both shipped consumers stamped their corpora with the CLI's version.
#[test]
fn the_driver_can_name_its_own_build() {
    let driver = ensure_driver();
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc --print sysroot");
    let lib = Path::new(String::from_utf8_lossy(&sysroot.stdout).trim())
        .join("lib")
        .display()
        .to_string();

    let run = |args: &[&str]| {
        Command::new(&driver)
            .args(args)
            .env("LD_LIBRARY_PATH", &lib)
            .env("DYLD_FALLBACK_LIBRARY_PATH", &lib)
            .output()
            .expect("failed to spawn reconverge-driver")
    };

    let output = run(&["--reconverge-version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("reconverge-driver {}", env!("CARGO_PKG_VERSION"))
    );

    // `--version` still belongs to rustc: cargo's own probes go through
    // this binary and must come back as rustc's answers.
    let output = run(&["--version"]);
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("rustc "),
        "argv must reach rustc untouched: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let output = run(&["-vV"]);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("host: "),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
