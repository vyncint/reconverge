//! What `check` builds, under which toolchain, and which artifacts it
//! answers for: the target-keyed half of the CLI contract.
//!
//! Every project here is written from scratch into the target tmp dir.
//! Where the question is not about a kernel, the crate has no dependencies
//! at all, so a run costs one `Checking` line rather than a cuda-oxide
//! build.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

const DEP: &str = "cuda-device = { git = \"https://github.com/NVlabs/cuda-oxide\", \
                   rev = \"b0f961df3af0ff140b3b006fa2b6750b71f43f62\" }";

const DIVERGENT: &str = "use cuda_device::{DisjointSlice, kernel, thread};\n\
     #[kernel]\npub fn NAME(mut out: DisjointSlice<u32>) {\n\
     \x20   let i = thread::index_1d();\n\
     \x20   if i.get() % 2 == 0 { thread::sync_threads(); }\n\
     \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n";

const CLEAN: &str = "use cuda_device::{DisjointSlice, kernel, thread};\n\
     #[kernel]\npub fn NAME(mut out: DisjointSlice<u32>) {\n\
     \x20   let i = thread::index_1d();\n\
     \x20   thread::sync_threads();\n\
     \x20   if let Some(e) = out.get_mut(i) { *e = 1; }\n}\n";

fn kernel(template: &str, name: &str) -> String {
    template.replace("NAME", name)
}

/// The driver binary the CLI shells out to; see `check_cli.rs`.
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

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    command
        .arg("reconverge")
        .env("RECONVERGE_DRIVER", ensure_driver());
    command
}

fn check(project: &Path, args: &[&str]) -> Output {
    cli()
        .arg("check")
        .args(args)
        .current_dir(project)
        .output()
        .expect("failed to spawn cargo-reconverge")
}

/// A project in the target tmp dir holding exactly `files`. The target
/// directory survives, so a dependency build is paid once per project.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.file_name() != "target" {
                let path = entry.path();
                let _ = fs::remove_dir_all(&path);
                let _ = fs::remove_file(&path);
            }
        }
    }
    for (path, text) in files {
        write(&dir, path, text);
    }
    dir
}

/// Write one file, backdated: a source written in the same mtime tick as
/// the previous run's fingerprint reads as unchanged to cargo, and every
/// edit in these tests has to be seen.
fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    file.set_modified(past).unwrap();
}

fn manifest(package: &str, extra: &str, deps: &str) -> String {
    format!(
        "[workspace]\n\n[package]\nname = \"{package}\"\nversion = \"0.0.0\"\n\
         edition = \"2024\"\npublish = false\n{extra}\n[dependencies]\n{deps}\n"
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn artifacts(project: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(project.join("target/reconverge"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Lines where cargo says it built `package`. The two words are matched
/// separately: with `CARGO_TERM_COLOR=always`, as CI sets it, an escape
/// sequence sits between `Checking` and the package name.
fn built(stderr: &str, package: &str) -> usize {
    stderr
        .lines()
        .filter(|l| (l.contains("Checking") || l.contains("Compiling")) && l.contains(package))
        .count()
}

/// The README's divergent barrier in a `[[bin]]` is a confirmed hang, and
/// it gates; deleting the bin takes its verdict with it.
///
/// Findings were keyed by *package* name while the driver names an artifact
/// after the *target*, so `findings-bench-bin.json` — a confirmed RC001,
/// witness and all — was silently skipped: `0 confirmed`, exit 0. And an
/// artifact was never removed, so a target deleted after a finding went on
/// failing every run, pointing at a file that no longer existed.
#[test]
fn a_bin_target_gates_and_a_deleted_one_stops_gating() {
    let dir = project(
        "t1-with-bin",
        &[
            ("Cargo.toml", &manifest("with-bin", "", DEP)),
            ("src/lib.rs", &kernel(CLEAN, "lib_clean")),
            (
                "src/bin/bench.rs",
                &(kernel(DIVERGENT, "bench_divergent") + "fn main() {}\n"),
            ),
        ],
    );
    let output = check(&dir, &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}\n{}", stderr(&output));
    assert!(
        out.contains("error[RC001]: kernel `bench_divergent`"),
        "{out}"
    );
    // The report prints paths the platform's way: `src\bin\bench.rs` on Windows.
    let bench = ["src", "bin", "bench.rs"].join(std::path::MAIN_SEPARATOR_STR);
    assert!(out.contains(&bench), "{out}");
    let json = check(&dir, &["--message-format", "json"]);
    let documents: Vec<serde_json::Value> = stdout(&json)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let crates: Vec<(&str, &str)> = documents
        .iter()
        .map(|d| (d["crate"].as_str().unwrap(), d["target"].as_str().unwrap()))
        .collect();
    assert_eq!(crates, [("bench", "bin"), ("with_bin", "lib")]);

    // `--cc` is an input cargo cannot see, so a change re-lints by hand —
    // the member only. Sweeping every fingerprint rebuilt `syn`, `quote`
    // and the rest of the tree to re-lint one crate.
    check(&dir, &["--cc", "8.6"]);
    let output = check(&dir, &["--cc", "9.0"]);
    let err = stderr(&output);
    assert!(built(&err, "with-bin") > 0, "a new --cc re-lints:\n{err}");
    for dependency in ["cuda-device", "cuda-macros", "syn", "proc-macro2"] {
        assert_eq!(
            built(&err, dependency),
            0,
            "a new --cc must not rebuild {dependency}:\n{err}"
        );
    }

    fs::remove_dir_all(dir.join("src/bin")).unwrap();
    let output = check(&dir, &["--strict"]);
    let out = stdout(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the deleted bin's verdict must not outlive it:\n{out}"
    );
    assert!(!out.contains("bench"), "{out}");
    assert_eq!(
        artifacts(&dir),
        ["findings-with_bin-lib.json", "unimap-with_bin-lib.json"],
        "the deleted target's findings, unimap and witness are pruned"
    );
}

/// A renamed `[lib]` and a bin-only package compile under names the
/// package does not have.
///
/// Both exited 2 on every run, after building twice — "produced no findings
/// artifact", about a target the driver had just analyzed — and a renamed
/// library with its old artifact still present was reported from the old,
/// clean document while the new one held a confirmed hang.
#[test]
fn a_renamed_lib_and_a_bin_only_package_gate_on_their_own_artifacts() {
    let lib = |name: &str| format!("[lib]\nname = \"{name}\"\n");
    let dir = project(
        "t2-renamed",
        &[
            ("Cargo.toml", &manifest("sample-kernels", &lib("kern"), DEP)),
            ("src/lib.rs", &kernel(CLEAN, "first_kernel")),
        ],
    );
    let output = check(&dir, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(artifacts(&dir).contains(&"findings-kern-lib.json".to_string()));

    // Renamed, and now divergent. The old name's clean document is still
    // on disk.
    write(
        &dir,
        "Cargo.toml",
        &manifest("sample-kernels", &lib("kern_device"), DEP),
    );
    write(&dir, "src/lib.rs", &kernel(DIVERGENT, "renamed_divergent"));
    let output = check(&dir, &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}\n{}", stderr(&output));
    assert!(
        out.contains("error[RC001]: kernel `renamed_divergent`"),
        "{out}"
    );
    assert!(
        !artifacts(&dir).iter().any(|n| n.contains("-kern-")),
        "the old name's artifacts are pruned: {:?}",
        artifacts(&dir)
    );

    // Bin-only, under a name of its own.
    write(
        &dir,
        "Cargo.toml",
        &manifest(
            "sample-kernels",
            "[[bin]]\nname = \"scale-offset-device\"\npath = \"src/device.rs\"\n",
            DEP,
        ),
    );
    fs::remove_file(dir.join("src/lib.rs")).unwrap();
    write(
        &dir,
        "src/device.rs",
        &(kernel(DIVERGENT, "bin_only_divergent") + "fn main() {}\n"),
    );
    let output = check(&dir, &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}\n{}", stderr(&output));
    assert!(
        out.contains("error[RC001]: kernel `bin_only_divergent`"),
        "{out}"
    );
    assert!(!out.contains("renamed_divergent"), "{out}");
}

/// A findings file cut short — a killed driver, a full disk — heals on the
/// next run instead of failing every run from then on.
#[test]
fn a_truncated_artifact_is_relinted_not_fatal() {
    let dir = project(
        "t3-truncated",
        &[
            ("Cargo.toml", &manifest("cut", "", "")),
            ("src/lib.rs", "pub fn f() {}\n"),
        ],
    );
    assert_eq!(check(&dir, &[]).status.code(), Some(0));
    let artifact = dir.join("target/reconverge/findings-cut-lib.json");
    let text = fs::read_to_string(&artifact).unwrap();
    fs::write(&artifact, &text[..text.len() / 2]).unwrap();

    let output = check(&dir, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the build is fresh, so only a forced re-lint rewrites the file:\n{}",
        stderr(&output)
    );
    let healed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&artifact).unwrap()).unwrap();
    assert_eq!(healed["crate"], "cut");
}

/// A crate with no `#[kernel]` passes, and says that nothing was analyzed;
/// `--require-kernels` makes it a failure. `witness` after it is an answer,
/// not an instruction to run the check that just ran.
#[test]
fn a_run_with_no_kernels_says_so() {
    let dir = project(
        "t4-plain",
        &[
            ("Cargo.toml", &manifest("plain", "", "")),
            (
                "src/lib.rs",
                "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
            ),
        ],
    );
    let _ = fs::remove_dir_all(dir.join("target/reconverge"));
    let output = cli()
        .arg("witness")
        .current_dir(&dir)
        .output()
        .expect("spawn witness");
    assert_eq!(output.status.code(), Some(2), "no check has run yet");
    assert!(
        stderr(&output).contains("run `cargo reconverge check` first"),
        "{}",
        stderr(&output)
    );

    let output = check(&dir, &[]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout(&output).trim_end(),
        "reconverge: no #[kernel] functions found in 1 crate \u{2014} nothing was analyzed"
    );
    // The Action turns that line into a `::warning::` by matching it, so
    // the two are one contract: rewording the summary must not silently
    // retire the warning.
    let action =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../action/action.yml"))
            .unwrap();
    assert!(
        action.contains("grep -q '^reconverge: no #\\[kernel\\] functions found'"),
        "action/action.yml must match the summary the CLI prints"
    );

    let output = check(&dir, &["--require-kernels"]);
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("--require-kernels: no #[kernel] function was analyzed"),
        "{}",
        stderr(&output)
    );

    let output = cli()
        .arg("witness")
        .current_dir(&dir)
        .output()
        .expect("spawn witness");
    assert_eq!(
        output.status.code(),
        Some(0),
        "a clean check leaves nothing to replay, which is not an error:\n{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("no witness to step through"),
        "{}",
        stderr(&output)
    );
}

/// The toolchain is the pin, whatever rustup's proxy exported.
///
/// rustup's `cargo` proxy sets `RUSTUP_TOOLCHAIN` for every subcommand it
/// runs, to whatever it resolved for the cwd, and `check` took that as an
/// explicit choice — so `cargo reconverge check`, typed as the README shows
/// it, loaded the driver against `stable` in any project without the pin
/// and exited 2. The suites never saw it: they launch the binary directly,
/// with the pin already in the environment.
#[test]
fn the_wrapped_build_uses_the_pin_whatever_the_outer_toolchain() {
    let dir = project(
        "t5-outer-toolchain",
        &[
            ("Cargo.toml", &manifest("outer", "", "")),
            ("src/lib.rs", "pub fn f() {}\n"),
        ],
    );
    let summary = "reconverge: no #[kernel] functions found";
    let stable_cargo = Command::new("rustup")
        .args(["which", "--toolchain", "stable", "cargo"])
        .output()
        .expect("rustup is how a user reaches `cargo reconverge`");
    assert!(
        stable_cargo.status.success(),
        "this needs the stable toolchain installed, as it is on every \
         GitHub runner:\n{}",
        stderr(&stable_cargo)
    );
    let stable_cargo = stdout(&stable_cargo).trim().to_string();

    // A user's shell, not this test's: `cargo test` puts the pinned
    // toolchain's libraries on the loader path and its `bin` on PATH, and
    // with those inherited the driver loads whatever toolchain it is
    // handed — which is how the suites never saw this.
    let outer = |command: &mut Command| {
        let inherited = env::var_os("PATH").unwrap();
        let path = env::split_paths(&inherited)
            .filter(|dir| !dir.components().any(|c| c.as_os_str() == "toolchains"));
        let cli_dir = Path::new(env!("CARGO_BIN_EXE_cargo-reconverge"))
            .parent()
            .unwrap()
            .to_path_buf();
        command
            .current_dir(&dir)
            .env(
                "PATH",
                env::join_paths(std::iter::once(cli_dir).chain(path)).unwrap(),
            )
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("DYLD_LIBRARY_PATH")
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .env("RECONVERGE_DRIVER", ensure_driver());
    };

    // Through rustup's own proxy, the way a user types it: `cargo` resolved
    // to `stable`, which finds `cargo-reconverge` on PATH.
    let mut command = Command::new("rustup");
    command.args(["run", "stable", "cargo", "reconverge", "check"]);
    outer(&mut command);
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "`cargo reconverge check` under a stable `cargo`:\n{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains(summary), "{}", stdout(&output));

    // What the proxy exports, set by hand; and empty, which used to count
    // as a choice too.
    for value in ["stable", ""] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
        command.args(["reconverge", "check"]);
        outer(&mut command);
        let output = command
            .env("RUSTUP_TOOLCHAIN", value)
            .env("CARGO", &stable_cargo)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "RUSTUP_TOOLCHAIN={value:?}:\n{}",
            stderr(&output)
        );
        assert!(stdout(&output).contains(summary), "{}", stdout(&output));
    }

    // The opt-out is a name nothing sets on the user's behalf, and when it
    // names a toolchain the driver cannot run under, the error says so —
    // not "fix the build errors", and not a setup that cannot help.
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-reconverge"));
    command.args(["reconverge", "check"]);
    outer(&mut command);
    let output = command
        .env("RECONVERGE_TOOLCHAIN", "stable")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("cannot start under the `stable` toolchain")
            && err.contains("RECONVERGE_TOOLCHAIN"),
        "{err}"
    );
}

/// A member whose build needs the CUDA toolkit fails in its build script;
/// the error says that, and `--package` / `--exclude` check around it.
///
/// Upstream's single-source layout links the host API, and `cuda-bindings`'
/// build script fails without the toolkit. The message blamed the
/// toolchain and recommended `setup`, which cannot help.
#[test]
fn a_build_script_failure_is_named_and_can_be_checked_around() {
    let dir = project(
        "t6-host-member",
        &[
            (
                "Cargo.toml",
                "[workspace]\nresolver = \"3\"\nmembers = [\"device\", \"host\"]\n",
            ),
            (
                "device/Cargo.toml",
                &manifest("device", "", "").replace("[workspace]\n", ""),
            ),
            ("device/src/lib.rs", "pub fn f() {}\n"),
            (
                "host/Cargo.toml",
                &manifest("host", "", "").replace("[workspace]\n", ""),
            ),
            ("host/src/lib.rs", "pub fn g() {}\n"),
            (
                "host/build.rs",
                "fn main() {\n    eprintln!(\"Neither CUDA_TOOLKIT_PATH nor CUDA_HOME is set\");\n    \
                 std::process::exit(1);\n}\n",
            ),
        ],
    );
    let output = check(&dir, &[]);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(
        err.contains("the build script of `host v0.0.0`") && err.contains("CUDA toolkit"),
        "{err}"
    );
    assert!(err.contains("--package"), "{err}");
    assert!(!err.contains("cargo reconverge setup"), "{err}");

    for args in [&["--package", "device"][..], &["--exclude", "host"][..]] {
        let output = check(&dir, args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}\n{}",
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("found in 1 crate"),
            "only the selected member is gated ({args:?}):\n{}",
            stdout(&output)
        );
    }

    let output = check(&dir, &["--package", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("`--package nope` names no member of this workspace"),
        "{}",
        stderr(&output)
    );
}

/// Outside any project, one `error:` and one reason.
#[test]
fn outside_a_project_the_error_is_said_once() {
    let dir = env::temp_dir().join(format!("reconverge-no-project-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let output = check(&dir, &[]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert_eq!(err.matches("error").count(), 1, "{err:?}");
    assert!(
        err.starts_with("error: cargo metadata failed: could not find `Cargo.toml`"),
        "{err:?}"
    );
    assert!(
        err.ends_with("directory\n"),
        "no trailing blank line: {err:?}"
    );
}
