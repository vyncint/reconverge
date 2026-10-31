//! Turn a wrong toolchain, or a missing `rustc-dev`, into one actionable
//! line.
//!
//! The driver links rustc's own crates, so it must be built by the exact
//! nightly it wraps, with the `rustc-dev` component that ships those crates.
//! A git checkout inherits reconverge's `rust-toolchain.toml` and has both;
//! the published crate carries no toolchain file, so `cargo install
//! reconverge-driver` gets whatever toolchain is ambient, and before this
//! check:
//!
//! - **on stable**, a `rustc-dev` message with a `<toolchain>` placeholder,
//!   when the real problem is that stable cannot build `rustc_private` at
//!   all and the nightly to use was never named;
//! - **on another nightly**, a build that *succeeded* — and a driver that
//!   failed only when `check` ran it, with `librustc_driver-<hash>.so:
//!   cannot open shared object file`.
//!
//! So the pin is embedded here and compared with the `commit-hash` of
//! `rustc -vV`, which names one build exactly where a date or a version
//! number would not; the error carries a command the user can paste.
//! `tests/toolchain_pin.rs` holds both constants to `rust-toolchain.toml`
//! and to the rustc that file selects, so a pin bump that forgets this file
//! fails in CI rather than in a user's install.
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

/// The nightly this driver wraps: `rust-toolchain.toml`'s channel.
pub(crate) const PINNED_TOOLCHAIN: &str = "nightly-2026-08-28";
/// `commit-hash` of that nightly's `rustc -vV`.
pub(crate) const PINNED_COMMIT_HASH: &str = "e457a7b0d326d67b4322ef0d11bd715cfaeda48f";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let verbose_version = Command::new(&rustc)
        .arg("-vV")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
    if let Some(error) = verbose_version
        .as_deref()
        .and_then(|vv| wrong_toolchain(vv, &std::env::var("CARGO_PKG_VERSION").unwrap_or_default()))
    {
        println!("cargo::error={error}");
        return;
    }

    let Some(dir) = rustc_dev_dir(&rustc) else {
        return;
    };
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
         with: rustup component add --toolchain {PINNED_TOOLCHAIN} rustc-dev \
         llvm-tools    (looked in {})",
        dir.display()
    );
}

/// The error for a rustc that is not the pinned build, or `None` when it
/// is — or when `rustc -vV` does not say (a compiler built from a source
/// tarball reports `commit-hash: unknown`), since a guess here would block
/// an install that might work.
pub(crate) fn wrong_toolchain(verbose_version: &str, version: &str) -> Option<String> {
    let field = |name: &str| {
        verbose_version
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
            .map(str::trim)
    };
    let hash = field("commit-hash")?;
    if hash == PINNED_COMMIT_HASH || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let found = verbose_version
        .lines()
        .next()
        .unwrap_or("an unknown rustc")
        .trim();
    Some(format!(
        "reconverge-driver must be built by {PINNED_TOOLCHAIN}, the exact \
         nightly it wraps (a rustc driver links that rustc's own crates), \
         but this is `{found}`. Install it with: rustup toolchain install \
         {PINNED_TOOLCHAIN} --profile minimal --component rustc-dev \
         --component llvm-tools --no-self-update && rustup run \
         {PINNED_TOOLCHAIN} cargo install --locked reconverge-driver@{version}"
    ))
}

/// `librustc_driver-<hash>.rmeta`, and not `librustc_driver_impl-…`, whose
/// prefix would otherwise match.
fn is_rustc_driver_rmeta(name: &str) -> bool {
    name.strip_prefix("librustc_driver-")
        .is_some_and(|rest| rest.ends_with(".rmeta"))
}

/// The directory `rustc-dev` installs its crates into, or `None` when it
/// cannot be established — in which case this build script stays quiet.
fn rustc_dev_dir(rustc: &str) -> Option<PathBuf> {
    let sysroot = print(rustc, "sysroot")?;
    let host = print(rustc, "host-tuple")?;
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
