//! `--explain RCxxx`: print the explain page for a diagnostic code.
//!
//! The pages live in this crate's `explain/` directory and are embedded at
//! build time, so `--explain` works offline, exactly as shipped — and so
//! they travel with the crate when it is published.

use crate::args::ArgError;

struct Page {
    code: &'static str,
    text: &'static str,
}

const PAGES: &[Page] = &[
    Page {
        code: "RC001",
        text: include_str!("../explain/RC001.md"),
    },
    Page {
        code: "RC002",
        text: include_str!("../explain/RC002.md"),
    },
    Page {
        code: "RC003",
        text: include_str!("../explain/RC003.md"),
    },
    Page {
        code: "RC004",
        text: include_str!("../explain/RC004.md"),
    },
    Page {
        code: "RC005",
        text: include_str!("../explain/RC005.md"),
    },
];

/// Diagnostic codes reserved for planned lints that have not shipped yet.
const RESERVED: &[(&str, &str)] = &[
    (
        "RC006",
        "strided global access from a lane-divergent index (planned for v1.1)",
    ),
    (
        "RC007",
        "shared-memory bank-conflict stride on a divergent index (planned for v1.1)",
    ),
];

/// Print the page for `code`. Exit 0 on success, 2 on an unknown code or
/// a reserved-but-unshipped one.
pub fn run(code: &str) -> u8 {
    let normalized = code.to_ascii_uppercase();
    if let Some(page) = PAGES.iter().find(|p| p.code == normalized) {
        print!("{}", page.text);
        return 0;
    }
    if let Some((_, what)) = RESERVED.iter().find(|(c, _)| *c == normalized) {
        eprintln!("error: {normalized} is reserved but not shipped yet: {what}");
        return 2;
    }
    eprintln!(
        "error: unknown diagnostic code `{code}`; known codes: {}",
        PAGES.iter().map(|p| p.code).collect::<Vec<_>>().join(", ")
    );
    2
}

/// Parse `--explain <CODE>`.
pub fn parse(args: &[String]) -> Result<String, ArgError> {
    let mut code = None;
    for arg in args {
        if ArgError::help(arg) {
            return Err(ArgError::Help);
        }
        if arg.starts_with('-') || code.is_some() {
            return Err(ArgError::unknown(arg));
        }
        code = Some(arg.clone());
    }
    code.ok_or_else(|| {
        ArgError::from("`--explain` requires a diagnostic code (e.g. `--explain RC001`)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_code_has_a_usable_page() {
        for page in PAGES {
            assert!(
                page.text.starts_with(&format!("# {} —", page.code)),
                "{} must open with its own heading",
                page.code
            );
            assert!(
                page.text.contains("```rust"),
                "{} must show a minimal kernel",
                page.code
            );
            // Warps are never described as running in lockstep.
            assert!(!page.text.to_ascii_lowercase().contains("lockstep"));
        }
    }

    /// No page may call a covered construct unchecked.
    ///
    /// `--explain RC002` told readers that `warp::ballot` under a divergent
    /// guard was "not yet checked" for two releases after the recognizer
    /// learned it — while it is the `confirmed` finding that just failed
    /// their CI. The natural conclusion is that the finding is a bug in
    /// reconverge, and the natural next step is a baseline entry against a
    /// real, gating, correct diagnostic. That is the expensive mistake the
    /// bullet made easy, and nothing could have noticed: the structural
    /// test above checks a heading, a fence and the word "lockstep".
    ///
    /// So the claim is *derived* rather than restated — the dialect is
    /// asked which names it classifies, and a page that calls one of them
    /// out of scope fails. Add a wrapper to the recognizer and any page
    /// still calling it unchecked goes red in the same PR.
    #[test]
    fn no_page_calls_a_recognized_construct_unchecked() {
        use reconverge_dialect_oxide::simt::classify_call;

        const OUT_OF_SCOPE: [&str; 4] = [
            "not yet checked",
            "out of scope",
            "recall gap",
            "cannot be checked",
        ];
        // The wrappers the classifier treats as full-mask collectives.
        // Named here rather than enumerated from the dialect because the
        // trait answers about one path at a time; the point is that this
        // list and the classifier agree, which the assertion below checks.
        const WRAPPERS: [&str; 4] = ["ballot", "all", "any", "shuffle"];

        for name in WRAPPERS {
            let path = format!("cuda_device::warp::{name}");
            assert!(
                matches!(
                    classify_call(&path),
                    reconverge_dialect_oxide::simt::CallKind::WarpCollective { .. }
                ),
                "`{path}` must be classified as a collective, or this test \
                 is checking a claim about something else"
            );
        }

        for page in PAGES {
            let lower = page.text.to_ascii_lowercase();
            for phrase in OUT_OF_SCOPE {
                let Some(at) = lower.find(phrase) else {
                    continue;
                };
                // The sentence around the phrase, generously bounded.
                let from = at.saturating_sub(400);
                let window = &lower[from..(at + phrase.len() + 200).min(lower.len())];
                for name in WRAPPERS {
                    assert!(
                        !window.contains(&format!("`warp::{name}`")),
                        "{}: `warp::{name}` is classified as a collective, but the \
                         page calls it {phrase:?}",
                        page.code
                    );
                }
            }
        }
    }

    #[test]
    fn parse_takes_exactly_one_code() {
        assert_eq!(parse(&["RC001".into()]).unwrap(), "RC001");
        assert!(parse(&[]).is_err());
        assert!(parse(&["RC001".into(), "RC002".into()]).is_err());
        assert!(parse(&["--flag".into()]).is_err());
    }

    #[test]
    fn run_normalizes_case_and_rejects_unknown_codes() {
        assert_eq!(run("rc001"), 0);
        assert_eq!(run("RC006"), 2); // reserved, unshipped
        assert_eq!(run("RC999"), 2);
    }
}
