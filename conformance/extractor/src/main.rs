//! The conformance toolkit: `extract` upstream examples into kernel-only
//! crates, `mutate` them into the labeled bug corpus, and `score` a
//! mutation run into the published precision/recall table.
//!
//! Nothing generated here is committed to the reconverge repository except
//! the reviewed score report; corpus and mutants are regenerated from the
//! pinned checkout (`conformance/PIN`) on every run. Upstream sources are
//! Apache-2.0; generated crates reproduce their source headers.

mod extract;
mod mutate;
mod score;
mod util;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "\
usage: conformance-extractor extract <upstream-checkout> <corpus-out-dir>
       conformance-extractor mutate <corpus-dir|kernel-file.rs> <mutants-out-dir>
       conformance-extractor score <mutants-dir> <baseline.jsonl> <mutants.jsonl> <out.md>
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("extract") if args.len() == 3 => {
            extract::run(Path::new(&args[1]), Path::new(&args[2]))
                .map(|(extracted, skipped)| format!("{extracted} extracted, {skipped} skipped"))
        }
        Some("mutate") if args.len() == 3 => {
            let input = Path::new(&args[1]);
            let out = Path::new(&args[2]);
            if input.is_dir() {
                mutate::run_corpus(input, out).map(|n| format!("{n} mutant crate(s)"))
            } else {
                mutate::run_file(input, out).map(|n| format!("{n} mutant file(s)"))
            }
        }
        Some("score") if args.len() == 5 => {
            let mutants_dir = Path::new(&args[1]);
            let outcome = score::score(
                &mutants_dir.join("labels.tsv"),
                &mutants_dir.join("mutation-report.tsv"),
                Path::new(&args[2]),
                Path::new(&args[3]),
            );
            let (report, ok) = match outcome {
                Ok(md) => (md, true),
                Err(md) => (md, false),
            };
            if let Err(e) = std::fs::write(&args[4], &report) {
                Err(format!("cannot write {}: {e}", args[4]))
            } else if ok {
                Ok(format!("precision 1.0; report at {}", args[4]))
            } else {
                Err(format!(
                    "FALSE POSITIVES on the mutation corpus; see {}",
                    args[4]
                ))
            }
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(msg) => {
            eprintln!("conformance-extractor: {msg}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("conformance-extractor: {err}");
            ExitCode::FAILURE
        }
    }
}
