//! Secret references (`docs/CLI.md` §4.1 item 4).
//!
//! Configuration never stores secret values — only *references* resolved at
//! assembly time through the credential store. The canonical TOML form is the
//! inline table from the §4.2 example:
//!
//! ```toml
//! api_key = { env = "ANTHROPIC_API_KEY" }
//! api_key = { keyring = "mag/local_proxy" }
//! ```
//!
//! Because the DTO must also serve non-TOML sources (decision D4: GUI
//! patches, env overlays, tests), deserialization additionally accepts a
//! string mini-DSL: `"env:VAR_NAME"` / `"keyring:entry-name"`. Serialization
//! always emits the canonical table form.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::ser::SerializeMap as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A reference to a secret held outside the configuration.
///
/// The DTO layer only *holds* the reference; resolving it to a value happens
/// at assembly time (`Engine::from_config`), never here. No test or log may
/// print a resolved value (`docs/CLI.md` §4.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretRef {
    /// Reference to an environment variable (e.g. `ANTHROPIC_API_KEY`).
    Env(String),
    /// Reference to a keyring entry (e.g. `mag/local_proxy`).
    Keyring(String),
}

impl SecretRef {
    /// Creates a reference to an environment variable.
    pub fn env(name: impl Into<String>) -> Self {
        Self::Env(name.into())
    }

    /// Creates a reference to a keyring entry.
    pub fn keyring(name: impl Into<String>) -> Self {
        Self::Keyring(name.into())
    }

    /// The referenced name (variable name or keyring entry name).
    pub fn name(&self) -> &str {
        match self {
            Self::Env(name) | Self::Keyring(name) => name,
        }
    }

    /// The reference kind: `"env"` or `"keyring"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Env(_) => "env",
            Self::Keyring(_) => "keyring",
        }
    }
}

impl fmt::Display for SecretRef {
    /// Human display form used by masked config views (`docs/CLI.md` §4.3
    /// `ConfigView`): `{env = "VAR"}` / `{keyring = "NAME"}`. Never renders a
    /// secret value — there is none at this layer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "{{env = \"{name}\"}}"),
            Self::Keyring(name) => write!(f, "{{keyring = \"{name}\"}}"),
        }
    }
}

impl FromStr for SecretRef {
    type Err = String;

    /// Parses the string mini-DSL: `"env:VAR_NAME"` or
    /// `"keyring:entry-name"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, name) = s.split_once(':').ok_or_else(|| {
            format!("invalid secret reference {s:?}: expected \"env:<NAME>\" or \"keyring:<NAME>\"")
        })?;
        if name.trim().is_empty() {
            return Err(format!("invalid secret reference {s:?}: empty {kind} name"));
        }
        match kind {
            "env" => Ok(Self::Env(name.to_string())),
            "keyring" => Ok(Self::Keyring(name.to_string())),
            other => Err(format!(
                "invalid secret reference {s:?}: unknown kind {other:?} \
                 (expected \"env\" or \"keyring\")"
            )),
        }
    }
}

impl Serialize for SecretRef {
    /// Always serializes in the canonical §4.2 table form
    /// (`{ env = "VAR" }` / `{ keyring = "NAME" }`).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Self::Env(name) => map.serialize_entry("env", name)?,
            Self::Keyring(name) => map.serialize_entry("keyring", name)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    /// Accepts the canonical table form (`{ env = "VAR" }` /
    /// `{ keyring = "NAME" }`) or the string mini-DSL (`"env:VAR"` /
    /// `"keyring:NAME"`). Rejects anything else — in particular, a plain
    /// secret value cannot masquerade as a reference.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Table(BTreeMap<String, String>),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Str(s) => s.parse().map_err(D::Error::custom),
            Repr::Table(mut table) => {
                if table.len() != 1 {
                    return Err(D::Error::custom(format!(
                        "invalid secret reference: expected exactly one of \
                         {{env = \"VAR\"}} / {{keyring = \"NAME\"}}, got keys {:?}",
                        table.keys().collect::<Vec<_>>()
                    )));
                }
                let (key, value) = table.pop_first().expect("len checked above");
                if value.trim().is_empty() {
                    return Err(D::Error::custom(format!(
                        "invalid secret reference: empty {key} name"
                    )));
                }
                match key.as_str() {
                    "env" => Ok(Self::Env(value)),
                    "keyring" => Ok(Self::Keyring(value)),
                    other => Err(D::Error::custom(format!(
                        "invalid secret reference: unknown key {other:?} \
                         (expected \"env\" or \"keyring\")"
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_dsl_parses_both_kinds() {
        assert_eq!(
            "env:ANTHROPIC_API_KEY".parse(),
            Ok(SecretRef::Env("ANTHROPIC_API_KEY".into()))
        );
        assert_eq!(
            "keyring:mag/local_proxy".parse(),
            Ok(SecretRef::Keyring("mag/local_proxy".into()))
        );
    }

    #[test]
    fn string_dsl_rejects_malformed_input() {
        assert!("ANTHROPIC_API_KEY".parse::<SecretRef>().is_err());
        assert!("env:".parse::<SecretRef>().is_err());
        assert!("vault:secret".parse::<SecretRef>().is_err());
    }

    #[test]
    fn display_matches_masked_view_shape() {
        assert_eq!(
            SecretRef::env("ANTHROPIC_API_KEY").to_string(),
            "{env = \"ANTHROPIC_API_KEY\"}"
        );
        assert_eq!(
            SecretRef::keyring("mag/local_proxy").to_string(),
            "{keyring = \"mag/local_proxy\"}"
        );
    }
}
