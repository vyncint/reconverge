//! The `check` subcommand: drive `cargo check` through the reconverge
//! driver, collect `findings.v1` artifacts, render, and gate the exit code.
//!
//! Artifacts land in `<target>/reconverge/` and compilation happens in the
//! dedicated `<target>/reconverge/build` directory, so wrapped builds never
//! disturb the user's ordinary build caches. Findings files persist across
//! runs: cargo's own freshness tracking guarantees a crate is recompiled —
//! and its findings rewritten — exactly when its inputs changed. The one
//! input cargo cannot see is `--cc`, so a change there wipes our build
//! fingerprints to force a re-lint.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use reconverge_artifacts::findings::FindingsArtifact;

use crate::args::ArgError;
use crate::review::{DEFAULT_BASELINE, Review};
use crate::{render, sarif};

pub struct CheckOptions {
    pub strict: bool,
    pub cc: Option<String>,
    pub message_format: MessageFormat,
    pub sarif_path: Option<PathBuf>,
    /// Explicit `--baseline <path>`; `None` means the default at the
    /// workspace root (missing is fine — an empty baseline).
    pub baseline: Option<PathBuf>,
    pub show_suppressed: bool,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum MessageFormat {
    Text,
    Json,
}

impl CheckOptions {
    pub fn parse(args: &[String]) -> Result<CheckOptions, ArgError> {
        let mut options = CheckOptions {
            strict: false,
            cc: None,
            message_format: MessageFormat::Text,
            sarif_path: None,
            baseline: None,
            show_suppressed: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (arg.as_str(), None),
            };
            let mut value = |name: &str| {
                inline_value
                    .clone()
                    .or_else(|| iter.next().cloned())
                    .ok_or_else(|| format!("`{name}` requires a value"))
            };
            match flag {
                "--strict" => options.strict = true,
                "--cc" => {
                    let raw = value("--cc")?;
                    let parsed = reconverge_dialect_oxide_cc_check(&raw)?;
                    options.cc = Some(parsed);
                }
                "--message-format" => {
                    options.message_format = match value("--message-format")?.as_str() {
                        "text" => MessageFormat::Text,
                        "json" => MessageFormat::Json,
                        other => {
                            return Err(ArgError::Value(format!(
                                "unknown message format `{other}` (expected `text` or `json`)"
                            )));
                        }
                    };
                }
                "--sarif" => options.sarif_path = Some(PathBuf::from(value("--sarif")?)),
                "--baseline" => options.baseline = Some(PathBuf::from(value("--baseline")?)),
                "--show-suppressed" => options.show_suppressed = true,
                other => return Err(ArgError::unknown(other)),
            }
        }
        Ok(options)
    }
}

/// Validate a `--cc` value against the dialect table, returning it verbatim.
fn reconverge_dialect_oxide_cc_check(raw: &str) -> Result<String, String> {
    use reconverge_dialect_oxide::cc;
    let parsed = cc::parse_compute_capability(raw)?;
    if cc::shared_memory_limits(parsed).is_none() {
        return Err(format!(
            "--cc {raw} is not in the compute-capability table; known: {}",
            cc::known_compute_capabilities().join(", ")
        ));
    }
    Ok(raw.to_string())
}

pub fn run(options: &CheckOptions) -> Result<Review, String> {
    let driver = locate_driver()?;
    let metadata = cargo_metadata()?;
    let reconverge_dir = metadata.target_directory.join("reconverge");
    let build_dir = reconverge_dir.join("build");
    fs::create_dir_all(&reconverge_dir)
        .map_err(|e| format!("cannot create {}: {e}", reconverge_dir.display()))?;

    refresh_cc_marker(&reconverge_dir, &build_dir, options.cc.as_deref())?;

    run_wrapped_check(&driver, &reconverge_dir, &build_dir, options)?;
    let mut artifacts = collect_artifacts(&reconverge_dir, &metadata.member_crates)?;

    // Self-heal: if a workspace member has no findings artifact (for
    // example the artifacts directory was deleted while the build stayed
    // fresh), force one re-lint by dropping our build fingerprints.
    let missing: Vec<&String> = metadata
        .member_crates
        .iter()
        .filter(|krate| !artifacts.iter().any(|a| &&a.krate == krate))
        .collect();
    if !missing.is_empty() {
        drop_build_fingerprints(&build_dir);
        run_wrapped_check(&driver, &reconverge_dir, &build_dir, options)?;
        artifacts = collect_artifacts(&reconverge_dir, &metadata.member_crates)?;
    }

    let review = Review::load(
        artifacts,
        options
            .baseline
            .clone()
            .unwrap_or_else(|| metadata.workspace_root.join(DEFAULT_BASELINE)),
        options.baseline.is_some(),
    )?;

    match options.message_format {
        MessageFormat::Text => {
            render::render_text(&review, options.strict, options.show_suppressed)
        }
        MessageFormat::Json => {
            // The analysis record, unfiltered: the baseline is a review
            // decision, and machine consumers get the raw findings plus the
            // baseline file itself rather than a pre-filtered mixture.
            for artifact in &review.artifacts {
                println!(
                    "{}",
                    serde_json::to_string(artifact).map_err(|e| e.to_string())?
                );
            }
        }
    }
    for entry in review.stale_entries() {
        eprintln!(
            "reconverge: baseline entry `{}` no longer matches any finding; \
             delete it from {} or rerun `cargo reconverge triage`",
            entry.label(),
            review.baseline_path.display()
        );
    }
    if let Some(path) = &options.sarif_path {
        sarif::write_report(path, &review)
            .map_err(|e| format!("cannot write SARIF to {}: {e}", path.display()))?;
    }

    Ok(review)
}

/// The reconverge driver binary: `$RECONVERGE_DRIVER`, or the sibling of
/// this executable.
fn locate_driver() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("RECONVERGE_DRIVER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "RECONVERGE_DRIVER={} does not exist",
            path.display()
        ));
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    let sibling = exe
        .parent()
        .ok_or("cannot locate own directory")?
        .join(format!("reconverge-driver{}", std::env::consts::EXE_SUFFIX));
    if sibling.is_file() {
        return Ok(sibling);
    }
    Err(format!(
        "reconverge-driver not found next to this binary ({}); run \
         `cargo reconverge setup` to install the matching version — in a \
         source checkout, `cargo build -p reconverge-driver` — or set \
         RECONVERGE_DRIVER",
        sibling.display()
    ))
}

pub(crate) struct Metadata {
    pub(crate) target_directory: PathBuf,
    /// Workspace root: where the default baseline lives.
    pub(crate) workspace_root: PathBuf,
    /// Workspace member *crate* names (package names with `-` mapped to `_`).
    pub(crate) member_crates: Vec<String>,
}

pub(crate) fn cargo_metadata() -> Result<Metadata, String> {
    let output = Command::new(cargo_bin())
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("bad cargo metadata: {e}"))?;
    let target_directory = json["target_directory"]
        .as_str()
        .ok_or("cargo metadata lacks target_directory")?
        .into();
    let workspace_root = json["workspace_root"]
        .as_str()
        .ok_or("cargo metadata lacks workspace_root")?
        .into();
    let member_crates = json["packages"]
        .as_array()
        .ok_or("cargo metadata lacks packages")?
        .iter()
        .filter_map(|p| p["name"].as_str())
        .map(|name| name.replace('-', "_"))
        .collect();
    Ok(Metadata {
        target_directory,
        workspace_root,
        member_crates,
    })
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Re-linting is keyed to `--cc`: cargo cannot see that input, so when it
/// changes we drop our (dedicated) build dir's fingerprints.
fn refresh_cc_marker(
    reconverge_dir: &Path,
    build_dir: &Path,
    cc: Option<&str>,
) -> Result<(), String> {
    let marker = reconverge_dir.join("cc-marker");
    let current = cc.unwrap_or("(none)");
    let previous = fs::read_to_string(&marker).ok();
    if previous.as_deref() != Some(current) {
        drop_build_fingerprints(build_dir);
        fs::write(&marker, current).map_err(|e| format!("cannot write cc marker: {e}"))?;
    }
    Ok(())
}

/// Drop cargo's freshness fingerprints in our dedicated build directory,
/// forcing the next wrapped `cargo check` to re-lint everything.
///
/// Cargo keeps fingerprints under each *profile* directory
/// (`<build_dir>/debug/.fingerprint`), so sweep every immediate
/// subdirectory rather than assuming one profile name.
fn drop_build_fingerprints(build_dir: &Path) {
    let _ = fs::remove_dir_all(build_dir.join(".fingerprint"));
    let Ok(entries) = fs::read_dir(build_dir) else {
        return; // nothing built yet: nothing to invalidate
    };
    for entry in entries.flatten() {
        let _ = fs::remove_dir_all(entry.path().join(".fingerprint"));
    }
}

fn run_wrapped_check(
    driver: &Path,
    reconverge_dir: &Path,
    build_dir: &Path,
    options: &CheckOptions,
) -> Result<(), String> {
    // The driver is a rustc-driver binary: it only runs against the exact
    // toolchain it was built by (the pin `cargo reconverge setup` installs,
    // which is also the repo's own `rust-toolchain.toml`). The analyzed
    // project has no reason to carry that pin, so export it — exactly what
    // action/action.yml does in CI — and resolve the driver's dylib path
    // from that toolchain, never the ambient one. An explicit
    // RUSTUP_TOOLCHAIN in the environment still wins, for drivers built
    // against a deliberately different toolchain (RECONVERGE_DRIVER).
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN")
        .unwrap_or_else(|_| crate::setup_cmd::PINNED_TOOLCHAIN.to_string());
    let sysroot_lib = sysroot_lib_dir(&toolchain)?;
    let mut command = Command::new(cargo_bin());
    command
        .arg("check")
        .env("RUSTC_WORKSPACE_WRAPPER", driver)
        .env("RECONVERGE_ARTIFACTS_OUT", reconverge_dir)
        .env("CARGO_TARGET_DIR", build_dir)
        .env("RUSTUP_TOOLCHAIN", &toolchain)
        .env(
            "LD_LIBRARY_PATH",
            prepend_path("LD_LIBRARY_PATH", &sysroot_lib),
        )
        .env(
            "DYLD_FALLBACK_LIBRARY_PATH",
            prepend_path("DYLD_FALLBACK_LIBRARY_PATH", &sysroot_lib),
        );
    if let Some(cc) = &options.cc {
        command.env("RECONVERGE_CC", cc);
    }
    // stdout stays ours (JSON mode must remain clean); cargo talks on stderr.
    let status = command
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("cannot run cargo check: {e}"))?;
    if !status.success() {
        return Err(format!(
            "`cargo check` under the reconverge driver failed (see the errors \
             above). If rustc reported real build errors, fix those and rerun; \
             if the driver itself failed to start (for example `error while \
             loading shared libraries`), the toolchain does not match the \
             driver — run `cargo reconverge setup` to install {} and the \
             matching binaries",
            crate::setup_cmd::PINNED_TOOLCHAIN
        ));
    }
    Ok(())
}

/// Library dir of the given toolchain, for the driver's rustc dylibs.
fn sysroot_lib_dir(toolchain: &str) -> Result<PathBuf, String> {
    // `rustup run` resolves the pinned toolchain even when the `rustc` on
    // PATH is not rustup's shim (a distro or Homebrew rust would otherwise
    // shadow it). Fall back to plain `rustc` — with the toolchain exported
    // for the shim case — only when rustup itself is absent.
    let output = match Command::new("rustup")
        .args(["run", toolchain, "rustc", "--print", "sysroot"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return Err(format!(
                "cannot resolve the {toolchain} sysroot: {}\nrun `cargo \
                 reconverge setup` to install it",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(_) => {
            let output = Command::new("rustc")
                .args(["--print", "sysroot"])
                .env("RUSTUP_TOOLCHAIN", toolchain)
                .output()
                .map_err(|e| format!("cannot run rustc --print sysroot: {e}"))?;
            if !output.status.success() {
                return Err("rustc --print sysroot failed".to_string());
            }
            output
        }
    };
    let sysroot = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    Ok(PathBuf::from(sysroot.trim()).join("lib"))
}

fn prepend_path(var: &str, dir: &Path) -> String {
    match std::env::var(var) {
        Ok(existing) if !existing.is_empty() => format!("{}:{existing}", dir.display()),
        _ => dir.display().to_string(),
    }
}

/// Read every findings artifact belonging to a current workspace member,
/// sorted by crate name.
///
/// Only the driver's current naming scheme is accepted —
/// `findings-<crate>-<crate types>.json`, with the filename's crate part
/// matching the artifact's own `crate` field. Anything else in the
/// directory (artifacts of crates no longer in the workspace, files from
/// older naming schemes) is ignored, never merged.
fn collect_artifacts(
    reconverge_dir: &Path,
    member_crates: &[String],
) -> Result<Vec<FindingsArtifact>, String> {
    let members: BTreeSet<&String> = member_crates.iter().collect();
    let mut artifacts = Vec::new();
    let entries = fs::read_dir(reconverge_dir)
        .map_err(|e| format!("cannot read {}: {e}", reconverge_dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Crate names cannot contain `-`, so the *first* `-` separates the
        // crate from the crate-types suffix unambiguously. (Not the last:
        // the suffix itself can contain one — `proc-macro`.)
        let Some(stem) = name
            .strip_prefix("findings-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        let Some((krate_in_name, _types)) = stem.split_once('-') else {
            continue;
        };
        if !members.contains(&krate_in_name.to_string()) {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let artifact: FindingsArtifact = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a findings.v1 artifact: {e}", path.display()))?;
        if artifact.krate == krate_in_name {
            artifacts.push(artifact);
        }
    }
    artifacts.sort_by(|a, b| a.krate.cmp(&b.krate));
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty scratch directory. Every caller passes a unique tag:
    /// tests run in parallel and each wipes its own directory on entry.
    fn empty_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reconverge-check-unit-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fingerprints_are_dropped_per_profile() {
        // Cargo keeps fingerprints under the *profile* directory
        // (`build/debug/.fingerprint`), not at the build root; deleting only
        // the root path silently invalidates nothing (the --cc-is-ignored
        // bug). The sweep must reach one level down.
        let build_dir = empty_dir("fingerprints");
        let profile_fp = build_dir.join("debug/.fingerprint/some-crate");
        let root_fp = build_dir.join(".fingerprint/other");
        fs::create_dir_all(&profile_fp).unwrap();
        fs::create_dir_all(&root_fp).unwrap();

        drop_build_fingerprints(&build_dir);

        assert!(!build_dir.join("debug/.fingerprint").exists());
        assert!(!build_dir.join(".fingerprint").exists());
        // The rest of the profile directory survives: only freshness is
        // invalidated, compiled deps stay cached.
        assert!(build_dir.join("debug").exists());
    }

    #[test]
    fn dropping_fingerprints_before_any_build_is_fine() {
        let build_dir = empty_dir("fingerprints-missing").join("never-built");
        drop_build_fingerprints(&build_dir); // must not panic or error
    }

    #[test]
    fn artifacts_of_proc_macro_members_are_collected() {
        // The crate-types suffix can itself contain `-` (`proc-macro`), so
        // the filename must split on the first hyphen, not the last.
        let dir = empty_dir("collect-proc-macro");
        let artifact =
            reconverge_artifacts::findings::FindingsArtifact::new("helper_macros", vec![]);
        fs::write(
            dir.join("findings-helper_macros-proc-macro.json"),
            serde_json::to_string(&artifact).unwrap(),
        )
        .unwrap();

        let collected = collect_artifacts(&dir, &["helper_macros".to_string()]).unwrap();
        assert_eq!(collected.len(), 1, "proc-macro artifact must be collected");
        assert_eq!(collected[0].krate, "helper_macros");
    }
}
