//! The reconverge rustc driver binary.
//!
//! Invoked by cargo as `RUSTC_WORKSPACE_WRAPPER`: argv\[1\] is the path to
//! the real rustc, followed by ordinary rustc arguments. The driver runs the
//! full compilation through `rustc_driver` and, once analysis is done, walks
//! the crate through `rustc_public` (Stable MIR). Compilation then continues
//! normally, so a wrapped `cargo check` behaves exactly like an unwrapped
//! one. Without any `RECONVERGE_*` variable set the wrapper is a pure
//! passthrough.
//!
//! Environment interface (set by `cargo reconverge`, or by hand):
//! - `RECONVERGE_ARTIFACTS_OUT=<dir>` — run the lint passes over every
//!   detected kernel and write a `findings-<crate>.json` artifact
//!   (`findings.v1`).
//! - `RECONVERGE_CC=<X.Y>` — target compute capability for RC004 capacity
//!   context; must be in the dialect's table.
//! - `RECONVERGE_MIR_OUT=<dir>` — dump `<kernel>.mir` per detected kernel
//!   plus a `detection.txt` manifest.
//!
//! Kernel detection (strategies in order): the
//! post-expansion `#[kernel]` attribute does not survive — upstream's proc
//! macro consumes it and re-emits the function renamed under its reserved
//! naming contract with `#[unsafe(no_mangle)]` (verified against
//! cuda-macros at the pinned rev). That renamed item is the marker the
//! macro leaves AND the symbol name, so the marker-item and symbol-name
//! strategies coincide; both are implemented by
//! `reconverge_dialect_oxide::kernel_base_name`.
//!
//! Unstable-internals justification (CONTRIBUTING.md): analysis uses
//! `rustc_public` exclusively. `rustc_driver`, `rustc_interface`, and
//! `rustc_middle` appear only because `rustc_public::run!` needs them in
//! scope to instantiate the compiler — there is no stable driver entry
//! point yet.

#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;

mod adapt;
mod analysis;
mod emit;
mod uniformity;

use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use reconverge_dialect_oxide::cc;
use rustc_public::CompilerError;

/// Directory to write `findings-<crate>.json` artifacts into.
const ARTIFACTS_OUT_ENV: &str = "RECONVERGE_ARTIFACTS_OUT";
/// Target compute capability (`X.Y`) for RC004 capacity context.
const CC_ENV: &str = "RECONVERGE_CC";
/// Directory to write `<kernel>.mir` files and the detection manifest into.
const MIR_OUT_ENV: &str = "RECONVERGE_MIR_OUT";

/// The one flag this binary answers itself.
///
/// Everything else is forwarded to rustc untouched, by design — it is a
/// rustc-driver, and cargo sends `-vV` and `--print` probes through the
/// wrapper that must come back as rustc's own answers. The consequence was
/// that the half doing the analysis and writing the artifact could not be
/// identified at all: `reconverge-driver --version` prints rustc's version,
/// and no other flag existed to ask. Both shipped consumers stamp their
/// corpora with the *CLI's* version while the finding came from here.
///
/// Deliberately not `--version`: that one belongs to rustc.
const VERSION_FLAG: &str = "--reconverge-version";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    // Before the rustc handoff, and only for an exact match, so an argv
    // that merely contains the string still reaches rustc unchanged.
    if args.len() == 2 && args[1] == VERSION_FLAG {
        println!("reconverge-driver {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // RUSTC_WORKSPACE_WRAPPER contract: argv[1] is the real rustc. Drop it
    // so rustc_driver sees a normal argv (argv[0] stays as program name).
    if args.get(1).map(Path::new).and_then(Path::file_stem) == Some(OsStr::new("rustc")) {
        args.remove(1);
    }

    let mir_out = std::env::var_os(MIR_OUT_ENV).map(PathBuf::from);
    let artifacts_out = std::env::var_os(ARTIFACTS_OUT_ENV).map(PathBuf::from);
    let target_cc = match std::env::var(CC_ENV) {
        Err(_) => None,
        Ok(raw) => match cc::parse_compute_capability(&raw) {
            Ok(parsed) if cc::shared_memory_limits(parsed).is_some() => Some(parsed),
            Ok(_) => {
                eprintln!(
                    "reconverge-driver: {CC_ENV}={raw} is not in the compute-capability \
                     table; known: {}",
                    cc::known_compute_capabilities().join(", ")
                );
                return ExitCode::FAILURE;
            }
            Err(msg) => {
                eprintln!("reconverge-driver: {CC_ENV}: {msg}");
                return ExitCode::FAILURE;
            }
        },
    };

    let crate_types = crate_types_from_args(&args);
    let result = rustc_public::run!(&args, || run_analyses(
        mir_out.as_deref(),
        artifacts_out.as_deref(),
        &crate_types,
        target_cc
    ));
    match result {
        // `Skipped` covers invocations that never reach analysis, e.g. the
        // `-vV` / `--print` probes cargo sends through the wrapper.
        Ok(()) | Err(CompilerError::Skipped) => ExitCode::SUCCESS,
        Err(CompilerError::Interrupted(msg)) => {
            eprintln!("reconverge-driver: {msg}");
            ExitCode::FAILURE
        }
        // Compile errors: rustc has already reported them.
        Err(_) => ExitCode::FAILURE,
    }
}

/// The `--crate-type` values of this invocation, joined with `+`, for the
/// artifact filename (a package's lib and bin targets share a crate name).
fn crate_types_from_args(args: &[String]) -> String {
    let mut types = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let value = match arg.strip_prefix("--crate-type") {
            Some("") => iter.peek().map(|s| s.as_str()),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        };
        if let Some(value) = value {
            types.extend(value.split(',').map(str::to_string));
        }
    }
    if types.is_empty() {
        "unknown".to_string()
    } else {
        types.sort();
        types.dedup();
        types.join("+")
    }
}

/// Everything that runs inside the `rustc_public` session. Returns `Break`
/// only on a tool error, which aborts the wrapped compilation.
fn run_analyses(
    mir_out: Option<&Path>,
    artifacts_out: Option<&Path>,
    crate_types: &str,
    target_cc: Option<cc::ComputeCapability>,
) -> ControlFlow<String, ()> {
    if mir_out.is_none() && artifacts_out.is_none() {
        return ControlFlow::Continue(());
    }
    let kernels = analysis::detect_kernels();

    if let Some(dir) = mir_out {
        if let Err(err) = emit::dump_kernel_mir(dir, &kernels) {
            return ControlFlow::Break(format!(
                "failed to write MIR dump under `{}`: {err}",
                dir.display()
            ));
        }
        eprintln!(
            "reconverge-driver: detected {} {} in `{}`",
            kernels.len(),
            reconverge_artifacts::plural(kernels.len(), "kernel", "kernels"),
            rustc_public::local_crate().name
        );
    }

    if let Some(dir) = artifacts_out {
        let mut findings = analysis::run_lints(&kernels, target_cc);

        // The uniformity engine: RC001 findings and the unimap artifact.
        let models = adapt::build(&reconverge_dialect_oxide::simt::CudaOxide);
        let results = uniformity::analyze_kernels(&models);
        let mut witnesses = Vec::new();
        uniformity::rc001_divergent_barriers(&models, &results, &mut findings, &mut witnesses);
        uniformity::rc002_nonconvergent_warp_ops(&models, &results, &mut findings, &mut witnesses);
        // After every rule, so all five codes carry it — coverage is a
        // property of the analysis, not of one lint.
        uniformity::annotate_coverage(&models, &results, &mut findings);
        analysis::sort_findings(&mut findings);
        if let Err(err) = emit::write_witnesses(dir, crate_types, witnesses) {
            return ControlFlow::Break(format!(
                "failed to write witnesses under `{}`: {err}",
                dir.display()
            ));
        }

        match emit::write_findings(
            dir,
            crate_types,
            &findings,
            uniformity::run_coverage(&results),
        ) {
            Ok(path) => eprintln!(
                "reconverge-driver: {} {} in `{}` -> {}",
                findings.len(),
                reconverge_artifacts::plural(findings.len(), "finding", "findings"),
                rustc_public::local_crate().name,
                path.display()
            ),
            Err(err) => {
                return ControlFlow::Break(format!(
                    "failed to write findings under `{}`: {err}",
                    dir.display()
                ));
            }
        }
        let functions = uniformity::build_unimap(&models, &results);
        if let Err(err) = emit::write_unimap(dir, crate_types, functions) {
            return ControlFlow::Break(format!(
                "failed to write unimap under `{}`: {err}",
                dir.display()
            ));
        }
    }

    ControlFlow::Continue(())
}
