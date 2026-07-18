#![warn(missing_docs)]

//! AI source configuration and credential storage for mag.
//!
//! `mag-sources` owns two closely related concerns from `docs/DESIGN.md` §3.5
//! and §6:
//!
//! - **Credential storage.** A [`Secret`] wrapper plus a [`CredentialStore`]
//!   trait with an in-memory backend ([`MemoryCredentialStore`], for offline
//!   tests) and an OS keyring backend (`KeyringCredentialStore`, behind the
//!   `os-keyring` feature). Secrets never enter a snapshot; a session
//!   re-injects its [`ProviderConfig`](agent_lib::facade::ProviderConfig) from
//!   the store on resume.
//! - **Source registry.** A [`SourceRegistry`] of hosted LLM providers
//!   ([`LlmSource`], projected to an agent-lib `ProviderConfig` via
//!   [`SourceRegistry::provider_config`]) and reserved [`LocalAgentSlot`]s. The
//!   local-agent path ([`LocalAgentBackend`]) is a declared-but-unimplemented
//!   extension point for future live integration (`docs/DESIGN.md` §10 I2,
//!   `PLAN.md` R-C).
//!
//! The crate depends only on `agent-lib` (for `ProviderConfig`) and, optionally,
//! `keyring`; it does not depend on any transport or on `mag-core`.

mod credentials;
mod registry;
mod secret;

pub use credentials::{CredentialError, CredentialStore, Credentials, MemoryCredentialStore};
pub use registry::{
    LlmSource, LocalAgentBackend, LocalAgentKind, LocalAgentSlot, SourceError, SourceRegistry,
};
pub use secret::Secret;

#[cfg(feature = "os-keyring")]
pub use credentials::KeyringCredentialStore;
