//! Every `confirmed` is earned: the verdict samples (`tests/verdict-samples`)
//! through `cargo reconverge check`, end to end.
//!
//! The 2026-10-31 review found four ways the witness promoted a finding
//! whose hang does not exist — a block shape the contract excludes (#190), a
//! barrier stepped over inside an un-inlined call (#191), signed values
//! compared unsigned (#192) — and one way a correct idiom was reported at
//! all (#194). Each lives in the sample crate as the kernel that showed it,
//! beside the control that proves the fix did not take the true positive
//! with it. A confidence tier here is the whole assertion: `confirmed` gates
//! CI, and the README promises it is never wrong.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// The driver the CLI shells out to, built on demand for an isolated
/// `cargo test -p cargo-reconverge` (as `check_cli` does).
fn ensure_driver() -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    let driver = cli
        .parent()
        .unwrap()
        .join(format!("reconverge-driver{}", env::consts::EXE_SUFFIX));
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

#[test]
fn every_confirmed_verdict_is_earned() {
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("v1-verdict-samples");
    // Refresh the sources, keep the target dir: dependency builds stay
    // cached while the samples always re-lint.
    let _ = fs::remove_dir_all(project.join("src"));
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/verdict-samples"),
        &project,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
        .args([
            "reconverge",
            "check",
            "--strict",
            "--message-format",
            "json",
        ])
        .current_dir(&project)
        .env("RECONVERGE_DRIVER", ensure_driver())
        .output()
        .expect("failed to spawn cargo-reconverge");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "the confirmed controls gate\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let findings: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON line"))
        .flat_map(|artifact| artifact["findings"].as_array().cloned().unwrap_or_default())
        .collect();
    let inventory: BTreeMap<(String, String), String> = findings
        .iter()
        .map(|f| {
            (
                (
                    f["code"].as_str().unwrap().to_string(),
                    f["kernel"].as_str().unwrap().to_string(),
                ),
                f["confidence"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(inventory.len(), findings.len(), "one finding per kernel");

    let expected: BTreeMap<(String, String), String> = [
        // #190: replayed in the declared shape, or not at all.
        ("RC001", "block_2d_x_guard_all_arrive", "warning"),
        ("RC001", "block_half_warp_all_arrive", "warning"),
        ("RC001", "block_2d_one_warp_all_arrive", "warning"),
        ("RC001", "block_2d_row_guard_hangs", "confirmed"),
        // #191: a barrier behind a call the inliner left in place.
        ("RC001", "recursion_all_arrive", "warning"),
        ("RC001", "depth_past_the_inliner", "warning"),
        ("RC001", "inlined_helper_hangs", "confirmed"),
        ("RC001", "sync_free_call_on_the_path", "confirmed"),
        // #192: signed order and division.
        ("RC001", "signed_all_lanes_arrive", "warning"),
        ("RC001", "signed_negative_literal", "warning"),
        ("RC001", "signed_real_split", "confirmed"),
        ("RC001", "signed_division_split", "confirmed"),
        // #194: `warp_uniform_guard`, `warp_uniform_division` and
        // `warp_uniform_shift` appear nowhere — that is the assertion.
        ("RC002", "warp_guard_without_contract", "warning"),
        ("RC002", "warp_guard_rows_split_a_warp", "warning"),
        ("RC002", "warp_number_overwritten_per_lane", "confirmed"),
    ]
    .into_iter()
    .map(|(code, kernel, confidence)| {
        (
            (code.to_string(), kernel.to_string()),
            confidence.to_string(),
        )
    })
    .collect();
    assert_eq!(inventory, expected, "stdout:\n{stdout}");

    // The tier is half the contract; the other half is that a declined
    // replay says why, and a result the replay did not reach is not one it
    // claims.
    let notes = |kernel: &str| -> String {
        findings
            .iter()
            .find(|f| f["kernel"] == kernel)
            .unwrap_or_else(|| panic!("no finding on {kernel}"))["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (kernel, needle) in [
        (
            "block_2d_x_guard_all_arrive",
            "replay: not attempted — the launch contract declares a (16, 16, 1) block \
             (256 threads, more than the 128 the replay runs)",
        ),
        (
            "block_half_warp_all_arrive",
            "replay: not attempted — the launch contract declares a (16, 1, 1) block \
             (16 threads, not a whole number of warps)",
        ),
        (
            "block_2d_row_guard_hangs",
            "witness: replayed with grid (1,1,1) x block (16,2,1), warp 0 — 16 of 32 lanes",
        ),
        (
            "recursion_all_arrive",
            "replay: not promoted — lanes reach a call to `countdown`",
        ),
        (
            "depth_past_the_inliner",
            "replay: not promoted — lanes reach a call to `level2`",
        ),
        (
            "signed_division_split",
            "lanes 0..31 at the failure point: ........ ........ WWWWWWWW WWWWWWWW",
        ),
        (
            "warp_guard_without_contract",
            "this kernel declares no `#[launch_contract]`",
        ),
        (
            "warp_guard_rows_split_a_warp",
            "declares a (48, 2, 1) block (3 warps, and a collective is replayed only \
             within a one-warp block)",
        ),
    ] {
        let notes = notes(kernel);
        assert!(
            notes.contains(needle),
            "{kernel}: no `{needle}` in\n{notes}"
        );
    }
    for kernel in [
        "block_2d_one_warp_all_arrive",
        "signed_all_lanes_arrive",
        "signed_negative_literal",
    ] {
        let notes = notes(kernel);
        assert!(
            !notes.contains("witness:") && !notes.contains("replay:"),
            "{kernel}: every lane arrives, so the replay has nothing to report:\n{notes}"
        );
    }

    // The 2-D replay's artifact carries the shape it ran, and its lane
    // count is the block's thread count rather than its x dimension.
    let witness_dir = project.join("target/reconverge");
    let two_d: Vec<reconverge_artifacts::witness::WitnessArtifact> = fs::read_dir(&witness_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("witness-verdict_samples-lib-block_2d_row_guard_hangs-")
        })
        .map(|p| serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap())
        .collect();
    assert_eq!(two_d.len(), 1, "one witness for the 2-D hang");
    assert_eq!(two_d[0].launch.block, [16, 2, 1]);
    assert_eq!(two_d[0].lanes, 32);
    assert_eq!(two_d[0].launch.warp, Some(0));
}
