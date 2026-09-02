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

/// Split `--flag` / `--flag=value`. Every command goes through here so
/// the two loop shapes cannot drift: a `split_once('=')` parser used to
/// ignore an inline value on booleans, while a plain `for arg in args`
/// treated `--ascii=false` as unrecognized. Both now share the same two
/// rules — a boolean rejects `=value`, a value-taking flag refuses a
/// following token that looks like a flag.
pub(crate) fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (arg, None),
    }
}

/// A boolean flag: presence is enough. An inline `=value` is an error
/// even when the value would have been ignored — `--strict=false` used
/// to enable strict, which is the opposite of what it reads as.
pub(crate) fn reject_value(name: &str, inline: Option<&str>) -> Result<(), ArgError> {
    match inline {
        Some(_) => Err(ArgError::Value(format!("`{name}` takes no value"))),
        None => Ok(()),
    }
}

/// A required value. An inline `--flag=value` is accepted as-is, so a
/// path that itself starts with `--` is written `--sarif=--weird`. A
/// separate token that starts with `--` is not a value — that is how
/// `--sarif --strict` used to write a report to a file named `--strict`.
pub(crate) fn require_value(
    name: &str,
    inline: Option<&str>,
    next: impl FnOnce() -> Option<String>,
) -> Result<String, ArgError> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    match next() {
        Some(token) if token.starts_with("--") => Err(ArgError::Value(format!(
            "`{name}` requires a value (got the flag `{token}`)"
        ))),
        Some(value) => Ok(value),
        None => Err(ArgError::Value(format!("`{name}` requires a value"))),
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

    #[test]
    fn a_boolean_rejects_an_inline_value() {
        assert_eq!(
            reject_value("--strict", Some("false"))
                .unwrap_err()
                .to_string(),
            "`--strict` takes no value"
        );
        assert!(
            !reject_value("--strict", Some("false"))
                .unwrap_err()
                .wants_usage()
        );
        assert!(reject_value("--strict", None).is_ok());
    }

    #[test]
    fn a_value_flag_refuses_a_following_flag() {
        let err = require_value("--sarif", None, || Some("--strict".into())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "`--sarif` requires a value (got the flag `--strict`)"
        );
        assert!(!err.wants_usage());
        assert_eq!(
            require_value("--sarif", None, || None)
                .unwrap_err()
                .to_string(),
            "`--sarif` requires a value"
        );
        // Inline is the escape hatch: a path beginning with `--` is fine.
        assert_eq!(
            require_value("--sarif", Some("--weird"), || Some(
                "--must-not-consume".into()
            ))
            .unwrap(),
            "--weird"
        );
    }
}
