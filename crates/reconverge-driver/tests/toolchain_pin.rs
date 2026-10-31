//! The build script's toolchain check, against the toolchain it pins.
//!
//! `build.rs` cannot be unit-tested in place — it runs before the crate
//! exists — so it is compiled here as a module, and its constants are held
//! to the two things they copy: `rust-toolchain.toml`, and the `rustc -vV`
//! of the compiler that file selects. A pin bump that forgets `build.rs`
//! would otherwise fail every user's install and none of this repository's
//! builds.

#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use build_script::{PINNED_COMMIT_HASH, PINNED_TOOLCHAIN, wrong_toolchain};
use std::path::Path;
use std::process::Command;

/// Recorded from `rustup run 1.98.0 rustc -vV` and `rustup run
/// nightly-2026-10-11 rustc -vV` — the two toolchains #204 was reproduced
/// on.
const STABLE: &str = "rustc 1.98.0 (88d9e12ae 2026-08-18)
binary: rustc
commit-hash: 88d9e12ae178fab0fb5cc050a94da85685d449ea
commit-date: 2026-08-18
host: x86_64-unknown-linux-gnu
release: 1.98.0
LLVM version: 22.1.8
";
const LATER_NIGHTLY: &str = "rustc 1.101.0-nightly (6f5da406b 2026-10-10)
binary: rustc
commit-hash: 6f5da406b56c2db6607202b63daee73aa8c61297
commit-date: 2026-10-10
host: x86_64-unknown-linux-gnu
release: 1.101.0-nightly
LLVM version: 23.1.3
";

fn workspace_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn the_pin_matches_rust_toolchain_toml() {
    let text = std::fs::read_to_string(workspace_root().join("rust-toolchain.toml")).unwrap();
    assert!(
        text.contains(&format!("channel = \"{PINNED_TOOLCHAIN}\"")),
        "build.rs pins {PINNED_TOOLCHAIN}, but rust-toolchain.toml says otherwise:\n{text}"
    );
}

/// The compiler building this test is the pinned one (the workspace's
/// toolchain file selects it), so it is the authority on the hash — and the
/// check must accept it, or no checkout could build the driver at all.
#[test]
fn the_pinned_rustc_passes_and_its_hash_is_the_embedded_one() {
    let out = Command::new("rustc")
        .arg("-vV")
        .current_dir(workspace_root())
        .output()
        .expect("rustc -vV");
    assert!(out.status.success());
    let verbose_version = String::from_utf8(out.stdout).unwrap();
    assert!(
        verbose_version.contains(&format!("commit-hash: {PINNED_COMMIT_HASH}\n")),
        "build.rs embeds {PINNED_COMMIT_HASH}, but {PINNED_TOOLCHAIN} reports:\n{verbose_version}"
    );
    assert_eq!(wrong_toolchain(&verbose_version, "0.7.0"), None);
}

/// Stable used to get a `rustc-dev` message with a `<toolchain>`
/// placeholder, and another nightly used to build a driver that could not
/// load. Both now stop the build with one pasteable line (#204).
#[test]
fn another_toolchain_gets_a_command_to_paste() {
    let error = wrong_toolchain(STABLE, "0.8.0").expect("stable is not the pin");
    assert!(
        error.contains("`rustc 1.98.0 (88d9e12ae 2026-08-18)`"),
        "{error}"
    );
    assert!(
        error.contains(&format!(
            "rustup run {PINNED_TOOLCHAIN} cargo install --locked reconverge-driver@0.8.0"
        )),
        "{error}"
    );
    assert!(
        error.contains(&format!("rustup toolchain install {PINNED_TOOLCHAIN} ")),
        "{error}"
    );
    assert!(!error.contains('<'), "no placeholders: {error}");
    // One `cargo::error=` line: a newline would end the directive.
    assert!(!error.contains('\n'), "{error}");

    // The case that used to build cleanly and fail only at `check` time.
    let error = wrong_toolchain(LATER_NIGHTLY, "0.8.0").expect("a later nightly is not the pin");
    assert!(
        error.contains("1.101.0-nightly (6f5da406b 2026-10-10)"),
        "{error}"
    );
}

/// Only what it can establish: a compiler that does not report its commit
/// is let through, and rustc itself reports whatever it reports.
#[test]
fn an_unknown_commit_is_not_a_mismatch() {
    let source_build = STABLE.replace(
        "commit-hash: 88d9e12ae178fab0fb5cc050a94da85685d449ea",
        "commit-hash: unknown",
    );
    assert_eq!(wrong_toolchain(&source_build, "0.8.0"), None);
    assert_eq!(wrong_toolchain("rustc 1.98.0\n", "0.8.0"), None);
}
