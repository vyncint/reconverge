//! Score a mutation-corpus run: join the labels against the tool's
//! findings, publish precision and recall, and fail on any false positive.
//!
//! Precision is judged at default confidence (deny/confirmed): every gating
//! finding on a mutant must be attributable either to that mutant's injected
//! bug or to the reviewed conformance baseline of its source example.
//! Precision 1.0 is a release requirement (see CONTRIBUTING.md); recall is
//! *reported*, per class, at both default and `--strict` confidence — the
//! honest number is the point, not a target.
//!
//! Detection is measured against the unmutated baseline: an injected bug
//! counts as detected only when the mutant shows *more* findings of the
//! expected code on the labeled kernel than the original example did, so
//! pre-existing upstream findings are never claimed as catches.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::mutate::{ALL_CLASSES, Class};

#[derive(Default, Clone)]
struct Counts {
    gating: usize,
    total: usize,
}

/// crate -> (code, kernel) -> counts
type FindingsMap = BTreeMap<String, BTreeMap<(String, String), Counts>>;

fn parse_jsonl(path: &Path) -> Result<FindingsMap, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut map = FindingsMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let doc: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("bad findings line: {e}"))?;
        let krate = doc["crate"]
            .as_str()
            .ok_or("findings document lacks `crate`")?
            .to_string();
        let per_crate = map.entry(krate).or_default();
        for finding in doc["findings"].as_array().into_iter().flatten() {
            let code = finding["code"].as_str().unwrap_or("?").to_string();
            let kernel = finding["kernel"].as_str().unwrap_or("-").to_string();
            let confidence = finding["confidence"].as_str().unwrap_or("?");
            let counts = per_crate.entry((code, kernel)).or_default();
            counts.total += 1;
            if confidence == "deny" || confidence == "confirmed" {
                counts.gating += 1;
            }
        }
    }
    Ok(map)
}

struct Label {
    mutant: String,
    class: Class,
    expected: String,
    source_crate: String,
    kernel: String,
}

fn parse_labels(path: &Path) -> Result<Vec<Label>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut labels = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            return Err(format!("malformed label row: {line}"));
        }
        let class = ALL_CLASSES
            .iter()
            .copied()
            .find(|c| c.slug() == fields[1])
            .ok_or_else(|| format!("unknown mutation class `{}`", fields[1]))?;
        labels.push(Label {
            mutant: fields[0].to_string(),
            class,
            expected: fields[2].to_string(),
            source_crate: fields[3].to_string(),
            kernel: fields[4].to_string(),
        });
    }
    Ok(labels)
}

#[derive(Default, Clone)]
struct ClassRow {
    emitted: usize,
    compiling: usize,
    detected_default: usize,
    detected_strict: usize,
}

fn percent(n: usize, d: usize) -> String {
    match (n * 100).checked_div(d) {
        None => "-".to_string(),
        Some(p) => format!("{n}/{d} ({p}%)"),
    }
}

/// Score a run. Returns `Ok(report)` when precision is 1.0; `Err` carries
/// the report plus the false-positive list otherwise.
pub fn score(
    labels_path: &Path,
    report_path: &Path,
    baseline_path: &Path,
    results_path: &Path,
) -> Result<String, String> {
    let labels = parse_labels(labels_path)?;
    let site_report = fs::read_to_string(report_path)
        .map_err(|e| format!("cannot read {}: {e}", report_path.display()))?;
    let baseline = parse_jsonl(baseline_path)?;
    let results = parse_jsonl(results_path)?;

    let empty = BTreeMap::new();
    let zero = Counts::default();
    let mut rows: BTreeMap<&'static str, ClassRow> = BTreeMap::new();
    let mut false_positives: Vec<String> = Vec::new();
    let mut gating_total = 0usize;

    for label in &labels {
        let row = rows.entry(label.class.slug()).or_default();
        row.emitted += 1;
        let Some(mutant_findings) = results.get(&label.mutant) else {
            continue; // did not compile; pruned and counted by the runner
        };
        row.compiling += 1;
        let base = baseline.get(&label.source_crate).unwrap_or(&empty);

        if label.expected != "-" {
            let key = (label.expected.clone(), label.kernel.clone());
            let got = mutant_findings.get(&key).unwrap_or(&zero);
            let had = base.get(&key).unwrap_or(&zero);
            if got.gating > had.gating {
                row.detected_default += 1;
            }
            if got.total > had.total {
                row.detected_strict += 1;
            }
        }

        // Precision: every gating finding must be the injected bug or a
        // baseline finding of the source example.
        for (key, counts) in mutant_findings {
            if counts.gating == 0 {
                continue;
            }
            gating_total += counts.gating;
            let mut allowed = base.get(key).unwrap_or(&zero).gating;
            if label.expected == key.0 && label.kernel == key.1 {
                allowed += 1;
            }
            if counts.gating > allowed {
                false_positives.push(format!(
                    "{}: {} on kernel `{}` — {} gating finding(s), {} attributable",
                    label.mutant, key.0, key.1, counts.gating, allowed
                ));
            }
        }
    }

    // Results for crates no label claims would mean the corpus and the run
    // disagree — never silently ignore that.
    for krate in results.keys() {
        if !labels.iter().any(|l| &l.mutant == krate) {
            false_positives.push(format!("{krate}: analyzed but not in labels.tsv"));
        }
    }

    let mut md = String::new();
    md.push_str("# Mutation-corpus results\n\n");
    md.push_str(
        "Generated by `scripts/run-mutation-corpus.sh` from the pinned upstream\n\
         examples (`conformance/PIN`) — mechanically injected bug classes, one\n\
         labeled single-site mutant per crate. CI regenerates this file on every\n\
         run and diffs it against the committed copy, so any precision or recall\n\
         movement is a deliberate, reviewed change. Do not edit the numbers by\n\
         hand; rerun the script and review the diff.\n\n",
    );

    md.push_str("## Summary\n\n");
    md.push_str("| class | injected bug | expected | mutants | compiling | detected (default) | detected (`--strict`) |\n");
    md.push_str("|-------|--------------|----------|--------:|----------:|-------------------:|----------------------:|\n");
    for class in ALL_CLASSES {
        let row = rows.get(class.slug()).cloned().unwrap_or_default();
        let what = match class {
            Class::WrapBarrier => "barrier wrapped in an index-derived `if`",
            Class::DeleteBarrier => "barrier deleted (data race)",
            Class::WrapCollective => "warp collective wrapped the same way",
            Class::ShrinkMask => "full mask shrunk to `0x0000_ffff`",
            Class::SwapMutSlice => "`DisjointSlice<T>` param swapped to `&mut [T]`",
        };
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} |",
            class.slug(),
            what,
            class.expected_code(),
            row.emitted,
            row.compiling,
            percent(row.detected_default, row.compiling),
            percent(row.detected_strict, row.compiling),
        );
    }

    let precision_ok = false_positives.is_empty();
    md.push('\n');
    if precision_ok {
        let _ = writeln!(
            md,
            "**Precision at default confidence: 1.000** — {gating_total} gating finding(s)\n\
             across all compiling mutants, every one attributed to its injected bug\n\
             or to the reviewed conformance baseline of its source example.",
        );
    } else {
        let _ = writeln!(md, "**Precision at default confidence: FAILED**\n");
        for fp in &false_positives {
            let _ = writeln!(md, "- {fp}");
        }
    }

    md.push_str(
        "\n## Reading the numbers\n\n\
         - **wrapbar** — detection at default confidence requires the witness\n\
           interpreter to replay a concrete hang; sites whose path to the barrier\n\
           crosses branches on values the interpreter honestly cannot know stay at\n\
           `warning` (visible with `--strict`). Both numbers are the product. A\n\
           wrap that lands on a barrier upstream *already* keeps under divergent\n\
           control cannot exceed the unmutated baseline and counts as undetected —\n\
           attribution never double-claims a pre-existing finding (at this pin,\n\
           every `--strict` miss is one of those found-in-the-wild sites).\n\
         - **delbar** — a deleted barrier is a data race, which static divergence\n\
           analysis cannot see by design (races are outside the decidable slice;\n\
           the witness replays only *found* divergence bugs). Expected recall 0;\n\
           the row is published anyway, and doubles as a precision invariant:\n\
           removing a barrier must not conjure findings.\n\
         - **wrapcol** — same mechanics as wrapbar, over the collectives the\n\
           dialect classifies: the masked `*_sync` surface of cuda-device's warp\n\
           module (`shuffle_*_sync` in every width, `ballot/any/all_sync`,\n\
           `match_*_sync`, `redux_sync_*`, `elect_sync`) plus `sync_mask`. Sites\n\
           hidden behind the unmasked convenience wrappers (`warp::shuffle`,\n\
           `warp::ballot`, the `reduce_*` helpers) are outside the v1 surface\n\
           and counted under site accounting below.\n\
           Promotion additionally needs a mask the analysis can evaluate; upstream\n\
           writes masks as named consts, which `rustc_public` cannot evaluate at\n\
           the pin, and an unevaluable mask is never witness-promoted — it could\n\
           be the correct guarded partial-warp idiom.\n\
         - **shrinkmask** — a shrunk full mask at a *convergent* call site names\n\
           no lane that is absent, so the witness has nothing to confirm: the\n\
           replay *does* compare the mask against the lanes it finds present\n\
           (that comparison is what promotes a divergent full-mask call and what\n\
           keeps the correct guarded partial-warp idiom out of the gate), and a\n\
           mismatch under some *other* launch shape than the replayed one is not\n\
           witnessed. Expected recall 0; the class stays in the corpus so the\n\
           boundary is public and any improvement shows up here.\n\
         - **mutslice** — RC003 is a syntactic `deny`; expected recall 100% of\n\
           compiling mutants. Swaps whose kernel body needs `DisjointSlice`-only\n\
           API do not compile and are pruned (counted below).\n",
    );

    md.push_str("\n## Site accounting\n\n");
    md.push_str(
        "Every site seen and every site skipped, with the reason — no silent\ncaps (from `mutation-report.tsv`):\n\n```\n",
    );
    md.push_str(site_report.trim_start_matches("# what\tcount\n"));
    md.push_str("```\n");

    if precision_ok { Ok(md) } else { Err(md) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn doc(krate: &str, findings: &[(&str, &str, &str)]) -> String {
        let items: Vec<String> = findings
            .iter()
            .map(|(code, kernel, conf)| {
                format!("{{\"code\":\"{code}\",\"kernel\":\"{kernel}\",\"confidence\":\"{conf}\"}}")
            })
            .collect();
        format!(
            "{{\"crate\":\"{krate}\",\"findings\":[{}]}}",
            items.join(",")
        )
    }

    #[test]
    fn detection_is_measured_against_the_baseline_and_fps_fail() {
        let dir = std::env::temp_dir().join(format!("rcv-score-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let labels = write(
            &dir,
            "labels.tsv",
            "# header\n\
             m_wrapbar_ex_00\twrapbar\tRC001\tconformance_ex\tk\t10\td\n\
             m_delbar_ex_00\tdelbar\t-\tconformance_ex\tk\t10\td\n\
             m_mutslice_ex_00\tmutslice\tRC003\tconformance_ex\tk\t5\td\n",
        );
        let report = write(
            &dir,
            "mutation-report.tsv",
            "# what\tcount\nemitted_wrapbar\t1\n",
        );
        // The source example already carries one RC001 warning on kernel k.
        let baseline = write(
            &dir,
            "base.jsonl",
            &(doc("conformance_ex", &[("RC001", "k", "warning")]) + "\n"),
        );

        // wrapbar: one MORE RC001, confirmed -> detected at default.
        // delbar: no findings. mutslice: RC003 deny -> detected.
        let results = write(
            &dir,
            "mut.jsonl",
            &format!(
                "{}\n{}\n{}\n",
                doc(
                    "m_wrapbar_ex_00",
                    &[("RC001", "k", "warning"), ("RC001", "k", "confirmed")]
                ),
                doc("m_delbar_ex_00", &[]),
                doc("m_mutslice_ex_00", &[("RC003", "k", "deny")]),
            ),
        );
        let md = score(&labels, &report, &baseline, &results).expect("precision holds");
        assert!(md.contains("**Precision at default confidence: 1.000**"));
        assert!(md.contains("| wrapbar | barrier wrapped in an index-derived `if` | RC001 | 1 | 1 | 1/1 (100%) | 1/1 (100%) |"));
        assert!(md.contains("| mutslice | `DisjointSlice<T>` param swapped to `&mut [T]` | RC003 | 1 | 1 | 1/1 (100%) | 1/1 (100%) |"));

        // Now a spurious gating finding on the delete mutant: precision fails.
        let bad = write(
            &dir,
            "bad.jsonl",
            &format!(
                "{}\n{}\n{}\n",
                doc("m_wrapbar_ex_00", &[("RC001", "k", "confirmed")]),
                doc("m_delbar_ex_00", &[("RC001", "k", "confirmed")]),
                doc("m_mutslice_ex_00", &[("RC003", "k", "deny")]),
            ),
        );
        let err = score(&labels, &report, &baseline, &bad).expect_err("FP must fail");
        assert!(err.contains("m_delbar_ex_00: RC001 on kernel `k`"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
