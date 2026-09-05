//! The `watch` subcommand: re-run the check whenever a source file
//! changes, and keep printing the live findings dashboard.
//!
//! **Why this is a text surface and not a TUI view.** docs/ARCHITECTURE.md lists
//! `watch` among the TUI's views, but the TUI's own standard in the same
//! section forbids timers ("event-driven rendering only … no
//! timers/animation in v1"), which is what makes termlens's quiet-period
//! detection reliable. Learning about file changes therefore needs either
//! a poll (a timer, forbidden there) or an OS notification crate — and the
//! obvious one, `notify`, is CC0-1.0/Artistic-2.0, outside the `deny.toml`
//! allowlist, so adopting it is a stop-and-ask (§0.4), not a quiet config
//! edit. Watching from the CLI keeps the TUI timer-free and the dependency
//! set inside policy, at the cost of a text dashboard rather than an
//! interactive one; the interactive review lives in `triage`.
//!
//! Change detection polls (path, mtime, len) with `std` only. Runs are
//! numbered rather than timestamped so the output stays deterministic and
//! testable — a wall clock in the dashboard would be the same mistake §8
//! bans in frames.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{fs, thread};

use crate::args::{self, ArgError};
use crate::check::{self, CheckOptions, MessageFormat};
use reconverge_artifacts::plural;

/// How often the watch set is restatted. Cheap: metadata calls only.
const POLL: Duration = Duration::from_millis(250);

pub struct WatchOptions {
    pub check: CheckOptions,
    /// Stop after this many analysis runs (bounded runs; used by tests).
    pub max_runs: Option<usize>,
}

impl WatchOptions {
    pub fn parse(args: &[String]) -> Result<WatchOptions, ArgError> {
        // `--max-runs` is ours; everything else belongs to `check`, so the
        // two surfaces cannot drift apart on flags.
        let mut passthrough = Vec::new();
        let mut max_runs = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (flag, inline_value) = args::split_flag(arg);
            if flag == "--max-runs" {
                let raw = args::require_value("--max-runs", inline_value, || iter.next().cloned())?;
                let runs: usize = raw
                    .parse()
                    .map_err(|_| format!("--max-runs {raw} is not a number"))?;
                if runs == 0 {
                    return Err(ArgError::from("--max-runs must be at least 1"));
                }
                max_runs = Some(runs);
            } else {
                passthrough.push(arg.clone());
            }
        }
        Ok(WatchOptions {
            check: CheckOptions::parse(&passthrough)?,
            max_runs,
        })
    }
}

/// One dashboard line, on the stream that is not the machine record.
///
/// `check` nulls cargo's stdout so JSON mode stays clean, and puts even its
/// stale-baseline notes on stderr. `watch` broke that with three bare
/// `println!`s that never consulted the format: four of eight stdout lines
/// were prose, so a strict JSONL reader died on line 1 before a single
/// finding, and a lenient one silently discarded every run boundary.
///
/// stderr keeps the boundary — the one thing a JSON consumer of `watch`
/// genuinely needs — and costs nothing an interactive user notices, since
/// both streams land on the same terminal. Routing it here rather than at
/// three call sites is what stops the next dashboard line reintroducing it.
fn dashboard(format: MessageFormat, line: &str) -> io::Result<()> {
    if format == MessageFormat::Json {
        eprintln!("{line}");
        return Ok(());
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Run until `--max-runs` is exhausted, or forever (Ctrl-C). The exit code
/// is that of the last completed check.
pub fn run(options: &WatchOptions) -> Result<u8, String> {
    let metadata = check::cargo_metadata()?;
    let format = options.check.message_format;
    let mut watched = scan(&metadata.workspace_root);
    let mut runs = 0usize;
    // Overwritten by every run; only read after the loop ends.
    let mut exit;

    loop {
        runs += 1;
        crate::out::finish(dashboard(
            format,
            &format!(
                "reconverge watch: run #{runs} \u{2014} {} {} watched",
                watched.len(),
                plural(watched.len(), "file", "files")
            ),
        ))?;
        match check::run(&options.check) {
            // A broken build is the normal state mid-edit: report it and
            // keep watching rather than dropping the user back to a shell.
            // It is still a tool error, so a bounded run reports it as one
            // instead of exiting 0 on a project that never compiled.
            Err(error) => {
                eprintln!("error: {error}");
                exit = 2;
            }
            Ok(review) => exit = review.exit_code(),
        }
        if options.max_runs.is_some_and(|max| runs >= max) {
            return Ok(exit);
        }

        crate::out::finish(dashboard(
            format,
            "reconverge watch: waiting for changes (Ctrl-C to stop)",
        ))?;
        loop {
            thread::sleep(POLL);
            let current = scan(&metadata.workspace_root);
            if current != watched {
                watched = current;
                crate::out::finish(dashboard(format, ""))?;
                break;
            }
        }
    }
}

/// The watch set: every source file under `root`, with the stamp used to
/// detect edits. Skips `target/` and dot-directories, which churn on every
/// build and would re-trigger forever.
fn scan(root: &Path) -> BTreeMap<PathBuf, (SystemTime, u64)> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() && is_watched(&name) => {
                    if let Ok(metadata) = entry.metadata() {
                        let stamp = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        files.insert(path, (stamp, metadata.len()));
                    }
                }
                _ => {}
            }
        }
    }
    files
}

fn is_watched(name: &str) -> bool {
    name.ends_with(".rs") || name == "Cargo.toml" || name == "Cargo.lock"
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-watch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_watch_set_is_sources_only_and_skips_build_output() {
        let dir = temp_dir("scan");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.join("src/lib.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("src/notes.md"), "not source").unwrap();
        fs::write(dir.join("target/debug/build.rs"), "generated").unwrap();
        fs::write(dir.join(".git/config.rs"), "vcs internals").unwrap();

        let watched = scan(&dir);
        let names: Vec<String> = watched
            .keys()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["Cargo.toml", "src/lib.rs"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edits_change_the_watch_set_and_rebuilds_alone_do_not() {
        let dir = temp_dir("changes");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join("src/lib.rs"), "fn main() {}").unwrap();
        let before = scan(&dir);

        // Build output churning must not look like a source edit.
        fs::write(dir.join("target/artifact.rs"), "fresh output").unwrap();
        assert_eq!(scan(&dir), before, "target/ churn is invisible");

        // An edit that changes the length is caught even if the filesystem
        // has coarse mtime resolution.
        fs::write(dir.join("src/lib.rs"), "fn main() { /* edited */ }").unwrap();
        assert_ne!(scan(&dir), before, "source edits are caught");

        // A new source file is a change too.
        let after_edit = scan(&dir);
        fs::write(dir.join("src/extra.rs"), "").unwrap();
        assert_ne!(scan(&dir), after_edit, "new files are caught");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn max_runs_is_parsed_and_validated_while_check_flags_pass_through() {
        let argv =
            |args: &[&str]| -> Vec<String> { args.iter().map(ToString::to_string).collect() };

        let options = WatchOptions::parse(&argv(&["--max-runs", "3", "--strict"])).unwrap();
        assert_eq!(options.max_runs, Some(3));
        assert!(options.check.strict);

        let options = WatchOptions::parse(&argv(&["--max-runs=2"])).unwrap();
        assert_eq!(options.max_runs, Some(2));

        assert!(WatchOptions::parse(&argv(&[])).unwrap().max_runs.is_none());
        assert!(WatchOptions::parse(&argv(&["--max-runs", "0"])).is_err());
        assert!(WatchOptions::parse(&argv(&["--max-runs", "soon"])).is_err());
        assert!(WatchOptions::parse(&argv(&["--max-runs"])).is_err());
        // Unknown flags are still rejected — by `check`'s own parser.
        assert!(WatchOptions::parse(&argv(&["--bogus"])).is_err());

        let err = WatchOptions::parse(&argv(&["--max-runs", "--strict"]))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--max-runs` requires a value (got the flag `--strict`)"
        );
        assert!(!err.wants_usage());
    }
}
