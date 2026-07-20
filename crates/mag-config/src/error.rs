//! Error type for configuration parsing, validation, and I/O.

use std::path::PathBuf;

/// Errors produced by the configuration DTO layer.
///
/// Two diagnostic shapes, per `docs/CLI.md` §4.1 item 5 ("verifiable,
/// diagnosable — bad config is never silent"):
///
/// - [`ConfigError::Parse`] carries **line/column** information derived from
///   the TOML parser's byte span, so a broken file points at the offending
///   line.
/// - [`ConfigError::Validation`] carries a **field path** (e.g.
///   `agents.reviewer.tools[2]`) for structural problems found after parsing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Filesystem failure while reading or writing a config file.
    #[error("I/O error on config file `{}`: {source}", .path.display())]
    Io {
        /// The file that could not be read or written.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// TOML syntax or schema failure during deserialization.
    ///
    /// `line`/`col` are 1-based; `0` means the parser did not report a span.
    #[error(
        "TOML parse error in `{}` at line {line}, column {col}: {message}",
        .path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<input>".to_string())
    )]
    Parse {
        /// Source file, when parsing from disk; `None` for `parse_str`.
        path: Option<PathBuf>,
        /// 1-based line of the error, or `0` if unknown.
        line: usize,
        /// 1-based column of the error, or `0` if unknown.
        col: usize,
        /// The parser's diagnostic message.
        message: String,
    },

    /// Failure while serializing a DTO to TOML.
    #[error("failed to serialize config to TOML: {message}")]
    Serialize {
        /// The serializer's diagnostic message.
        message: String,
    },

    /// Structural validation failure found after a successful parse.
    ///
    /// Semantic validation (dangling cross references, unknown enum values)
    /// is the resolve layer's job (DTO→DO); it reuses this variant.
    #[error("invalid configuration at `{path}`: {message}")]
    Validation {
        /// Dotted field path, e.g. `providers.anthropic.wire` or
        /// `agents.reviewer.tools[2]`.
        path: String,
        /// What is wrong with the field.
        message: String,
    },
}

impl ConfigError {
    /// Builds a [`ConfigError::Validation`] from a field path and message.
    pub fn validation(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            path: path.into(),
            message: message.into(),
        }
    }
}
