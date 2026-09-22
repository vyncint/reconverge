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
//!
//! # Which types are `#[non_exhaustive]`, and why the rest are not
//!
//! "Schemas are additive-only" and "adding a field is not a breaking change"
//! are not the same promise, and only the first one held. A new key in
//! `findings.v1` is additive in JSON and a major break in this crate, because
//! a struct with public fields can be built with a struct expression from
//! anywhere.
//!
//! The split is by *who builds the value*, measured rather than assumed:
//!
//! - **The four artifact roots** — [`findings::FindingsArtifact`],
//!   [`unimap::UnimapArtifact`], [`witness::WitnessArtifact`],
//!   [`baseline::BaselineArtifact`] — and [`findings::ToolInfo`] are
//!   `#[non_exhaustive]`. A reader deserializes a root; it does not assemble
//!   one field at a time, and each already has a `new` that fills the two
//!   fields nobody chooses (`schema`, `tool`). A top-level key is also the
//!   likeliest thing a schema gains.
//! - **The leaf records** — `Finding`, `SourceSpan`, `Step`, `Entry` and the
//!   rest — stay plain. The workspace's own front-ends build them 42 times
//!   between `cargo-reconverge`, `reconverge-tui` and the driver, and a
//!   downstream front-end writing artifacts would do exactly the same; making
//!   every one of those go through a constructor buys additive evolution at
//!   the cost of the thing this crate is for. When one of them does gain a
//!   field, that is a minor bump of a 0.x crate and the CHANGELOG says so.
//! - **The vocabulary enums** — [`findings::Confidence`],
//!   [`witness::LaneState`], [`witness::VerdictKind`], [`unimap::Uniformity`],
//!   [`unimap::ValueSource`] — stay exhaustive **on purpose**. They are the
//!   analysis, not a payload: a `_` arm over confidence tiers or lane states
//!   is a wrong answer that compiles, and a reader who has not been made to
//!   handle a new one is a reader silently mis-drawing it. Adding a variant
//!   is meant to be a break here.
//! - **[`read::ReadError`]** is `#[non_exhaustive]`, like any error enum:
//!   a new way for a document to be unreadable should not break a caller who
//!   only prints it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod baseline;
pub mod findings;
pub mod read;
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
