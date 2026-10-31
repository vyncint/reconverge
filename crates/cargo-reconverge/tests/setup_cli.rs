//! `setup` and `check` against a stand-in `rustup`, end to end through the
//! real binary (#203).
//!
//! Where `setup` installs is decided by where the running executable sits,
//! so the CLI is copied into a scratch install root and run from there; the
//! `rustup` it finds first on `PATH` is a script that records its argv. No
//! toolchain is installed and nothing is compiled.
//!
//! Unix only: the stand-in is a shell script.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

/// A fresh scratch directory under the target tmp dir.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Install an executable through a helper process. Written and closed by a
/// child that has exited, it can be exec'd at once: a file this process
/// wrote can still be open in a sibling test's fork, and exec then fails
/// with ETXTBSY.
fn install_executable(source: &Path, dest: &Path) {
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let status = Command::new("sh")
        .args(["-c", r#"cp "$1" "$2" && chmod 755 "$2""#, "sh"])
        .arg(source)
        .arg(dest)
        .status()
        .unwrap();
    assert!(status.success());
}

/// A `rustup` that appends its argv to `log` and then runs `body`.
fn fake_rustup(dir: &Path, log: &Path, body: &str) -> PathBuf {
    let script = dir.join("rustup.src");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}\n",
            log.display()
        ),
    )
    .unwrap();
    let bin = dir.join("fake-bin");
    install_executable(&script, &bin.join("rustup"));
    bin
}

/// A stand-in driver at `dest`: a script that exits 0. A script rather
/// than a copy of `true`, which is `/bin/true` on Linux and only
/// `/usr/bin/true` on macOS.
fn fake_driver(dir: &Path, dest: &Path) {
    let script = dir.join("driver.src");
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    install_executable(&script, dest);
}

/// The CLI, copied to `<root>/bin`, run with the stand-in first on `PATH`.
fn run_installed(root: &Path, fake_bin: &Path, cwd: &Path, args: &[&str]) -> Output {
    let cli = root.join("bin/cargo-reconverge");
    if !cli.exists() {
        install_executable(Path::new(env!("CARGO_BIN_EXE_cargo-reconverge")), &cli);
    }
    let path = env::join_paths(
        std::iter::once(fake_bin.to_path_buf())
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    Command::new(&cli)
        .arg("reconverge")
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        // The pin, whatever the outer cargo exported.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RECONVERGE_DRIVER")
        .output()
        .unwrap()
}

/// A CLI installed under any root gets its companions in that root: the
/// plan carries `--root`, and rustup is told not to replace itself.
#[test]
fn setup_installs_into_the_root_that_holds_the_cli() {
    let dir = scratch("setup-root");
    let log = dir.join("argv.log");
    let fake = fake_rustup(&dir, &log, "exit 0");
    let root = dir.join("local");

    let out = run_installed(&root, &fake, &dir, &["setup"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");

    let calls = fs::read_to_string(&log).unwrap();
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("toolchain install "), "{calls:?}");
    assert!(calls[0].ends_with(" --no-self-update"), "{calls:?}");
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        calls[1].ends_with(&format!(
            "cargo install --locked --root {} reconverge-driver@{version} reconverge-tui@{version}",
            root.display()
        )),
        "{calls:?}"
    );
    assert!(
        stderr.contains(&format!("installed in {}", root.join("bin").display())),
        "{stderr}"
    );
}

/// A failing step prints manual steps that reproduce the same root, or
/// pasting them recreates the loop.
#[test]
fn a_failed_setup_names_the_same_root_in_its_manual_steps() {
    let dir = scratch("setup-fail");
    let log = dir.join("argv.log");
    let fake = fake_rustup(&dir, &log, "exit 1");
    let root = dir.join("local");

    let out = run_installed(&root, &fake, &dir, &["setup"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "cargo install --locked --root {} ",
            root.display()
        )),
        "{stderr}"
    );
}

/// `check` with a driver in hand and the toolchain missing asks for the
/// toolchain alone. `setup` was the old advice, and it recompiles the driver
/// and the TUI from source, which somebody with a release archive does not
/// need.
#[test]
fn a_missing_toolchain_asks_for_rustup_not_setup() {
    let dir = scratch("check-no-toolchain");
    let log = dir.join("argv.log");
    let fake = fake_rustup(
        &dir,
        &log,
        "echo \"error: toolchain 'nightly-2026-08-28-x86_64-unknown-linux-gnu' is not installed\" >&2\nexit 1",
    );
    let root = dir.join("archive");
    // A driver beside the CLI, as in a release archive. It is never run:
    // the sysroot is resolved first.
    fake_driver(&dir, &root.join("bin/reconverge-driver"));
    let project = dir.join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "").unwrap();

    let out = run_installed(&root, &fake, &project, &["check"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "rustup toolchain install nightly-2026-08-28 --profile minimal --no-self-update"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("reconverge setup"), "{stderr}");
}

/// A driver on `PATH` is found when none sits beside the CLI — the shape of
/// an archive unpacked anywhere plus companions in cargo's default root.
#[test]
fn check_finds_a_driver_on_path() {
    let dir = scratch("check-path-driver");
    let log = dir.join("argv.log");
    let fake = fake_rustup(&dir, &log, "exit 1");
    fake_driver(&dir, &fake.join("reconverge-driver"));
    let root = dir.join("archive");
    let project = dir.join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "").unwrap();

    let out = run_installed(&root, &fake, &project, &["check"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // It got past the lookup: the next thing `check` does is resolve the
    // sysroot through rustup, which this stand-in refuses.
    assert!(
        !stderr.contains("not found next to this binary"),
        "{stderr}"
    );
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("rustc --print sysroot"),
        "{stderr}"
    );
}
