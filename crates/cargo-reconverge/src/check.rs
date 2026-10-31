//! The `check` subcommand: drive `cargo check` through the reconverge
//! driver, collect `findings.v1` artifacts, render, and gate the exit code.
//!
//! Artifacts land in `<target>/reconverge/` and compilation happens in the
//! dedicated `<target>/reconverge/build` directory, so wrapped builds never
//! disturb the user's ordinary build caches. Findings files persist across
//! runs: cargo's own freshness tracking guarantees a crate is recompiled —
//! and its findings rewritten — exactly when its inputs changed.
//!
//! That holds only if two things are true, and until 0.5.0 neither was.
//!
//! The build has to cover every member the report gates on. It was a bare
//! `cargo check`, so cargo's own package selection applied: run the gate
//! from inside a member directory, or put a `default-members` line in the
//! workspace, and the sibling was never re-linted while its previous,
//! clean artifact was still printed and gated on as this run's answer. It
//! is `--workspace` now (or exactly the `--package` selection), and a
//! target that still produces nothing is named rather than passed over.
//!
//! And the inputs cargo cannot see have to be tracked by hand. `--cc` was;
//! the driver binary was not, so `cargo install`-ing a driver over the same
//! path re-linted nothing and CI gated on whatever the old one concluded.
//! Both now live in the `cc-marker`.
//!
//! Since 0.8.0 the unit is the *target*, not the package. The driver names
//! an artifact after the crate it compiled — a `[[bin]]`'s, a renamed
//! `[lib]`'s — and the set this run answers for is read off cargo's own
//! build messages rather than predicted from package names.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use reconverge_artifacts::findings::FindingsArtifact;

use crate::args::{self, ArgError};
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
    /// `--package`: check only these workspace members.
    pub packages: Vec<String>,
    /// `--exclude`: check every workspace member but these.
    pub exclude: Vec<String>,
    /// `--require-kernels`: a run that analyzed no `#[kernel]` is a tool
    /// error rather than a clean pass.
    pub require_kernels: bool,
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
            packages: Vec::new(),
            exclude: Vec::new(),
            require_kernels: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = args::split_flag(arg);
            let mut value =
                |name: &str| args::require_value(name, inline_value, || iter.next().cloned());
            match flag {
                "--strict" => {
                    args::reject_value("--strict", inline_value)?;
                    options.strict = true;
                }
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
                "--show-suppressed" => {
                    args::reject_value("--show-suppressed", inline_value)?;
                    options.show_suppressed = true;
                }
                "--package" | "-p" => options.packages.push(value("--package")?),
                "--exclude" => options.exclude.push(value("--exclude")?),
                "--require-kernels" => {
                    args::reject_value("--require-kernels", inline_value)?;
                    options.require_kernels = true;
                }
                // Below the value-taking flags, so `--baseline --help` is
                // still the missing value it was.
                flag if ArgError::help(flag) => return Err(ArgError::Help),
                flag if ArgError::version(flag) => return Err(ArgError::Version),
                other => return Err(ArgError::unknown(other)),
            }
        }
        // cargo refuses the pair too, but only after the analysis has been
        // set up, and in words about `--workspace` that this CLI never
        // asked the user to type.
        if !options.packages.is_empty() && !options.exclude.is_empty() {
            return Err(ArgError::Value(
                "`--package` and `--exclude` cannot be combined: name the \
                 members to check, or the members to leave out"
                    .to_string(),
            ));
        }
        Ok(options)
    }
}

/// Validate a `--cc` value against the dialect table, returning it in the
/// canonical `major.minor` form.
///
/// The normalized spelling — not the raw one — is what travels on into
/// `RECONVERGE_CC` and the `cc-marker`, so two spellings of one capability
/// (`8.6` and `+8.6`) do not look like a change and force a needless re-lint.
fn reconverge_dialect_oxide_cc_check(raw: &str) -> Result<String, String> {
    use reconverge_dialect_oxide::cc;
    let (major, minor) = cc::parse_compute_capability(raw)?;
    if cc::shared_memory_limits((major, minor)).is_none() {
        return Err(format!(
            "--cc {raw} is not in the compute-capability table; known: {}",
            cc::known_compute_capabilities().join(", ")
        ));
    }
    Ok(format!("{major}.{minor}"))
}

pub fn run(options: &CheckOptions) -> Result<Review, String> {
    let driver = locate_driver()?;
    let toolchain = resolve_toolchain()?;
    // The same cargo as the build, so the package ids the build reports
    // are spelled the way this listing spells them.
    let metadata = metadata_with(&toolchain.cargo)?;
    metadata.check_selection(&options.packages, &options.exclude)?;
    let reconverge_dir = metadata.target_directory.join("reconverge");
    let build_dir = reconverge_dir.join("build");
    fs::create_dir_all(&reconverge_dir)
        .map_err(|e| format!("cannot create {}: {e}", reconverge_dir.display()))?;
    let members = metadata.member_names();

    refresh_cc_marker(
        &reconverge_dir,
        &build_dir,
        options.cc.as_deref(),
        &driver_identity(&driver),
        &members,
    )?;

    let wrapped = Wrapped {
        driver: &driver,
        toolchain: &toolchain,
        reconverge_dir: &reconverge_dir,
        build_dir: &build_dir,
        metadata: &metadata,
        options,
    };
    let mut built = wrapped.run()?;
    let mut read = read_artifacts(&reconverge_dir, &built)?;

    // Self-heal: a target this build compiled with no readable findings
    // artifact (the artifacts directory was deleted while the build stayed
    // fresh, or a killed driver left half a file) gets one forced re-lint.
    // An unreadable file used to be fatal on every later run: the build was
    // fresh, so nothing ever rewrote it, and nothing said to delete it.
    if !read.missing.is_empty() || !read.unreadable.is_empty() {
        drop_build_fingerprints(&build_dir, &members);
        built = wrapped.run()?;
        read = read_artifacts(&reconverge_dir, &built)?;
    }
    if let Some((path, err)) = read.unreadable.first() {
        return Err(format!(
            "{} is not a findings.v1 artifact: {err}\n\
             it stayed unreadable after a forced re-lint; delete {} and rerun",
            path.display(),
            reconverge_dir.display()
        ));
    }

    // And then *read* the result of that re-lint, which is the half that was
    // missing: `missing` was computed, used to force a rebuild, and dropped
    // on the floor whether or not the rebuild helped. A member with no
    // artifact was silently absent from the report, so a two-member
    // workspace with a `default-members` line read as a clean one-member
    // one — and paid two full builds every run to accomplish it.
    //
    // Exit 2 rather than 1: the analysis did not run, which is a different
    // statement from "it ran and found something", and it is the code
    // consumers already treat as a hard stop for a candidate.
    if !read.missing.is_empty() {
        let names: Vec<String> = read.missing.iter().map(TargetKey::label).collect();
        return Err(format!(
            "{} {} compiled but produced no findings artifact and {} not \
             analyzed this run: {}\n\
             the driver writes one for every lib and bin target it compiles, \
             so a target without one means the driver did not run on it. A \
             target the gate cannot read is never a pass.",
            names.len(),
            reconverge_artifacts::plural(names.len(), "target", "targets"),
            reconverge_artifacts::plural(names.len(), "was", "were"),
            names.join(", "),
        ));
    }
    // Only now, with this run's answer in hand: a file of a target that no
    // longer exists is not this run's to gate on, and `inspect`, `witness`
    // and `triage` would otherwise go on showing it.
    prune_stale_artifacts(&reconverge_dir, &metadata);
    let artifacts = read.artifacts;

    // Version skew: an artifact this build did not produce is one it cannot
    // vouch for. Not fatal — an explicit RECONVERGE_DRIVER against a
    // deliberately different toolchain is supported, and the driver-identity
    // marker above already forces a re-lint on an in-place upgrade — but it
    // is never rendered silently as this run's own answer again.
    for artifact in &artifacts {
        if artifact.tool.version != env!("CARGO_PKG_VERSION") {
            eprintln!(
                "reconverge: `{}` was analyzed by {} {}, but this is \
                 cargo-reconverge {}; run `cargo reconverge setup` to install \
                 the matching driver",
                artifact.krate,
                artifact.tool.name,
                artifact.tool.version,
                env!("CARGO_PKG_VERSION"),
            );
        }
    }

    // `--strict` and `--show-suppressed` are deliberate no-ops in JSON mode
    // — machine consumers get the unfiltered record plus the baseline file,
    // never a pre-filtered mixture — but nothing said so, and a script
    // author who passes one reasonably believes it took effect.
    if options.message_format == MessageFormat::Json && (options.strict || options.show_suppressed)
    {
        eprintln!(
            "reconverge: --strict and --show-suppressed affect text output \
             only; --message-format json is always the unfiltered record"
        );
    }

    let review = Review::load(
        artifacts,
        options
            .baseline
            .clone()
            .unwrap_or_else(|| metadata.workspace_root.join(DEFAULT_BASELINE)),
        options.baseline.is_some(),
    )?;

    // The verdict is fully computed by now, so a reader that closes early
    // costs the report and not the exit code.
    crate::out::finish(match options.message_format {
        MessageFormat::Text => render::render_text(
            &review,
            &metadata.workspace_root,
            options.strict,
            options.show_suppressed,
        ),
        MessageFormat::Json => write_jsonl(&review),
    })?;
    for entry in review.stale_entries() {
        eprintln!(
            "reconverge: baseline entry {} no longer matches any finding; \
             delete it from {} or rerun `cargo reconverge triage`",
            entry.label(),
            review.baseline_path.display()
        );
    }
    if let Some(path) = &options.sarif_path {
        sarif::write_report(path, &review, &metadata.workspace_root)
            .map_err(|e| format!("cannot write SARIF to {}: {e}", path.display()))?;
    }
    // After the report, so the run still says what it compiled; the exit
    // code is the part a CI gate reads.
    if options.require_kernels && review.found_no_kernels() {
        return Err(format!(
            "--require-kernels: no #[kernel] function was analyzed in {} \
             {}. Check the working directory, that the crate depends on \
             cuda-oxide's device crate, and that the kernels are not behind a \
             disabled cfg or feature",
            review.artifacts.len(),
            reconverge_artifacts::plural(review.artifacts.len(), "crate", "crates"),
        ));
    }

    Ok(review)
}

/// The analysis record, unfiltered: the baseline is a review decision, and
/// machine consumers get the raw findings plus the baseline file itself
/// rather than a pre-filtered mixture.
fn write_jsonl(review: &Review) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for artifact in &review.artifacts {
        // Serializing a document we just deserialized cannot fail; carrying
        // it as an io error keeps one return type for the caller.
        let line = serde_json::to_string(artifact).map_err(io::Error::other)?;
        writeln!(out, "{line}")?;
    }
    out.flush()
}

/// The reconverge driver binary: `$RECONVERGE_DRIVER`, the sibling of this
/// executable, or the first one on `PATH`.
fn locate_driver() -> Result<PathBuf, String> {
    crate::setup_cmd::locate_companion(
        "reconverge-driver",
        "RECONVERGE_DRIVER",
        "cargo build -p reconverge-driver",
    )
}

/// One compiled target as the driver names its artifacts:
/// `findings-<krate>-<types>.json`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TargetKey {
    /// The target's crate name — the *target's*, not the package's: a
    /// `[[bin]]`, a renamed `[lib]` and a bin-only package all compile under
    /// a name the package does not have.
    pub(crate) krate: String,
    /// Its crate types, sorted and joined with `+`, exactly as the driver
    /// spells them from rustc's `--crate-type` arguments.
    pub(crate) types: String,
}

impl TargetKey {
    /// A target of a cargo metadata or build-message `target` object, if it
    /// is one `cargo check` compiles through the driver: a library of any
    /// crate type, or a binary. Build scripts go through the wrapper too,
    /// but every package's is named `build_script_build`, and examples,
    /// tests and benches are not part of a plain `cargo check`.
    fn from_json(target: &serde_json::Value) -> Option<TargetKey> {
        const CHECKED: &[&str] = &[
            "lib",
            "rlib",
            "dylib",
            "cdylib",
            "staticlib",
            "proc-macro",
            "bin",
        ];
        let checked = target["kind"]
            .as_array()?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|kind| CHECKED.contains(&kind));
        if !checked {
            return None;
        }
        let mut types: Vec<&str> = target["crate_types"]
            .as_array()?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        types.sort_unstable();
        types.dedup();
        Some(TargetKey {
            krate: target["name"].as_str()?.replace('-', "_"),
            types: types.join("+"),
        })
    }

    fn findings_file(&self) -> String {
        format!("findings-{}-{}.json", self.krate, self.types)
    }

    /// Whether `file_name` is one of this target's artifacts of `kind`
    /// (`findings`, `unimap` or `witness`).
    ///
    /// An exact name, not a substring: `-{crate}-` anywhere in the name
    /// matched a crate called `lib` against every library's artifacts.
    pub(crate) fn owns(&self, kind: &str, file_name: &str) -> bool {
        let stem = format!("{kind}-{}-{}", self.krate, self.types);
        match kind {
            "witness" => file_name
                .strip_prefix(&stem)
                .is_some_and(|rest| rest.starts_with('-') && rest.ends_with(".json")),
            _ => file_name.strip_prefix(&stem) == Some(".json"),
        }
    }

    fn label(&self) -> String {
        format!("`{}` ({})", self.krate, self.types)
    }
}

pub(crate) struct Metadata {
    pub(crate) target_directory: PathBuf,
    /// Workspace root: where the default baseline lives.
    pub(crate) workspace_root: PathBuf,
    /// Workspace members, each with the targets the driver compiles.
    pub(crate) members: Vec<Member>,
}

pub(crate) struct Member {
    /// cargo's package id, as its build messages spell it.
    id: String,
    name: String,
    targets: Vec<TargetKey>,
}

impl Metadata {
    /// Every target of every member, whether or not this run built it.
    pub(crate) fn targets(&self) -> impl Iterator<Item = &TargetKey> {
        self.members.iter().flat_map(|m| m.targets.iter())
    }

    /// The artifacts of `kind` in `dir` that belong to a current target,
    /// sorted. Anything else there — a deleted target's, another
    /// workspace's, an older naming scheme's — is not this project's answer.
    pub(crate) fn artifacts_in(&self, dir: &Path, kinds: &[&str]) -> io::Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if kinds
                .iter()
                .any(|kind| self.targets().any(|t| t.owns(kind, name)))
            {
                found.push(path);
            }
        }
        found.sort();
        Ok(found)
    }

    fn member_names(&self) -> Vec<String> {
        self.members.iter().map(|m| m.name.clone()).collect()
    }

    /// Refuse a `--package` or `--exclude` that names no member, before
    /// anything is built.
    fn check_selection(&self, packages: &[String], exclude: &[String]) -> Result<(), String> {
        for (flag, name) in packages
            .iter()
            .map(|n| ("--package", n))
            .chain(exclude.iter().map(|n| ("--exclude", n)))
        {
            if !self.members.iter().any(|m| &m.name == name) {
                return Err(format!(
                    "`{flag} {name}` names no member of this workspace; the \
                     members are: {}",
                    self.member_names().join(", ")
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn cargo_metadata() -> Result<Metadata, String> {
    metadata_with(Path::new(&cargo_bin()))
}

fn metadata_with(cargo: &Path) -> Result<Metadata, String> {
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        // Plain text: the message is re-printed under our own prefix, and
        // with `CARGO_TERM_COLOR=always` (CI sets it) cargo's `error: ` comes
        // wrapped in escapes that no prefix match would see.
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        // cargo's own message already begins `error: `, and ours is printed
        // under one: two prefixes and a trailing blank line, outside a
        // project, which is the first place anyone tries a new tool.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.trim();
        return Err(format!(
            "cargo metadata failed: {}",
            reason.strip_prefix("error: ").unwrap_or(reason)
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
    let members = json["packages"]
        .as_array()
        .ok_or("cargo metadata lacks packages")?
        .iter()
        .filter_map(|p| {
            Some(Member {
                id: p["id"].as_str()?.to_string(),
                name: p["name"].as_str()?.to_string(),
                targets: p["targets"]
                    .as_array()
                    .map(|targets| targets.iter().filter_map(TargetKey::from_json).collect())
                    .unwrap_or_default(),
            })
        })
        .collect();
    Ok(Metadata {
        target_directory,
        workspace_root,
        members,
    })
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Re-linting is keyed to the inputs cargo cannot see: `--cc`, and the
/// driver binary itself. When either changes we drop the workspace members'
/// fingerprints in our (dedicated) build dir.
///
/// The driver goes in as `RUSTC_WORKSPACE_WRAPPER`, and cargo does not
/// notice a same-path wrapper whose *contents* changed — so
/// `cargo install reconverge-driver` over the top re-linted nothing, and
/// the next run reprinted the previous build's verdict, witness note and
/// all. Upgrading to a driver that closes a missed detection therefore
/// changed nothing about the next run, and where the old verdict was clean
/// the build stayed green on a crate that now has a finding.
fn refresh_cc_marker(
    reconverge_dir: &Path,
    build_dir: &Path,
    cc: Option<&str>,
    driver: &str,
    members: &[String],
) -> Result<(), String> {
    let marker = reconverge_dir.join("cc-marker");
    let current = format!("{}\n{driver}\n", cc.unwrap_or("(none)"));
    let previous = fs::read_to_string(&marker).ok();
    if previous.as_deref() != Some(current.as_str()) {
        drop_build_fingerprints(build_dir, members);
        fs::write(&marker, &current).map_err(|e| format!("cannot write cc marker: {e}"))?;
    }
    Ok(())
}

/// A cheap identity for the driver binary: path, size and mtime.
///
/// Path plus size plus mtime is what cargo itself keys a wrapper on, and it
/// is enough for the case that matters — an install over the top. A content
/// hash would be more correct and costs a read of the whole binary on every
/// run; if the metadata is unavailable the identity degrades to the path,
/// which is what the marker held before.
fn driver_identity(driver: &Path) -> String {
    let mut identity = driver.display().to_string();
    if let Ok(meta) = fs::metadata(driver) {
        use std::fmt::Write as _;
        let _ = write!(identity, " {}", meta.len());
        if let Ok(mtime) = meta.modified()
            && let Ok(since) = mtime.duration_since(std::time::UNIX_EPOCH)
        {
            let _ = write!(identity, " {}", since.as_nanos());
        }
    }
    identity
}

/// Drop cargo's freshness fingerprints of `packages` in our dedicated build
/// directory, forcing the next wrapped `cargo check` to re-lint them.
///
/// Only the workspace members': the driver runs on nothing else, so nothing
/// else can have a stale lint. Sweeping every unit made `--cc 8.6` followed
/// by `--cc 9.0` recompile `syn` and the rest of the dependency tree to
/// re-lint one crate.
///
/// Two layouts, because cargo moved them: through 1.96 every profile
/// directory held one `.fingerprint/` tree (`<profile>/.fingerprint/<pkg>-<hash>`);
/// from 1.100 (the 2026-08-28 nightly) the fingerprint lives beside each
/// unit's build-script state, `<profile>/build/<pkg>/<hash>/fingerprint`,
/// and no `.fingerprint/` directory exists at all. Sweeping only the old
/// location made this function a no-op on the new cargo — a replaced driver
/// re-linted nothing, which is the regression `replacing_the_driver_in_place_forces_a_relint`
/// exists to catch. Both are swept, under every profile, so the layout cargo
/// picks is not something this code has to know.
fn drop_build_fingerprints(build_dir: &Path, packages: &[String]) {
    let Ok(profiles) = fs::read_dir(build_dir) else {
        return; // nothing built yet: nothing to invalidate
    };
    for profile in profiles.flatten() {
        let profile = profile.path();
        // Old layout: `<pkg>-<16 hex digits>`. Package names may contain
        // `-` themselves, so the whole suffix has to be the hash.
        if let Ok(units) = fs::read_dir(profile.join(".fingerprint")) {
            for unit in units.flatten() {
                let name = unit.file_name();
                let name = name.to_string_lossy();
                let ours = packages.iter().any(|pkg| {
                    name.strip_prefix(pkg.as_str())
                        .and_then(|rest| rest.strip_prefix('-'))
                        .is_some_and(|hash| {
                            !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())
                        })
                });
                if ours {
                    let _ = fs::remove_dir_all(unit.path());
                }
            }
        }
        // New layout: build/<pkg>/<hash>/fingerprint. Only the fingerprint
        // goes; a build script's `out` directory is regenerated on demand and
        // costs nothing to keep.
        for pkg in packages {
            let Ok(units) = fs::read_dir(profile.join("build").join(pkg)) else {
                continue;
            };
            for unit in units.flatten() {
                let _ = fs::remove_dir_all(unit.path().join("fingerprint"));
            }
        }
    }
}

/// The toolchain the wrapped build runs under, resolved once per run.
struct Toolchain {
    name: String,
    sysroot: PathBuf,
    /// That toolchain's own cargo, so the whole build — not only rustc —
    /// is the one the driver was built against.
    cargo: PathBuf,
}

/// The variable that chooses a toolchain other than the pin, for a driver
/// built against a different one (`RECONVERGE_DRIVER`).
const TOOLCHAIN_ENV: &str = "RECONVERGE_TOOLCHAIN";

/// The driver is a rustc-driver binary: it only runs against the exact
/// toolchain it was built by (the pin `cargo reconverge setup` installs,
/// which is also the repo's own `rust-toolchain.toml`). The analyzed
/// project has no reason to carry that pin, so the build always gets it —
/// exactly what action/action.yml does in CI — and the driver's dylib path
/// is resolved from it, never from the ambient toolchain.
///
/// `RUSTUP_TOOLCHAIN` is deliberately not consulted. It used to win, on the
/// theory that a set value was an explicit choice, but rustup's `cargo`
/// proxy exports it to every subcommand it runs, set to whatever it resolved
/// for the cwd — so a bare `cargo reconverge check` in a project without the
/// pin loaded the driver against `stable` and exited 2, and `setup` could
/// not fix it because the pin was already installed. An opt-out needs a
/// name nothing sets on the user's behalf.
fn resolve_toolchain() -> Result<Toolchain, String> {
    let name = std::env::var(TOOLCHAIN_ENV)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| crate::setup_cmd::PINNED_TOOLCHAIN.to_string());
    let sysroot = sysroot_of(&name)?;
    let own_cargo = sysroot
        .join("bin")
        .join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
    let cargo = if own_cargo.is_file() {
        own_cargo
    } else {
        PathBuf::from(cargo_bin())
    };
    Ok(Toolchain {
        name,
        sysroot,
        cargo,
    })
}

/// Sysroot of the given toolchain, for the driver's rustc dylibs.
fn sysroot_of(toolchain: &str) -> Result<PathBuf, String> {
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
                "cannot resolve the {toolchain} sysroot: {}\n{}",
                String::from_utf8_lossy(&output.stderr).trim(),
                crate::setup_cmd::missing_toolchain_hint(toolchain)
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
    Ok(PathBuf::from(sysroot.trim()))
}

impl Toolchain {
    /// Point `command`'s dynamic loader at this toolchain's rustc
    /// libraries, ahead of whatever the caller's environment holds — which,
    /// under rustup's proxy, is the *outer* toolchain's.
    fn library_env(&self, command: &mut Command) {
        let lib = self.sysroot.join("lib");
        command
            .env("LD_LIBRARY_PATH", prepend_path("LD_LIBRARY_PATH", &lib))
            .env(
                "DYLD_FALLBACK_LIBRARY_PATH",
                prepend_path("DYLD_FALLBACK_LIBRARY_PATH", &lib),
            );
        // Windows finds a DLL on PATH, and the rustc DLLs live in `bin`.
        if cfg!(windows) {
            command.env("PATH", prepend_path("PATH", &self.sysroot.join("bin")));
        }
    }
}

fn prepend_path(var: &str, dir: &Path) -> std::ffi::OsString {
    let existing = std::env::var_os(var).unwrap_or_default();
    let paths = std::iter::once(dir.to_path_buf())
        .chain(std::env::split_paths(&existing).filter(|p| !p.as_os_str().is_empty()));
    std::env::join_paths(paths).unwrap_or_else(|_| dir.as_os_str().to_os_string())
}

/// One wrapped `cargo check`, with everything it needs.
struct Wrapped<'a> {
    driver: &'a Path,
    toolchain: &'a Toolchain,
    reconverge_dir: &'a Path,
    build_dir: &'a Path,
    metadata: &'a Metadata,
    options: &'a CheckOptions,
}

impl Wrapped<'_> {
    /// Run the build and return the member targets it compiled, fresh or
    /// not — the set of findings artifacts this run has to account for.
    ///
    /// That set comes from cargo's own build messages rather than from a
    /// prediction. It used to be the package names, and the driver names an
    /// artifact after the *target*: a `[[bin]]`'s confirmed hang was left
    /// out of the report and exited 0, and a renamed `[lib]` exited 2 on
    /// every run, "not analyzed", having been analyzed twice.
    fn run(&self) -> Result<Vec<TargetKey>, String> {
        use std::io::{BufRead, BufReader, IsTerminal};

        let mut command = Command::new(&self.toolchain.cargo);
        command.arg("check");
        if self.options.packages.is_empty() {
            // The report has always been workspace-wide -- `cargo_metadata`
            // returns every member from any cwd -- so the build has to be
            // too. A bare `cargo check` took cargo's own package selection,
            // which narrows to the cwd's package or to `default-members`,
            // and the two sets disagreeing is what let a deny finding in an
            // unbuilt crate pass with the sibling's previous, clean artifact
            // still printed.
            command.arg("--workspace");
            for name in &self.options.exclude {
                command.args(["--exclude", name]);
            }
        } else {
            for name in &self.options.packages {
                command.args(["--package", name]);
            }
        }
        command
            // Diagnostics still go to stderr as text; stdout carries the
            // build's own account of which targets it compiled.
            .arg("--message-format=json-render-diagnostics")
            .env("RUSTC_WORKSPACE_WRAPPER", self.driver)
            .env("RECONVERGE_ARTIFACTS_OUT", self.reconverge_dir)
            .env("CARGO_TARGET_DIR", self.build_dir)
            // cargo 1.100 splits the build directory (fingerprints, deps,
            // incremental state) from the target directory, and
            // `CARGO_TARGET_DIR` alone leaves the build directory at its
            // default — the workspace's own `target/`. The wrapped check
            // would then share fingerprints with the user's plain `cargo
            // check`, and `drop_build_fingerprints` would sweep an empty
            // tree, so a driver replaced in place could never force a
            // re-lint. Pin both to our directory.
            .env("CARGO_BUILD_BUILD_DIR", self.build_dir)
            .env("RUSTUP_TOOLCHAIN", &self.toolchain.name);
        self.toolchain.library_env(&mut command);
        if let Some(cc) = &self.options.cc {
            command.env("RECONVERGE_CC", cc);
        }
        // cargo's stderr comes through us now, so it no longer sees a
        // terminal; keep its colours where the user would have had them.
        if std::env::var_os("CARGO_TERM_COLOR").is_none() && io::stderr().is_terminal() {
            command.env("CARGO_TERM_COLOR", "always");
        }
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run cargo check: {e}"))?;

        // stderr is passed through as it arrives, and read on the way for
        // the one failure that is not the project's code or the driver: a
        // build script. A thread, because stdout has to drain concurrently.
        let stderr = child.stderr.take().expect("stderr is piped");
        let relay = std::thread::spawn(move || {
            let mut failed_scripts = Vec::new();
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                let _ = io::stderr().write_all(&line);
                let text = String::from_utf8_lossy(&line);
                if let Some(rest) = text
                    .split("failed to run custom build command for `")
                    .nth(1)
                    && let Some(package) = rest.split('`').next()
                {
                    // `host v0.1.0 (/abs/path)`: the path adds nothing.
                    let package = package.split(" (").next().unwrap_or(package);
                    failed_scripts.push(package.to_string());
                }
                line.clear();
            }
            failed_scripts
        });

        let ids: BTreeSet<&str> = self
            .metadata
            .members
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        let mut built = BTreeSet::new();
        let stdout = child.stdout.take().expect("stdout is piped");
        // Bytes, not `lines()`: a line that is not UTF-8 must not end the
        // loop, or cargo blocks on a full pipe that nothing drains.
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
            let message = serde_json::from_slice::<serde_json::Value>(&line);
            line.clear();
            let Ok(message) = message else { continue };
            if message["reason"] == "compiler-artifact"
                && message["package_id"]
                    .as_str()
                    .is_some_and(|id| ids.contains(id))
                && let Some(target) = TargetKey::from_json(&message["target"])
            {
                built.insert(target);
            }
        }
        let status = child
            .wait()
            .map_err(|e| format!("cannot run cargo check: {e}"))?;
        let failed_scripts = relay.join().unwrap_or_default();
        if !status.success() {
            return Err(self.why_it_failed(&failed_scripts));
        }
        Ok(built.into_iter().collect())
    }

    /// Name the layer that failed. "The toolchain does not match the
    /// driver — run setup" was the answer to all three, and for the two
    /// that are not that, setup cannot help.
    fn why_it_failed(&self, failed_scripts: &[String]) -> String {
        // Asked directly rather than read off cargo's output: the loader's
        // wording differs on every platform, and a driver that cannot
        // answer its own flag cannot have analyzed anything either.
        let mut probe = Command::new(self.driver);
        probe.arg("--reconverge-version");
        self.toolchain.library_env(&mut probe);
        let starts = probe.output().is_ok_and(|output| output.status.success());
        if !starts {
            let hint = if std::env::var_os(TOOLCHAIN_ENV).is_some() {
                format!(
                    "{TOOLCHAIN_ENV} chose `{}`; unset it to use {}, or point \
                     RECONVERGE_DRIVER at a driver built by `{0}`",
                    self.toolchain.name,
                    crate::setup_cmd::PINNED_TOOLCHAIN
                )
            } else {
                format!(
                    "run `cargo reconverge setup` to install {} and the \
                     matching binaries",
                    crate::setup_cmd::PINNED_TOOLCHAIN
                )
            };
            return format!(
                "the reconverge driver ({}) cannot start under the `{}` \
                 toolchain, so nothing was analyzed; {hint}",
                self.driver.display(),
                self.toolchain.name,
            );
        }
        if !failed_scripts.is_empty() {
            return format!(
                "the build script of {} failed (see \
                 above). That is the project's own build, not the analysis: \
                 `check` builds every crate the selected members depend on, and \
                 a crate that links the CUDA host API needs the CUDA toolkit to \
                 build — no GPU, but the toolkit's headers. Install it, or \
                 check only the crate that holds the kernels: \
                 `cargo reconverge check --package <device crate>`",
                failed_scripts
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        "`cargo check` under the reconverge driver failed: the project did \
         not build (see the errors above). Fix those and rerun"
            .to_string()
    }
}

/// What reading this run's findings artifacts came to.
struct Reading {
    artifacts: Vec<FindingsArtifact>,
    /// Compiled targets with no findings file at all.
    missing: Vec<TargetKey>,
    /// Files that exist and are not a document — truncated, or empty.
    unreadable: Vec<(PathBuf, reconverge_artifacts::read::ReadError)>,
}

/// Read the findings artifact of every target in `built`, sorted by crate
/// name and then crate types.
///
/// Only the driver's current naming scheme is accepted —
/// `findings-<crate>-<crate types>.json`, with the filename's crate part
/// matching the artifact's own `crate` field. Anything else in the
/// directory (artifacts of targets this run did not build, files from
/// older naming schemes) is ignored, never merged.
///
/// A document that declares another schema is an error at once: that is a
/// version statement, and re-linting with the same driver cannot change
/// it. A document that does not parse is damage, and is reported back for
/// one re-lint.
fn read_artifacts(reconverge_dir: &Path, built: &[TargetKey]) -> Result<Reading, String> {
    use reconverge_artifacts::read::ReadError;

    let mut reading = Reading {
        artifacts: Vec::new(),
        missing: Vec::new(),
        unreadable: Vec::new(),
    };
    for target in built {
        let path = reconverge_dir.join(target.findings_file());
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                reading.missing.push(target.clone());
                continue;
            }
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        // The schema tag, at last. The error text below has always promised
        // this check; until 0.5.0 the only validation was the filename, so a
        // `findings.v99` document — from a driver this build has never met —
        // was merged, rendered, gated on and re-published on stdout while
        // the SARIF from the same invocation stamped a different producer.
        let mut artifact: FindingsArtifact =
            match reconverge_artifacts::read::deserialize_checked(&text) {
                Ok(artifact) => artifact,
                Err(err @ ReadError::Schema { .. }) => {
                    return Err(format!(
                        "{} is not a findings.v1 artifact: {err}; it was \
                         written by a different reconverge-driver — run \
                         `cargo reconverge setup` to install the matching one",
                        path.display()
                    ));
                }
                Err(err) => {
                    reading.unreadable.push((path, err));
                    continue;
                }
            };
        if artifact.krate == target.krate {
            // A document written before 0.5.0 has no `target`; the filename
            // has carried the crate types all along, so fill it in rather
            // than leave two lib+bin documents indistinguishable.
            if artifact.target.is_none() {
                artifact.target = Some(target.types.clone());
            }
            reading.artifacts.push(artifact);
        } else {
            // Its own name and its contents disagree: not this target's
            // answer, and never silently absent from the report.
            reading.missing.push(target.clone());
        }
    }
    // Total, so stream order stops depending on `read_dir`: `crate` alone
    // left two documents of one package ordered by directory iteration,
    // which is stable within a project and differs between two projects
    // whose package names differ and nothing else does.
    reading
        .artifacts
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    Ok(reading)
}

/// Remove the findings, unimap and witness files of targets that no longer
/// exist — a deleted `src/main.rs`, a renamed `[lib]`, a member that left
/// the workspace.
///
/// Nothing rewrites those, so they were permanent: a deleted binary's
/// confirmed RC001 went on gating every run, and a renamed library's old,
/// clean document was rendered in place of the new one's finding. Targets
/// that exist but were not built this run (`--exclude`, a bin whose
/// `required-features` are off) keep theirs.
fn prune_stale_artifacts(reconverge_dir: &Path, metadata: &Metadata) {
    let Ok(entries) = fs::read_dir(reconverge_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(kind) = ["findings", "unimap", "witness"]
            .into_iter()
            .find(|kind| name.starts_with(&format!("{kind}-")) && name.ends_with(".json"))
        else {
            continue;
        };
        if !metadata.targets().any(|t| t.owns(kind, name)) {
            let _ = fs::remove_file(entry.path());
        }
    }
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
        let old_layout = build_dir.join("debug/.fingerprint/some-crate-0123456789abcdef");
        let new_layout = build_dir.join("debug/build/some-crate/0123456789abcdef/fingerprint");
        fs::create_dir_all(&old_layout).unwrap();
        fs::create_dir_all(&new_layout).unwrap();

        drop_build_fingerprints(&build_dir, &["some-crate".to_string()]);

        assert!(!old_layout.exists());
        assert!(!new_layout.exists());
        // The rest of the profile directory survives: only freshness is
        // invalidated, compiled deps stay cached.
        assert!(
            build_dir
                .join("debug/build/some-crate/0123456789abcdef")
                .exists()
        );
    }

    #[test]
    fn only_the_members_fingerprints_are_dropped() {
        // `--cc 8.6` then `--cc 9.0` recompiled `syn`, `quote` and every
        // other dependency to re-lint the one crate the driver runs on.
        let build_dir = empty_dir("fingerprints-members");
        let member_old = build_dir.join("debug/.fingerprint/my-kernels-0123456789abcdef");
        let member_new = build_dir.join("debug/build/my-kernels/0123456789abcdef/fingerprint");
        let dep_old = build_dir.join("debug/.fingerprint/syn-0123456789abcdef");
        let dep_new = build_dir.join("debug/build/syn/0123456789abcdef/fingerprint");
        // A dependency whose name extends a member's: not the member's.
        let lookalike = build_dir.join("debug/.fingerprint/my-kernels-macros-0123456789abcdef");
        for dir in [&member_old, &member_new, &dep_old, &dep_new, &lookalike] {
            fs::create_dir_all(dir).unwrap();
        }

        drop_build_fingerprints(&build_dir, &["my-kernels".to_string()]);

        assert!(!member_old.exists() && !member_new.exists());
        assert!(
            dep_old.exists() && dep_new.exists(),
            "a dependency stays fresh"
        );
        assert!(lookalike.exists(), "`my-kernels-macros` is another package");
    }

    #[test]
    fn dropping_fingerprints_before_any_build_is_fine() {
        let build_dir = empty_dir("fingerprints-missing").join("never-built");
        drop_build_fingerprints(&build_dir, &["k".to_string()]); // must not panic or error
    }

    #[test]
    fn cc_check_normalizes_and_classifies() {
        // Both spellings of one capability normalize to the same marker
        // value, so `+8.6` after `8.6` is not seen as a change to re-lint.
        assert_eq!(reconverge_dialect_oxide_cc_check("8.6").unwrap(), "8.6");
        assert_eq!(reconverge_dialect_oxide_cc_check("+8.6").unwrap(), "8.6");
        // Out-of-range input reaches the table message, not "non-numeric".
        let err = reconverge_dialect_oxide_cc_check("999.999").unwrap_err();
        assert!(err.contains("not in the compute-capability table"), "{err}");
        assert!(!err.contains("non-numeric"), "{err}");
        // A valid but unlisted capability still reports against the table.
        assert!(
            reconverge_dialect_oxide_cc_check("9.9")
                .unwrap_err()
                .contains("not in the compute-capability table")
        );
    }

    fn key(krate: &str, types: &str) -> TargetKey {
        TargetKey {
            krate: krate.to_string(),
            types: types.to_string(),
        }
    }

    fn write_artifact(dir: &Path, krate: &str, types: &str) {
        let artifact = FindingsArtifact::new(krate, vec![]);
        fs::write(
            dir.join(format!("findings-{krate}-{types}.json")),
            serde_json::to_string(&artifact).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn artifacts_of_proc_macro_members_are_collected() {
        // The crate-types suffix can itself contain `-` (`proc-macro`).
        let dir = empty_dir("collect-proc-macro");
        write_artifact(&dir, "helper_macros", "proc-macro");

        let read = read_artifacts(&dir, &[key("helper_macros", "proc-macro")]).unwrap();
        assert_eq!(
            read.artifacts.len(),
            1,
            "proc-macro artifact must be collected"
        );
        assert_eq!(read.artifacts[0].krate, "helper_macros");
        assert!(read.missing.is_empty() && read.unreadable.is_empty());
    }

    #[test]
    fn a_target_is_named_by_its_crate_not_its_package() {
        // What cargo metadata and cargo's build messages say about a
        // package `with-bin` with a `[[bin]] bench` and a renamed lib.
        let lib = serde_json::json!({
            "kind": ["lib"], "crate_types": ["lib"], "name": "kern-device"
        });
        let bin = serde_json::json!({
            "kind": ["bin"], "crate_types": ["bin"], "name": "scale-offset"
        });
        let both = serde_json::json!({
            "kind": ["cdylib", "rlib"], "crate_types": ["rlib", "cdylib"], "name": "k"
        });
        let script = serde_json::json!({
            "kind": ["custom-build"], "crate_types": ["bin"], "name": "build-script-build"
        });
        let example = serde_json::json!({
            "kind": ["example"], "crate_types": ["bin"], "name": "demo"
        });
        assert_eq!(TargetKey::from_json(&lib), Some(key("kern_device", "lib")));
        assert_eq!(TargetKey::from_json(&bin), Some(key("scale_offset", "bin")));
        // Sorted and joined the way the driver joins rustc's --crate-type.
        assert_eq!(TargetKey::from_json(&both), Some(key("k", "cdylib+rlib")));
        assert_eq!(TargetKey::from_json(&script), None);
        assert_eq!(TargetKey::from_json(&example), None);
    }

    #[test]
    fn a_target_owns_exactly_its_own_files() {
        let lib = key("kern", "lib");
        assert!(lib.owns("findings", "findings-kern-lib.json"));
        assert!(lib.owns("unimap", "unimap-kern-lib.json"));
        assert!(lib.owns("witness", "witness-kern-lib-reduce-RC001-0.json"));
        // A substring match took these: another crate, another target.
        assert!(!lib.owns("findings", "findings-kern_device-lib.json"));
        assert!(!lib.owns("findings", "findings-kern-bin.json"));
        assert!(!lib.owns("witness", "witness-kern-lib+rlib-reduce-RC001-0.json"));
        assert!(!key("lib", "lib").owns("findings", "findings-kern-lib.json"));
    }

    #[test]
    fn a_truncated_artifact_is_reported_for_a_relint_not_fatal() {
        // A killed driver or a full disk leaves half a document. That used
        // to exit 2 on every later run: the build was fresh, so nothing
        // rewrote the file, and nothing said to delete it.
        let dir = empty_dir("collect-truncated");
        let artifact = FindingsArtifact::new("k", vec![]);
        let text = serde_json::to_string_pretty(&artifact).unwrap();
        fs::write(dir.join("findings-k-lib.json"), &text[..text.len() / 2]).unwrap();

        let read = read_artifacts(&dir, &[key("k", "lib"), key("k", "bin")]).unwrap();
        assert!(read.artifacts.is_empty());
        assert_eq!(read.missing, [key("k", "bin")]);
        assert_eq!(read.unreadable.len(), 1);
        assert!(read.unreadable[0].0.ends_with("findings-k-lib.json"));
    }

    #[test]
    fn a_newer_schema_is_an_error_with_the_setup_hint() {
        let dir = empty_dir("collect-schema");
        fs::write(
            dir.join("findings-k-lib.json"),
            r#"{"schema":"findings.v2","crate":"k","findings":[{"code":"RC001"}]}"#,
        )
        .unwrap();
        let err = read_artifacts(&dir, &[key("k", "lib")])
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.contains("schema is `findings.v2`, expected `findings.v1`"),
            "{err}"
        );
        assert!(err.contains("cargo reconverge setup"), "{err}");
    }

    fn metadata_of(targets: &[TargetKey]) -> Metadata {
        Metadata {
            target_directory: PathBuf::new(),
            workspace_root: PathBuf::new(),
            members: vec![Member {
                id: "id".into(),
                name: "pkg".into(),
                targets: targets.to_vec(),
            }],
        }
    }

    #[test]
    fn stale_targets_lose_their_artifacts_and_live_ones_keep_them() {
        let dir = empty_dir("prune");
        for name in [
            "findings-kern-lib.json",
            "unimap-kern-lib.json",
            "witness-kern-lib-reduce-RC001-0.json",
            // A deleted `src/main.rs`, and the renamed lib's old name.
            "findings-pkg-bin.json",
            "witness-pkg-bin-main_kernel-RC001-0.json",
            "findings-old_name-lib.json",
            "unimap-old_name-lib.json",
            // Not ours to judge.
            "cc-marker",
            "notes.json",
        ] {
            fs::write(dir.join(name), "{}").unwrap();
        }
        let metadata = metadata_of(&[key("kern", "lib")]);
        prune_stale_artifacts(&dir, &metadata);
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "cc-marker",
                "findings-kern-lib.json",
                "notes.json",
                "unimap-kern-lib.json",
                "witness-kern-lib-reduce-RC001-0.json",
            ]
        );
        assert_eq!(
            metadata
                .artifacts_in(&dir, &["findings", "unimap"])
                .unwrap(),
            [
                dir.join("findings-kern-lib.json"),
                dir.join("unimap-kern-lib.json")
            ]
        );
    }

    #[test]
    fn a_selection_names_members() {
        let metadata = metadata_of(&[key("kern", "lib")]);
        assert!(metadata.check_selection(&["pkg".into()], &[]).is_ok());
        let err = metadata.check_selection(&[], &["nope".into()]).unwrap_err();
        assert!(
            err.contains("`--exclude nope`") && err.contains("pkg"),
            "{err}"
        );
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_value_flag_does_not_swallow_the_next_flag() {
        // `--sarif --strict` used to write a report to a file named
        // `--strict` and drop strict mode. Same shape for `--baseline`.
        let err = CheckOptions::parse(&argv(&["--sarif", "--strict"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--sarif` requires a value (got the flag `--strict`)"
        );
        assert!(!err.wants_usage(), "a missing value is not unknown: {err}");

        let err = CheckOptions::parse(&argv(&["--baseline", "--sarif", "out.json"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--baseline` requires a value (got the flag `--sarif`)"
        );
        assert!(!err.wants_usage());

        let err = CheckOptions::parse(&argv(&["--sarif"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--sarif` requires a value");
    }

    #[test]
    fn a_boolean_rejects_an_inline_value() {
        let err = CheckOptions::parse(&argv(&["--strict=false"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--strict` takes no value");
        assert!(!err.wants_usage());

        let err = CheckOptions::parse(&argv(&["--show-suppressed=no"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--show-suppressed` takes no value");
        assert!(!err.wants_usage());

        let err = CheckOptions::parse(&argv(&["--show-suppressed="]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--show-suppressed` takes no value");
    }

    #[test]
    fn an_inline_value_may_look_like_a_flag() {
        // `--sarif=--weird` is the documented escape hatch for a path
        // that begins with `--`. The next-token guard must not apply.
        let options = CheckOptions::parse(&argv(&["--sarif=--weird", "--strict"])).unwrap();
        assert_eq!(options.sarif_path.as_deref(), Some(Path::new("--weird")));
        assert!(options.strict);
    }

    #[test]
    fn documented_flags_still_parse() {
        let options = CheckOptions::parse(&argv(&[
            "--strict",
            "--show-suppressed",
            "--sarif",
            "out.sarif",
            "--baseline",
            "bl.json",
            "--message-format",
            "json",
            "--cc",
            "8.6",
        ]))
        .unwrap();
        assert!(options.strict);
        assert!(options.show_suppressed);
        assert_eq!(options.sarif_path.as_deref(), Some(Path::new("out.sarif")));
        assert_eq!(options.baseline.as_deref(), Some(Path::new("bl.json")));
        assert!(matches!(options.message_format, MessageFormat::Json));
        assert_eq!(options.cc.as_deref(), Some("8.6"));
    }

    #[test]
    fn package_selection_parses_and_does_not_combine() {
        let options = CheckOptions::parse(&argv(&[
            "--package",
            "a",
            "-p",
            "b",
            "--package=c",
            "--require-kernels",
        ]))
        .unwrap();
        assert_eq!(options.packages, ["a", "b", "c"]);
        assert!(options.require_kernels);
        let options = CheckOptions::parse(&argv(&["--exclude", "host"])).unwrap();
        assert_eq!(options.exclude, ["host"]);

        let err = CheckOptions::parse(&argv(&["-p", "a", "--exclude", "b"]))
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("cannot be combined"), "{err}");
        assert!(!err.wants_usage());
        let err = CheckOptions::parse(&argv(&["--require-kernels=yes"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "`--require-kernels` takes no value");
    }
}
