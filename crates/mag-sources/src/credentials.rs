//! The [`CredentialStore`] abstraction plus its in-memory and OS-keyring backends.
//!
//! A store maps a stable string key (typically a source id) to a single
//! [`Secret`]. The in-memory [`MemoryCredentialStore`] backs offline tests and
//! ephemeral use; the OS keyring backend lives behind the `os-keyring` feature
//! so the default build never links a platform keyring (`docs/DESIGN.md` §3.5).

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Mutex;

use crate::Secret;

/// A group of credentials for a single AI source.
///
/// The first version carries only an API key (the sole secret needed to build a
/// [`ProviderConfig`](agent_lib::facade::ProviderConfig); the base URL and
/// version are non-secret [`LlmSource`](crate::LlmSource) configuration). It is
/// kept as a distinct type so future providers can extend the credential group
/// without changing the [`provider_config`](crate::SourceRegistry::provider_config)
/// signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    api_key: Secret,
}

impl Credentials {
    /// Builds a credential group from an API key or bearer token.
    #[must_use]
    pub fn new(api_key: impl Into<Secret>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    /// Returns the API key secret.
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }
}

/// An error returned by [`CredentialStore`] operations.
///
/// The message describes the backend failure and never contains secret
/// material.
#[derive(Debug)]
#[non_exhaustive]
pub enum CredentialError {
    /// The underlying credential backend (OS keyring or its lock) failed.
    Backend(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(message) => write!(formatter, "credential backend error: {message}"),
        }
    }
}

impl Error for CredentialError {}

/// Abstract secret store keyed by a stable string key.
///
/// Implementations must be safe to share across threads; mag holds a
/// `CredentialStore` behind an `Arc` and reads from it when (re)injecting a
/// [`ProviderConfig`](agent_lib::facade::ProviderConfig) into a session.
pub trait CredentialStore: Send + Sync {
    /// Returns the secret stored under `key`, or `None` when absent.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError`] when the backend cannot be read.
    fn get(&self, key: &str) -> Result<Option<Secret>, CredentialError>;

    /// Stores (or overwrites) the secret under `key`.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError`] when the backend cannot be written.
    fn set(&self, key: &str, secret: Secret) -> Result<(), CredentialError>;

    /// Removes any secret stored under `key`.
    ///
    /// Deleting a missing key succeeds (idempotent).
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError`] when the backend cannot be modified.
    fn delete(&self, key: &str) -> Result<(), CredentialError>;
}

/// An in-memory [`CredentialStore`] for tests and ephemeral, offline use.
///
/// Secrets live only in process memory and are dropped with the store; nothing
/// touches the OS keyring or disk.
#[derive(Default)]
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<String, Secret>>,
}

impl MemoryCredentialStore {
    /// Creates an empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Locks the inner map, mapping a poisoned lock to a backend error.
    fn entries(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, Secret>>, CredentialError> {
        self.entries
            .lock()
            .map_err(|_| CredentialError::Backend("credential store mutex poisoned".to_owned()))
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, key: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(self.entries()?.get(key).cloned())
    }

    fn set(&self, key: &str, secret: Secret) -> Result<(), CredentialError> {
        self.entries()?.insert(key.to_owned(), secret);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        self.entries()?.remove(key);
        Ok(())
    }
}

#[cfg(feature = "os-keyring")]
mod keyring_backend {
    use super::{CredentialError, CredentialStore, Secret};
    use keyring::{Entry, Error as KeyringError};

    /// An OS keyring-backed [`CredentialStore`] for production use.
    ///
    /// Each secret is stored as a keyring entry under a fixed service name and
    /// the store key as the account. The concrete platform backend is selected
    /// by the downstream binary's `keyring` feature (for example
    /// `keyring/apple-native`); this crate only wraps the API so tests never
    /// depend on a real keyring (`docs/DESIGN.md` §3.5).
    pub struct KeyringCredentialStore {
        service: String,
    }

    impl KeyringCredentialStore {
        /// Creates a keyring store whose entries share the given service name.
        #[must_use]
        pub fn new(service: impl Into<String>) -> Self {
            Self {
                service: service.into(),
            }
        }

        /// Builds the keyring [`Entry`] for `key` under this store's service.
        fn entry(&self, key: &str) -> Result<Entry, CredentialError> {
            Entry::new(&self.service, key)
                .map_err(|error| CredentialError::Backend(error.to_string()))
        }
    }

    impl CredentialStore for KeyringCredentialStore {
        fn get(&self, key: &str) -> Result<Option<Secret>, CredentialError> {
            match self.entry(key)?.get_password() {
                Ok(password) => Ok(Some(Secret::new(password))),
                Err(KeyringError::NoEntry) => Ok(None),
                Err(error) => Err(CredentialError::Backend(error.to_string())),
            }
        }

        fn set(&self, key: &str, secret: Secret) -> Result<(), CredentialError> {
            self.entry(key)?
                .set_password(secret.expose())
                .map_err(|error| CredentialError::Backend(error.to_string()))
        }

        fn delete(&self, key: &str) -> Result<(), CredentialError> {
            match self.entry(key)?.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                Err(error) => Err(CredentialError::Backend(error.to_string())),
            }
        }
    }
}

#[cfg(feature = "os-keyring")]
pub use keyring_backend::KeyringCredentialStore;

#[cfg(test)]
mod tests {
    use super::{CredentialStore, Credentials, MemoryCredentialStore};
    use crate::Secret;

    #[test]
    fn memory_store_round_trips_a_secret() {
        let store = MemoryCredentialStore::new();
        assert!(store.get("anthropic-main").unwrap().is_none());

        store
            .set("anthropic-main", Secret::new("sk-round-trip"))
            .unwrap();
        let fetched = store.get("anthropic-main").unwrap().expect("present");
        assert_eq!(fetched.expose(), "sk-round-trip");
    }

    #[test]
    fn memory_store_overwrites_and_deletes() {
        let store = MemoryCredentialStore::new();
        store.set("k", Secret::new("first")).unwrap();
        store.set("k", Secret::new("second")).unwrap();
        assert_eq!(store.get("k").unwrap().unwrap().expose(), "second");

        store.delete("k").unwrap();
        assert!(store.get("k").unwrap().is_none());
        // Deleting a missing key is idempotent.
        store.delete("k").unwrap();
    }

    #[test]
    fn credentials_debug_redacts_api_key() {
        let creds = Credentials::new("sk-should-not-print");
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("sk-should-not-print"), "leaked api key");
        assert_eq!(creds.api_key().expose(), "sk-should-not-print");
    }
}
