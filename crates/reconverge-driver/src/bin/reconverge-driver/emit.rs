//! Artifact and MIR-dump writing, plus span conversion.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use reconverge_artifacts::findings::{Finding, FindingsArtifact, RunCoverage, SourceSpan};
use reconverge_artifacts::unimap::{Function, UnimapArtifact};
use reconverge_artifacts::witness::WitnessArtifact;
use rustc_public::ty::Span;

use crate::analysis::Kernel;

/// Convert a compiler span into the artifact form (1-based, end-inclusive).
pub fn source_span(span: Span) -> SourceSpan {
    let lines = span.get_lines();
    SourceSpan {
        file: span.get_filename(),
        line_start: lines.start_line,
        column_start: lines.start_col,
        line_end: lines.end_line,
        column_end: lines.end_col,
    }
}

/// Write the `findings.v1` artifact for the current compiled target.
///
/// The filename carries the crate types (`lib`, `bin`, …) so a package with
/// several targets sharing one crate name never overwrites its own
/// artifacts — and since 0.5.0 so does the document, in `target`. Telling
/// consumers to key on `crate` was the advice that broke them: a package
/// with a lib and a bin writes two documents under one crate name, and a
/// dictionary keyed on `crate` keeps whichever `read_dir` handed back last,
/// which in the ordinary GPU-project shape can be the empty one.
pub fn write_findings(
    dir: &Path,
    crate_types: &str,
    findings: &[Finding],
    coverage: RunCoverage,
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let krate = rustc_public::local_crate().name;
    let artifact = FindingsArtifact::new(krate.clone(), findings.to_vec())
        .for_target(crate_types)
        .with_coverage(coverage);
    let path = dir.join(format!("findings-{krate}-{crate_types}.json"));
    let mut file = BufWriter::new(File::create(&path)?);
    serde_json::to_writer_pretty(&mut file, &artifact)?;
    writeln!(file)?;
    file.flush()?;
    Ok(path)
}

/// Write the `unimap.v1` artifact for the current crate (same naming
/// scheme as findings).
pub fn write_unimap(
    dir: &Path,
    crate_types: &str,
    functions: Vec<Function>,
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let krate = rustc_public::local_crate().name;
    let artifact = UnimapArtifact::new(krate.clone(), functions);
    let path = dir.join(format!("unimap-{krate}-{crate_types}.json"));
    let mut file = BufWriter::new(File::create(&path)?);
    serde_json::to_writer_pretty(&mut file, &artifact)?;
    writeln!(file)?;
    file.flush()?;
    Ok(path)
}

/// Write one `witness.v1` artifact per successful replay (same naming
/// scheme as findings, suffixed with kernel, code, and an index so several
/// witnesses per kernel never collide).
pub fn write_witnesses(
    dir: &Path,
    crate_types: &str,
    mut witnesses: Vec<WitnessArtifact>,
) -> std::io::Result<usize> {
    fs::create_dir_all(dir)?;
    let krate = rustc_public::local_crate().name;
    // Above the empty-set return, so the run that *should* clean up is the
    // one that does. A witness was only ever created, never removed: fix
    // the kernel, re-run, get `0 confirmed`, open the debugger and watch it
    // replay `verdict: undefined behavior` on the barrier you just hoisted.
    // Downstream it was worse than misleading — simt-diff reads this
    // directory to decide promotion, and a stale witness turns "declined"
    // into "promoted", silently, in the direction that hides a regression.
    //
    // Scoped by crate name *and* crate types, so a lib and a bin in one
    // package do not delete each other's witnesses.
    prune_witnesses(dir, &krate, crate_types)?;
    if witnesses.is_empty() {
        return Ok(0);
    }
    let count = witnesses.len();
    for (i, artifact) in witnesses.iter_mut().enumerate() {
        artifact.krate = krate.clone();
        let code = artifact
            .finding
            .as_ref()
            .map_or("RC000", |f| f.code.as_str());
        let path = dir.join(format!(
            "witness-{krate}-{crate_types}-{}-{code}-{i}.json",
            artifact.kernel
        ));
        let mut file = BufWriter::new(File::create(&path)?);
        serde_json::to_writer_pretty(&mut file, &artifact)?;
        writeln!(file)?;
        file.flush()?;
    }
    Ok(count)
}

/// Remove this target's witnesses from a previous run.
///
/// Only `witness-<krate>-<crate_types>-*.json`: everything else in the
/// directory belongs to another target, another crate, or another tool.
fn prune_witnesses(dir: &Path, krate: &str, crate_types: &str) -> std::io::Result<()> {
    let prefix = format!("witness-{krate}-{crate_types}-");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // Nothing written yet: nothing to prune.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".json") {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Dump `<kernel>.mir` per kernel plus the `detection.txt` manifest
/// (`<strategy>\t<kernel>\t<item path>` per line, sorted).
pub fn dump_kernel_mir(dir: &Path, kernels: &[Kernel]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut manifest = Vec::new();

    for kernel in kernels {
        let mut file = BufWriter::new(File::create(dir.join(format!("{}.mir", kernel.name)))?);
        writeln!(file, "// kernel `{}`", kernel.name)?;
        writeln!(file, "// item: {}", kernel.path)?;
        kernel.item.emit_mir(&mut file)?;
        file.flush()?;

        // Detection strategy actually in effect; see main.rs for why the
        // attribute strategy cannot fire and marker == symbol here.
        manifest.push(format!("naming-contract\t{}\t{}", kernel.name, kernel.path));
    }

    manifest.sort();
    let mut file = BufWriter::new(File::create(dir.join("detection.txt"))?);
    for line in &manifest {
        writeln!(file, "{line}")?;
    }
    file.flush()
}
