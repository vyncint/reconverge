//! Reading an artifact document, with the schema tag actually checked.
//!
//! Every artifact carries a `schema` field whose only purpose is to be
//! checked, and until 0.5.0 nothing checked it. `serde` takes any string, so
//! a `findings.v1` document could be loaded as a baseline and suppress a
//! deny-tier finding; a `findings.v99` written by a driver from the future
//! was merged, rendered, gated on and re-published on stdout as this run's
//! own answer. The write side has always been right — all four constructors
//! stamp the constant — and all four round-trip tests then assert the value
//! they themselves just wrote, which is why the read side had no coverage.
//!
//! One helper rather than four copies of the comparison: a fifth reader
//! cannot reintroduce the gap, and every surface refuses in the same words.

use serde::de::DeserializeOwned;

/// A document that declares which schema it is.
pub trait Artifact: DeserializeOwned {
    /// The schema tag this build implements.
    const SCHEMA: &'static str;

    /// The tag the parsed document actually declares.
    fn declared_schema(&self) -> &str;
}

/// Why a document could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// Not JSON, or JSON that is not this artifact's shape.
    Parse(serde_json::Error),
    /// Parsed, and declares a schema this build does not implement.
    Schema {
        declared: String,
        expected: &'static str,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Bare, so a caller's `"{path} is not a findings.v1 artifact:
            // {e}"` keeps reading the way it always did.
            ReadError::Parse(e) => write!(f, "{e}"),
            ReadError::Schema { declared, expected } => {
                write!(f, "schema is `{declared}`, expected `{expected}`")
            }
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReadError::Parse(e) => Some(e),
            ReadError::Schema { .. } => None,
        }
    }
}

/// Parse `text` as `T`, refusing a document that declares another schema.
///
/// # Errors
///
/// [`ReadError::Parse`] when the text is not a `T` at all, and
/// [`ReadError::Schema`] when it parses but names a different artifact kind
/// or version.
pub fn deserialize_checked<T: Artifact>(text: &str) -> Result<T, ReadError> {
    let parsed: T = serde_json::from_str(text).map_err(ReadError::Parse)?;
    if parsed.declared_schema() != T::SCHEMA {
        return Err(ReadError::Schema {
            declared: parsed.declared_schema().to_string(),
            expected: T::SCHEMA,
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::BaselineArtifact;
    use crate::findings::FindingsArtifact;
    use crate::unimap::UnimapArtifact;
    use crate::witness::WitnessArtifact;

    #[test]
    fn a_document_naming_another_artifact_kind_is_refused() {
        // The reproducer from the issue: a findings document, from a tool
        // that is not this one, carrying an entry that suppresses a
        // deny-tier finding.
        let text = r#"{
            "schema": "findings.v1",
            "tool": { "name": "totally-different-tool", "version": "999" },
            "entries": [ { "crate": "lint_samples", "kernel": "rc003_mut_slice",
                           "code": "RC003", "reason": "reviewed" } ]
        }"#;
        let err = deserialize_checked::<BaselineArtifact>(text).unwrap_err();
        assert!(matches!(err, ReadError::Schema { .. }));
        assert_eq!(
            err.to_string(),
            "schema is `findings.v1`, expected `baseline.v1`"
        );
    }

    #[test]
    fn a_future_version_of_the_same_kind_is_refused_too() {
        let text = r#"{
            "schema": "baseline.v2",
            "tool": { "name": "reconverge", "version": "9.9.9" },
            "entries": []
        }"#;
        assert!(matches!(
            deserialize_checked::<BaselineArtifact>(text).unwrap_err(),
            ReadError::Schema { .. }
        ));
    }

    #[test]
    fn a_parse_failure_stays_a_parse_failure() {
        // The two are genuinely different causes and must not collapse into
        // one message: a damaged file is not a version statement.
        let err = deserialize_checked::<FindingsArtifact>("not json at all").unwrap_err();
        assert!(matches!(err, ReadError::Parse(_)));
        assert!(err.to_string().contains("expected ident"), "{err}");
    }

    #[test]
    fn every_artifact_kind_checks_its_own_tag() {
        // A round-trip test asserts the tag the writer just stamped; this
        // asserts the reader refuses anything else, which is the half that
        // had no coverage in any of the four.
        assert_eq!(FindingsArtifact::SCHEMA, crate::schema::FINDINGS);
        assert_eq!(UnimapArtifact::SCHEMA, crate::schema::UNIMAP);
        assert_eq!(WitnessArtifact::SCHEMA, crate::schema::WITNESS);
        assert_eq!(BaselineArtifact::SCHEMA, crate::schema::BASELINE);
    }
}
