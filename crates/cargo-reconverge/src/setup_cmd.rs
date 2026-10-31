//! The `setup` subcommand: install the driver and TUI that match this CLI.
//!
//! `cargo install cargo-reconverge` delivers only this binary. The analysis
//! needs `reconverge-driver` — a rustc driver, which must be built by the
//! exact nightly it wraps — and the terminal views need `reconverge-tui`.
//! Rather than asking every user to reproduce that incantation, `setup`
//! runs it: the pinned toolchain with the components the driver links
//! against, then both companion crates at this CLI's **own version**, so
//! the three binaries cannot drift apart.
//!
//! Everything it does is printed before it runs, and every failure ends
//! with the manual commands, so `setup` is a convenience — never a
//! mystery.

use crate::args::ArgError;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The nightly the driver wraps, in lockstep with upstream cuda-oxide's
/// own pin. `rust-toolchain.toml` at the repository root is the source of
/// truth; a test below fails in-repo if the two ever drift.
pub const PINNED_TOOLCHAIN: &str = "nightly-2026-08-28";

pub struct SetupOptions {}

impl SetupOptions {
    pub fn parse(args: &[String]) -> Result<SetupOptions, ArgError> {
        match args.first().map(String::as_str) {
            None => Ok(SetupOptions {}),
            Some(flag) if ArgError::help(flag) => Err(ArgError::Help),
            Some(flag) if ArgError::version(flag) => Err(ArgError::Version),
            Some(other) => Err(ArgError::unknown(other)),
        }
    }
}

/// The commands `setup` runs, in order. A pure plan so tests can hold it
/// to the contract: pinned toolchain, driver components, version-matched
/// companions, locked dependencies, and the install root `check` looks in.
///
/// `--no-self-update`: by default `rustup toolchain install` also checks
/// for a newer rustup and replaces itself, which has nothing to do with
/// reconverge and fails on its own terms — observed as `failed to set
/// permissions for '…/rustup-init'` with the toolchain already present,
/// and as `rustup is not installed at …` under a `CARGO_HOME` that does not
/// hold rustup. Either one ended `setup` with exit 2.
fn plan(root: Option<&str>) -> [Vec<String>; 2] {
    let version = env!("CARGO_PKG_VERSION");
    let argv = |args: &[&str]| args.iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut install = argv(&[
        "rustup",
        "run",
        PINNED_TOOLCHAIN,
        "cargo",
        "install",
        "--locked",
    ]);
    if let Some(root) = root {
        install.extend(argv(&["--root", root]));
    }
    install.extend([
        format!("reconverge-driver@{version}"),
        format!("reconverge-tui@{version}"),
    ]);
    [
        argv(&[
            "rustup",
            "toolchain",
            "install",
            PINNED_TOOLCHAIN,
            "--profile",
            "minimal",
            "--component",
            "rustc-dev",
            "--component",
            "llvm-tools",
            "--no-self-update",
        ]),
        install,
    ]
}

/// The install root whose `bin` holds `exe`, which is where `check` and
/// `inspect` look first.
///
/// Without it `cargo install` went to cargo's default root, and a CLI
/// installed anywhere else — `cargo install --root ~/.local`,
/// `CARGO_INSTALL_ROOT`, a release archive — got a `setup` that succeeded
/// and a `check` that said "not found next to this binary; run `cargo
/// reconverge setup`", forever. For `~/.cargo/bin` this is the default
/// root anyway.
///
/// `None` when `exe` is not in a directory named `bin` (an unpacked
/// archive, `target/debug`): `cargo install` always writes to `<root>/bin`,
/// so no root puts the companions beside such a binary, and inventing one
/// would scatter a `bin/` somewhere unexpected. Cargo's default then
/// applies, and the companions are found on `PATH` instead.
fn install_root(exe: &Path) -> Option<&str> {
    let dir = exe.parent()?;
    if dir.file_name()? != "bin" {
        return None;
    }
    dir.parent()?.to_str()
}

/// The root `setup` passes to `cargo install`, or `None` for cargo's
/// default — with the reason, for the user, when it is `None`.
///
/// A `bin` this user cannot write is `None` too: a CLI in `/usr/local/bin`
/// or a distro's `/usr/bin` would otherwise get `cargo install --root
/// /usr/local`, which fails without sudo where cargo's default root
/// (`~/.cargo`) works and is on `PATH`.
fn choose_root(exe: &Path) -> (Option<&str>, Option<String>) {
    let dir = exe.parent().unwrap_or(exe);
    match install_root(exe) {
        Some(root) if writable(dir) => (Some(root), None),
        Some(_) => (
            None,
            Some(format!(
                "setup: {} is not writable by you, so the companions go to \
                 cargo's default install root; `check` finds them on PATH",
                dir.display()
            )),
        ),
        None => (
            None,
            Some(format!(
                "setup: {} is not a cargo `bin` directory, so the companions \
                 go to cargo's default install root; `check` finds them on PATH",
                dir.display()
            )),
        ),
    }
}

/// Whether this user can create a file in `dir`, asked by creating one.
/// The permission bits cannot say: they answer for the owner, not for
/// whoever is running, and they ignore ACLs and read-only mounts.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".cargo-reconverge-setup-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(e) => e.kind() == std::io::ErrorKind::AlreadyExists,
    }
}

/// A companion binary: `$<env_var>`, else the sibling of this executable,
/// else the first absolute `PATH` entry that holds it. Shared by `check`
/// (the driver) and `inspect`/`learn` (the TUI), which used to stop at the
/// sibling.
pub(crate) fn locate_companion(
    name: &str,
    env_var: &str,
    source_build: &str,
) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    find_companion(
        name,
        env_var,
        std::env::var_os(env_var),
        &exe,
        std::env::var_os("PATH"),
        source_build,
    )
}

fn find_companion(
    name: &str,
    env_var: &str,
    explicit: Option<OsString>,
    exe: &Path,
    path: Option<OsString>,
    source_build: &str,
) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("{env_var}={} does not exist", path.display()));
    }
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let sibling = exe
        .parent()
        .ok_or("cannot locate own directory")?
        .join(&file);
    if sibling.is_file() {
        return Ok(sibling);
    }
    // Absolute entries only. An empty or relative entry means "the current
    // directory" to a shell, and the current directory here is the project
    // being analyzed: a `reconverge-driver` committed to a repository would
    // otherwise run as the compiler wrapper for whoever checks it.
    let on_path = path
        .iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file());
    if let Some(found) = on_path {
        return Ok(found);
    }
    Err(format!(
        "{name} not found next to this binary ({}) or on PATH; run \
         `cargo reconverge setup` to install the matching version — in a \
         source checkout, `{source_build}` — or set {env_var}",
        sibling.display()
    ))
}

/// What to say when the pinned toolchain is missing but the driver is not.
///
/// `check` only gets this far with a driver in hand, so `setup` — which
/// rebuilds the driver and the TUI from source — is the wrong advice for
/// anyone who took the binaries from a release archive: all they lack is
/// the toolchain whose `librustc_driver` the driver loads, and the minimal
/// profile carries that.
pub(crate) fn missing_toolchain_hint(toolchain: &str) -> String {
    format!(
        "install it with `rustup toolchain install {toolchain} --profile \
         minimal --no-self-update`"
    )
}

/// Run the plan. Returns the CLI exit code.
pub fn run(_options: &SetupOptions) -> Result<u8, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    let (root, note) = choose_root(&exe);
    if let Some(note) = note {
        eprintln!("{note}");
    }
    for command in plan(root) {
        eprintln!("setup: running `{}`", command.join(" "));
        let status = Command::new(&command[0])
            .args(&command[1..])
            .status()
            .map_err(|e| {
                format!(
                    "cannot run `{}`: {e}\nsetup needs rustup (https://rustup.rs); \
                     the manual steps are:\n{}",
                    command[0],
                    manual_steps(root)
                )
            })?;
        if !status.success() {
            return Err(format!(
                "`{}` failed; the manual steps are:\n{}",
                command.join(" "),
                manual_steps(root)
            ));
        }
    }
    match root {
        Some(root) => eprintln!(
            "setup: done — reconverge-driver and reconverge-tui {} are \
             installed in {}",
            env!("CARGO_PKG_VERSION"),
            Path::new(root).join("bin").display()
        ),
        None => eprintln!(
            "setup: done — reconverge-driver and reconverge-tui {} are installed",
            env!("CARGO_PKG_VERSION")
        ),
    }
    Ok(0)
}

/// What to paste if `setup` cannot finish the job itself.
fn manual_steps(root: Option<&str>) -> String {
    plan(root)
        .iter()
        .map(|command| format!("  {}", command.join(" ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_pins_toolchain_version_and_lockfile() {
        let [toolchain, install] = plan(None);
        assert_eq!(toolchain[0], "rustup");
        assert!(toolchain.contains(&PINNED_TOOLCHAIN.to_string()));
        assert!(toolchain.contains(&"rustc-dev".to_string()));
        // rustup must not replace itself on the way (#203).
        assert!(toolchain.contains(&"--no-self-update".to_string()));
        assert!(!install.contains(&"--root".to_string()));

        let version = env!("CARGO_PKG_VERSION");
        assert!(install.contains(&format!("reconverge-driver@{version}")));
        assert!(install.contains(&format!("reconverge-tui@{version}")));
        assert!(install.contains(&"--locked".to_string()));
        // The companions are built by the pinned toolchain, not whatever
        // toolchain happens to be ambient.
        assert_eq!(&install[..3], &["rustup", "run", PINNED_TOOLCHAIN]);
    }

    /// Both READMEs must offer the command `setup` actually runs.
    ///
    /// The crates.io page — this crate's own README — offered an *unpinned*
    /// `cargo install reconverge-driver reconverge-tui` as "the manual
    /// equivalent" of a command that pins, one sentence after saying setup
    /// installs the companions "at this CLI's own version". crates.io is
    /// where the install happens; following that line installs whatever is
    /// newest against whatever CLI you pinned, which is the exact skew
    /// `setup` exists to prevent.
    ///
    /// The asymmetry is what makes this cheap: this crate's README ships
    /// inside the package, so that half is unconditional; only the
    /// repository README needs the source-checkout early return.
    #[test]
    fn both_readmes_offer_the_command_setup_runs() {
        let version = env!("CARGO_PKG_VERSION");
        // The pages carry `@VERSION` as a placeholder — the real number
        // belongs to whatever release the reader installed — so compare
        // against the plan with the same substitution.
        let expected: Vec<String> = plan(None)
            .iter()
            .map(|command| {
                command
                    .join(" ")
                    .replace(&format!("@{version}"), "@VERSION")
            })
            .collect();

        let own = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
        let text = std::fs::read_to_string(&own).expect("this crate's README ships in the package");
        for command in &expected {
            assert!(
                text.contains(command),
                "{}: the manual steps must match `setup`'s own plan; missing:\n  {command}",
                own.display()
            );
        }

        // Only meaningful in a source checkout.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
        let Ok(text) = std::fs::read_to_string(&root) else {
            return;
        };
        for command in &expected {
            assert!(
                text.contains(command),
                "README.md: the manual steps must match `setup`'s own plan; \
                 missing:\n  {command}"
            );
        }
    }

    #[test]
    fn the_embedded_pin_matches_rust_toolchain_toml() {
        // Only meaningful in a source checkout, which is exactly where the
        // pin could drift; the published package has no toolchain file.
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        assert!(
            text.contains(&format!("channel = \"{PINNED_TOOLCHAIN}\"")),
            "PINNED_TOOLCHAIN drifted from rust-toolchain.toml"
        );
    }

    #[test]
    fn setup_takes_no_arguments() {
        assert!(SetupOptions::parse(&[]).is_ok());
        assert!(SetupOptions::parse(&["--force".into()]).is_err());
    }

    #[test]
    fn failure_guidance_is_pasteable() {
        let steps = manual_steps(None);
        assert!(steps.contains("rustup toolchain install"));
        assert!(steps.contains("cargo install --locked"));
        // ...and names the root `setup` would have used, or pasting it
        // reproduces the loop the root exists to break.
        assert!(manual_steps(Some("/opt/rc")).contains("cargo install --locked --root /opt/rc "));
    }

    /// The companions go where `check` looks: the root whose `bin` holds
    /// this binary (#203). A CLI installed with `--root ~/.local` used to get
    /// its driver in `~/.cargo/bin`, and `check` then never found it.
    #[test]
    fn setup_installs_beside_its_own_binary() {
        let exe = Path::new("/home/u/.local/bin/cargo-reconverge");
        let root = install_root(exe);
        assert_eq!(root, Some("/home/u/.local"));
        let [_, install] = plan(root);
        let at = install.iter().position(|a| a == "--root").expect("--root");
        assert_eq!(install[at + 1], "/home/u/.local");
        assert_eq!(&install[..3], &["rustup", "run", PINNED_TOOLCHAIN]);

        // No `bin` directory, no root: `cargo install` always appends
        // `bin/`, so nothing would land beside an unpacked archive anyway.
        assert_eq!(
            install_root(Path::new("/tmp/rc-v0.7.0/cargo-reconverge")),
            None
        );
        assert_eq!(
            install_root(Path::new("target/debug/cargo-reconverge")),
            None
        );
    }

    /// A `bin` this user cannot write — `/usr/local/bin`, a distro's
    /// `/usr/bin` — falls back to cargo's default root rather than a
    /// `cargo install --root` that needs sudo.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_bin_falls_back_to_the_default_root() {
        use std::os::unix::fs::PermissionsExt;

        // Root writes through 0o555, so the fallback cannot be observed.
        let uid = Command::new("id").arg("-u").output().unwrap();
        if String::from_utf8_lossy(&uid.stdout).trim() == "0" {
            eprintln!("skipped: running as root");
            return;
        }
        let dir = scratch("readonly-bin");
        let exe = dir.join("bin/cargo-reconverge");
        touch(&exe);
        let root = dir.to_str().unwrap();

        let (chosen, note) = choose_root(&exe);
        assert_eq!(chosen, Some(root));
        assert_eq!(note, None);
        // The probe leaves nothing behind.
        assert_eq!(std::fs::read_dir(dir.join("bin")).unwrap().count(), 1);

        let bin = dir.join("bin");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();
        let (chosen, note) = choose_root(&exe);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(chosen, None);
        let note = note.expect("a note saying why");
        assert!(note.contains("is not writable by you"), "{note}");
        assert!(note.contains("finds them on PATH"), "{note}");
        assert!(!manual_steps(chosen).contains("--root"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reconverge-locate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    /// The sibling wins; `PATH` is the fallback for a CLI whose companions
    /// were installed elsewhere — a release archive unpacked anywhere, then
    /// `setup` into cargo's default root.
    #[test]
    fn a_companion_is_found_beside_the_cli_then_on_path() {
        let dir = scratch("path");
        let file = format!("reconverge-driver{}", std::env::consts::EXE_SUFFIX);
        let exe = dir.join("archive/cargo-reconverge");
        let on_path = dir.join("cargo/bin").join(&file);
        touch(&exe);
        touch(&on_path);
        let path = Some(std::env::join_paths([dir.join("empty"), dir.join("cargo/bin")]).unwrap());
        let find = |explicit: Option<OsString>, path: Option<OsString>| {
            find_companion(
                "reconverge-driver",
                "RECONVERGE_DRIVER",
                explicit,
                &exe,
                path,
                "b",
            )
        };

        assert_eq!(find(None, path.clone()), Ok(on_path.clone()));

        let sibling = dir.join("archive").join(&file);
        touch(&sibling);
        assert_eq!(find(None, path.clone()), Ok(sibling.clone()));

        let explicit = dir.join("explicit");
        touch(&explicit);
        assert_eq!(
            find(Some(explicit.clone().into()), path.clone()),
            Ok(explicit)
        );
        let err = find(Some(dir.join("nope").into()), path).unwrap_err();
        assert!(err.starts_with("RECONVERGE_DRIVER="), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A relative `PATH` entry resolves against the analyzed project, so a
    /// driver committed to a repository must not be picked up from it.
    /// Unix only, for the arithmetic: a Windows prefix is not one `..`.
    #[cfg(unix)]
    #[test]
    fn a_relative_path_entry_is_never_searched() {
        let dir = scratch("relative");
        let exe = dir.join("archive/cargo-reconverge");
        touch(&exe);
        let planted = dir.join("project");
        touch(&planted.join(format!("reconverge-driver{}", std::env::consts::EXE_SUFFIX)));
        // The same directory, spelled relative to the cwd: it resolves, so
        // only the absolute-entries rule can keep it out.
        let cwd = std::env::current_dir().unwrap();
        let mut relative = PathBuf::new();
        for _ in cwd.components().skip(1) {
            relative.push("..");
        }
        relative.extend(planted.components().skip(1));
        assert!(
            relative.is_relative() && relative.is_dir(),
            "{}",
            relative.display()
        );

        let path = Some(std::env::join_paths([relative]).unwrap());
        let err = find_companion(
            "reconverge-driver",
            "RECONVERGE_DRIVER",
            None,
            &exe,
            path,
            "b",
        )
        .unwrap_err();
        assert!(err.contains("or on PATH"), "{err}");
        assert!(err.contains("cargo reconverge setup"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_toolchain_is_one_rustup_command_not_a_rebuild() {
        let hint = missing_toolchain_hint(PINNED_TOOLCHAIN);
        assert!(
            hint.contains(&format!(
                "rustup toolchain install {PINNED_TOOLCHAIN} --profile minimal --no-self-update"
            )),
            "{hint}"
        );
        assert!(!hint.contains("setup"), "{hint}");
    }
}
