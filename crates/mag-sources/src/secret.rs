//! A redacted secret wrapper shared by the credential store and source registry.

use std::fmt;

/// An opaque, redacted secret value such as an API key or bearer token.
///
/// `Secret` deliberately does **not** implement `serde::Serialize`,
/// `serde::Deserialize`, or [`std::fmt::Display`], and its [`fmt::Debug`]
/// output is redacted, so a value can be logged or embedded in an error message
/// without leaking the underlying credential (`docs/DESIGN.md` §3.5 / §9.5).
///
/// Secrets never enter a snapshot; they live only inside a
/// [`CredentialStore`](crate::CredentialStore) and are re-injected into a
/// [`ProviderConfig`](agent_lib::facade::ProviderConfig) on resume.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a raw secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the underlying secret material.
    ///
    /// The name is intentionally explicit: every call site is a point where the
    /// raw credential leaves the wrapper, so callers must handle it carefully
    /// and must never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    /// Prints a fixed placeholder so the secret never reaches a log or panic.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn expose_returns_raw_value() {
        let secret = Secret::new("sk-test-value");
        assert_eq!(secret.expose(), "sk-test-value");
    }

    #[test]
    fn debug_output_redacts_secret() {
        let secret = Secret::from("sk-super-secret");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("sk-super-secret"), "debug leaked secret");
        assert_eq!(rendered, "Secret(<redacted>)");
    }
}
