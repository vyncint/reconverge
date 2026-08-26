//! Typed bindings for the versioned artifact schemas in `schemas/`.
//!
//! The artifacts are the contract between the analysis side (driver, core,
//! witness) and every front-end (CLI text, TUI). Schemas are semver'd
//! independently and additive-only within a major version; `fixtures/`
//! holds the golden JSONs that act as the API tests, and this crate must
//! round-trip every one of them.
//!
//! Three of the schemas are produced by the driver; `baseline.v1` is
//! written by `cargo reconverge triage` — the one artifact a human owns.

#![forbid(unsafe_code)]

pub mod baseline;
pub mod findings;
pub mod unimap;
pub mod witness;

/// The singular or the plural of a word, chosen by a count.
///
/// Here, rather than in each front-end, because the same counts are printed
/// by the CLI summary, the TUI headers and the driver's own progress line,
/// and they should agree. Every one of them used to say `finding(s)` — on the
/// last line of every run, and in whatever a CI log pasted into an issue.
///
/// Both forms are taken rather than an `s` appended: it keeps the irregular
/// cases honest, and it lets a caller put the verb in too, which is the
/// difference between `1 day is short` and `1 day(s) are short`.
#[must_use]
pub fn plural<'a, N: PartialEq + From<u8>>(n: N, one: &'a str, many: &'a str) -> &'a str {
    if n == N::from(1) { one } else { many }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::fs;
    use std::path::Path;

    /// Round-trip every fixture in `fixtures/<dir>/` through a parser that
    /// returns the reserialized JSON value; the fixtures are the API tests.
    pub(crate) fn round_trip_fixtures(
        dir: &str,
        parse: impl Fn(&str) -> Result<serde_json::Value, serde_json::Error>,
    ) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(dir);
        let mut checked = 0;
        for entry in fs::read_dir(&dir).expect("fixture directory must exist") {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let reserialized =
                parse(&text).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
            let original: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                reserialized,
                original,
                "{} does not round-trip",
                path.display()
            );
            checked += 1;
        }
        assert!(checked >= 1, "no fixtures found in {dir:?}");
    }
}

/// Schema identifiers, as embedded in every emitted artifact.
pub mod schema {
    /// Findings artifact (diagnostics with provenance and confidence).
    pub const FINDINGS: &str = "findings.v1";
    /// Uniformity-map artifact (per-function labels, provenance edges, CFG).
    pub const UNIMAP: &str = "unimap.v1";
    /// Witness artifact (32-lane event timeline replaying a finding).
    pub const WITNESS: &str = "witness.v1";
    /// Baseline document (reviewed suppressions, maintained by `triage`).
    pub const BASELINE: &str = "baseline.v1";
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_identifiers_are_versioned() {
        for id in [
            super::schema::FINDINGS,
            super::schema::UNIMAP,
            super::schema::WITNESS,
            super::schema::BASELINE,
        ] {
            assert!(id.ends_with(".v1"));
        }
    }
}
