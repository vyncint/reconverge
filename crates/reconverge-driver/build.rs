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
//! This reports only what it can establish. If the sysroot cannot be
//! resolved, or its `lib` directory cannot be read, the build proceeds and
//! rustc reports whatever it reports: a wrong guess here would block an
//! install that was going to work.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let Some(lib) = sysroot_lib() else { return };
    let Ok(entries) = std::fs::read_dir(&lib) else {
        return;
    };
    let present = entries.filter_map(Result::ok).any(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("librustc_driver") || n.starts_with("rustc_driver"))
    });
    if present {
        return;
    }

    println!(
        "cargo::error=the `rustc-dev` component is missing from this \
         toolchain, so rustc's own crates cannot be linked. Install it \
         with: rustup component add --toolchain <toolchain> rustc-dev \
         llvm-tools    (looked in {})",
        lib.display()
    );
}

/// The active toolchain's `lib` directory, or `None` when it cannot be
/// established — in which case this build script stays quiet.
fn sysroot_lib() -> Option<PathBuf> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sysroot = String::from_utf8(out.stdout).ok()?;
    let lib = PathBuf::from(sysroot.trim()).join("lib");
    lib.is_dir().then_some(lib)
}
