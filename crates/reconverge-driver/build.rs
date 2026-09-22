//! Turn a missing `rustc-dev` into one actionable line.
//!
//! The driver links rustc's own crates, which ship in the `rustc-dev`
//! component. A git checkout inherits reconverge's `rust-toolchain.toml`
//! and has it; the published crate carries no toolchain file, so
//! `cargo install reconverge-driver` on a fresh nightly meets four
//! `E0463`s naming `rustc_driver`, `rustc_interface`, `rustc_middle` and
//! `rustc_public` — four errors for one missing `rustup` argument, and
//! `rustc-dev` is not a component most people install for any other
//! reason.
//!
//! This reports only what it can establish. If the sysroot or the host tuple
//! cannot be resolved, or the directory cannot be read, the build proceeds
//! and rustc reports whatever it reports: a wrong guess here would block an
//! install that was going to work.
//!
//! # Where it looks, and why that is not `<sysroot>/lib`
//!
//! It looks for `librustc_driver-<hash>.rmeta` in
//! `<sysroot>/lib/rustlib/<host>/lib`, which is where the component installs
//! its crates on every platform.
//!
//! The first version of this looked in `<sysroot>/lib` for a file starting
//! with `librustc_driver` or `rustc_driver`, and was wrong in **both**
//! directions:
//!
//! - **False negative, everywhere.** `librustc_driver-<hash>.so` lives in
//!   `<sysroot>/lib` on any toolchain, because it is the shared library
//!   `rustc` itself links — it comes with the compiler, not with
//!   `rustc-dev`. Checked against a stable 1.98.0 toolchain with no
//!   `rustc-dev`: one match in `lib`, zero in `lib/rustlib/<host>/lib`. So
//!   the guard never fired for the case it exists for.
//! - **False positive on Windows.** windows-msvc puts
//!   `rustc_driver-<hash>.dll` under `<sysroot>/bin`, so `<sysroot>/lib`
//!   held nothing and a toolchain with `rustc-dev` correctly installed was
//!   reported as missing it — the build stopped, which is precisely what the
//!   paragraph above promises not to do. That is how this was found: the
//!   first Windows CI run, on a job whose `rustup toolchain install` had
//!   just reported eight components downloaded.
//!
//! `.rmeta` rather than the dynamic library: it is the metadata a
//! *dependent* crate needs, which is exactly what this build is about to be.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let Some(dir) = rustc_dev_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let present = entries
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_str().is_some_and(is_rustc_driver_rmeta));
    if present {
        return;
    }

    println!(
        "cargo::error=the `rustc-dev` component is missing from this \
         toolchain, so rustc's own crates cannot be linked. Install it \
         with: rustup component add --toolchain <toolchain> rustc-dev \
         llvm-tools    (looked in {})",
        dir.display()
    );
}

/// `librustc_driver-<hash>.rmeta`, and not `librustc_driver_impl-…`, whose
/// prefix would otherwise match.
fn is_rustc_driver_rmeta(name: &str) -> bool {
    name.strip_prefix("librustc_driver-")
        .is_some_and(|rest| rest.ends_with(".rmeta"))
}

/// The directory `rustc-dev` installs its crates into, or `None` when it
/// cannot be established — in which case this build script stays quiet.
fn rustc_dev_dir() -> Option<PathBuf> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let sysroot = print(&rustc, "sysroot")?;
    let host = print(&rustc, "host-tuple")?;
    let dir = PathBuf::from(sysroot)
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("lib");
    dir.is_dir().then_some(dir)
}

/// One `rustc --print <what>` line, trimmed, or `None` if rustc did not say.
fn print(rustc: &str, what: &str) -> Option<String> {
    let out = Command::new(rustc).args(["--print", what]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
