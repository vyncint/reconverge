//! What a command-line error is, and whether the usage text belongs with it.

use std::fmt;

/// A command-line error.
///
/// The distinction is not cosmetic — it decides whether forty-odd lines of
/// usage text follow the message.
///
/// For an argument nobody recognises, that reference *is* the answer: the
/// reader has just learned the interface does not contain what they typed,
/// and the list of what it does contain is the next thing they need.
///
/// For a recognised argument with an unusable value, the message already
/// names what is accepted — `--cc` answers with the compute-capability
/// table — and the usage text only pushes that answer out of view. A caller
/// reading the tail of stderr, which is where a failing tool usually puts
/// its reason, got the exit-code legend instead. launchbound reported it as
/// the cause of a failure, eleven times, one per candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// An argument that is not part of the interface.
    Unknown(String),
    /// A recognised argument whose value cannot be used.
    Value(String),
}

impl ArgError {
    /// Whether the usage text helps a reader who has just seen this.
    #[must_use]
    pub fn wants_usage(&self) -> bool {
        matches!(self, ArgError::Unknown(_))
    }

    /// An unrecognised argument, phrased the one way every command phrases it.
    #[must_use]
    pub fn unknown(argument: &str) -> Self {
        ArgError::Unknown(format!("unrecognized argument `{argument}`"))
    }
}

impl fmt::Display for ArgError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::Unknown(message) | ArgError::Value(message) => out.write_str(message),
        }
    }
}

/// Every other parse failure is a value error, so the parsers can keep
/// building their messages with `format!` and say nothing about layout.
impl From<String> for ArgError {
    fn from(message: String) -> Self {
        ArgError::Value(message)
    }
}

impl From<&str> for ArgError {
    fn from(message: &str) -> Self {
        ArgError::Value(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unknown_argument_asks_for_the_usage_text() {
        assert!(ArgError::unknown("--bogus").wants_usage());
        assert!(!ArgError::from("`80` is not a compute capability".to_string()).wants_usage());
    }

    #[test]
    fn the_message_is_the_whole_display() {
        // No prefix of its own: `main` already writes "error: ".
        assert_eq!(
            ArgError::unknown("--x").to_string(),
            "unrecognized argument `--x`"
        );
        assert_eq!(
            ArgError::from("bad value".to_string()).to_string(),
            "bad value"
        );
    }
}
