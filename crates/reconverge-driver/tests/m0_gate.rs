//! The MIR access gate.
//!
//! Acceptance: compiling a cuda-oxide kernel crate on the pinned nightly,
//! through the reconverge driver as `RUSTC_WORKSPACE_WRAPPER`, yields
//! host-side MIR for every `#[kernel]` body — no GPU, no vendor SDK.
//!
//! The sample crate is copied to the target tmp dir first, so the checked-in
//! sources are never touched and every run re-checks the sample with fresh
//! mtimes while dependency builds stay cached in a persistent target dir.

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

/// Prepend the pinned toolchain's lib dir to a library-path variable so the
/// driver binary can locate librustc_driver at runtime (Linux and macOS
/// spellings both set; the irrelevant one is harmless).
fn library_path(var: &str, sysroot_lib: &Path) -> String {
    match env::var(var) {
        Ok(existing) if !existing.is_empty() => {
            format!("{}:{existing}", sysroot_lib.display())
        }
        _ => sysroot_lib.display().to_string(),
    }
}

#[test]
fn m0_mir_access_gate() {
    let driver = Path::new(env!("CARGO_BIN_EXE_reconverge-driver"));
    let checked_in_sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sample-kernels");
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));

    let sample = tmp.join("m0-sample");
    let _ = fs::remove_dir_all(&sample);
    copy_dir(&checked_in_sample, &sample);

    let mir_out = tmp.join("m0-mir-out");
    let _ = fs::remove_dir_all(&mir_out);

    let sysroot = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .current_dir(&sample)
        .output()
        .expect("rustc --print sysroot");
    assert!(sysroot.status.success(), "could not determine sysroot");
    let sysroot_lib = PathBuf::from(String::from_utf8(sysroot.stdout).unwrap().trim()).join("lib");

    let cargo = env::var("CARGO").expect("cargo sets $CARGO for tests");
    let output = Command::new(cargo)
        .arg("check")
        .current_dir(&sample)
        .env("RUSTC_WORKSPACE_WRAPPER", driver)
        .env("RECONVERGE_MIR_OUT", &mir_out)
        // Persistent across runs: dependency builds stay cached, while the
        // freshly copied sample sources always re-check through the wrapper.
        .env("CARGO_TARGET_DIR", tmp.join("m0-sample-target"))
        .env(
            "LD_LIBRARY_PATH",
            library_path("LD_LIBRARY_PATH", &sysroot_lib),
        )
        .env(
            "DYLD_FALLBACK_LIBRARY_PATH",
            library_path("DYLD_FALLBACK_LIBRARY_PATH", &sysroot_lib),
        )
        .output()
        .expect("failed to spawn cargo check on the sample crate");
    assert!(
        output.status.success(),
        "cargo check through the driver failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let manifest = fs::read_to_string(mir_out.join("detection.txt"))
        .expect("driver must write the detection manifest");
    let lines: Vec<&str> = manifest.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected exactly the two sample kernels, got:\n{manifest}"
    );
    assert!(
        lines[0].starts_with("naming-contract\tdivergent_barrier\t"),
        "manifest line 0: {}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("naming-contract\tscale\t"),
        "manifest line 1: {}",
        lines[1]
    );

    let scale = fs::read_to_string(mir_out.join("scale.mir")).expect("scale.mir must exist");
    assert!(
        scale.contains("fn ") && !scale.trim().is_empty(),
        "scale.mir does not look like MIR:\n{scale}"
    );

    let barrier = fs::read_to_string(mir_out.join("divergent_barrier.mir"))
        .expect("divergent_barrier.mir must exist");
    assert!(
        barrier.contains("sync_threads"),
        "the divergent-barrier kernel's MIR should reference sync_threads:\n{barrier}"
    );
}
