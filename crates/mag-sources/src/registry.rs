//! The AI [`SourceRegistry`]: hosted LLM providers plus a reserved local-agent slot.
//!
//! LLM sources hold non-secret configuration (wire protocol, base URL, optional
//! version); the API key is supplied separately from a
//! [`CredentialStore`](crate::CredentialStore) as [`Credentials`] and combined
//! by [`SourceRegistry::provider_config`] into an agent-lib
//! [`ProviderConfig`]. Local-agent sources (Claude Code / Codex / OpenCode /
//! ACP) are declared as reserved slots only — the connecting path returns
//! [`SourceError::LocalAgentUnsupported`] until the interface layer lands it
//! (`docs/DESIGN.md` §10 I2, `PLAN.md` R-C).

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use agent_lib::facade::ProviderConfig;
use agent_lib::model::extras::ProviderId;

use crate::Credentials;

/// A hosted LLM provider source (Anthropic / OpenAI, via agent-lib).
///
/// Holds only non-secret configuration; the API key is provided at
/// [`provider_config`](SourceRegistry::provider_config) time from a
/// [`CredentialStore`](crate::CredentialStore).
#[derive(Clone, Debug)]
pub struct LlmSource {
    id: String,
    name: String,
    provider: ProviderId,
    base_url: String,
    api_version: Option<String>,
}

impl LlmSource {
    /// Creates an LLM source. The human-readable name defaults to `id`.
    #[must_use]
    pub fn new(id: impl Into<String>, provider: ProviderId, base_url: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            provider,
            base_url: base_url.into(),
            api_version: None,
        }
    }

    /// Sets a human-readable display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the protocol version override (`anthropic-version` header or
    /// OpenAI `api-version` query parameter).
    #[must_use]
    pub fn with_api_version(mut self, api_version: impl Into<String>) -> Self {
        self.api_version = Some(api_version.into());
        self
    }

    /// Returns the stable source id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the human-readable name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the wire protocol this source speaks.
    #[must_use]
    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    /// Returns the endpoint base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the protocol version override, if any.
    #[must_use]
    pub fn api_version(&self) -> Option<&str> {
        self.api_version.as_deref()
    }
}

/// The family of a reserved local-agent source.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalAgentKind {
    /// Anthropic Claude Code CLI.
    ClaudeCode,
    /// OpenAI Codex CLI.
    Codex,
    /// OpenCode CLI.
    OpenCode,
    /// A generic ACP-speaking local agent.
    Acp,
}

/// A reserved registration slot for a local coding-agent source.
///
/// The slot is listable so the UI can surface configured local agents, but
/// connecting to one is not yet implemented (see [`LocalAgentBackend`] and
/// [`SourceRegistry::connect_local_agent`]).
#[derive(Clone, Debug)]
pub struct LocalAgentSlot {
    id: String,
    name: String,
    kind: LocalAgentKind,
}

impl LocalAgentSlot {
    /// Creates a reserved local-agent slot. The name defaults to `id`.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: LocalAgentKind) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            kind,
        }
    }

    /// Sets a human-readable display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Returns the stable source id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the human-readable name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the local-agent family.
    #[must_use]
    pub fn kind(&self) -> LocalAgentKind {
        self.kind
    }
}

/// The reserved extension point for a connected local-agent backend.
///
/// This trait is the placeholder "slot" for live local-agent integration
/// (`docs/DESIGN.md` §10 I2, `PLAN.md` R-C). No implementation exists yet;
/// [`SourceRegistry::connect_local_agent`] therefore always fails with
/// [`SourceError::LocalAgentUnsupported`]. It is defined now so the registry API
/// is stable once a backend is added.
pub trait LocalAgentBackend: Send + Sync {
    /// Returns the source id of the backing local agent.
    fn id(&self) -> &str;

    /// Returns the local-agent family of this backend.
    fn kind(&self) -> LocalAgentKind;
}

/// An error returned by [`SourceRegistry`] operations.
///
/// Messages never contain secret material.
#[derive(Debug)]
#[non_exhaustive]
pub enum SourceError {
    /// No LLM source is registered under the given id.
    UnknownSource(String),
    /// A local-agent source is registered but connecting to it is not yet
    /// implemented (`docs/DESIGN.md` §10 I2, `PLAN.md` R-C).
    LocalAgentUnsupported(String),
    /// Building the provider configuration failed (for example a blank key).
    Provider(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource(id) => write!(formatter, "unknown LLM source `{id}`"),
            Self::LocalAgentUnsupported(id) => write!(
                formatter,
                "local-agent source `{id}` is registered but not yet implemented"
            ),
            Self::Provider(message) => write!(formatter, "provider configuration error: {message}"),
        }
    }
}

impl Error for SourceError {}

/// A registry of AI sources: hosted LLM providers plus reserved local agents.
///
/// LLM sources drive [`provider_config`](SourceRegistry::provider_config);
/// local-agent slots are declared for future live integration. New AI sources
/// register here and are delegated to uniformly (`docs/DESIGN.md` §8, §6).
#[derive(Default)]
pub struct SourceRegistry {
    llm: HashMap<String, LlmSource>,
    local_agents: HashMap<String, LocalAgentSlot>,
}

impl SourceRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or replaces) a hosted LLM source, keyed by its id.
    pub fn register_llm(&mut self, source: LlmSource) {
        self.llm.insert(source.id.clone(), source);
    }

    /// Registers (or replaces) a reserved local-agent slot, keyed by its id.
    ///
    /// The slot becomes listable via [`local_agents`](Self::local_agents), but
    /// [`connect_local_agent`](Self::connect_local_agent) still reports it as
    /// unimplemented until a [`LocalAgentBackend`] exists.
    pub fn register_local_agent(&mut self, slot: LocalAgentSlot) {
        self.local_agents.insert(slot.id.clone(), slot);
    }

    /// Returns the registered LLM sources, sorted by id for a stable listing.
    #[must_use]
    pub fn llm_sources(&self) -> Vec<&LlmSource> {
        let mut sources: Vec<&LlmSource> = self.llm.values().collect();
        sources.sort_by(|left, right| left.id.cmp(&right.id));
        sources
    }

    /// Returns the reserved local-agent slots, sorted by id.
    #[must_use]
    pub fn local_agents(&self) -> Vec<&LocalAgentSlot> {
        let mut slots: Vec<&LocalAgentSlot> = self.local_agents.values().collect();
        slots.sort_by(|left, right| left.id.cmp(&right.id));
        slots
    }

    /// Looks up a registered LLM source by id.
    #[must_use]
    pub fn llm_source(&self, source_id: &str) -> Option<&LlmSource> {
        self.llm.get(source_id)
    }

    /// Builds an agent-lib [`ProviderConfig`] for `source_id` using `creds`.
    ///
    /// The non-secret parts (protocol, base URL, version) come from the
    /// registered [`LlmSource`]; the API key comes from `creds`, which a caller
    /// reads from a [`CredentialStore`](crate::CredentialStore). The returned
    /// value carries the secret but redacts it in [`Debug`] and never
    /// serializes it.
    ///
    /// # Errors
    ///
    /// - [`SourceError::UnknownSource`] when `source_id` is not registered.
    /// - [`SourceError::Provider`] when the underlying builder rejects the
    ///   configuration (for example a blank base URL or key).
    pub fn provider_config(
        &self,
        source_id: &str,
        creds: &Credentials,
    ) -> Result<ProviderConfig, SourceError> {
        let source = self
            .llm
            .get(source_id)
            .ok_or_else(|| SourceError::UnknownSource(source_id.to_owned()))?;

        let builder = match source.provider {
            ProviderId::Anthropic => ProviderConfig::anthropic(),
            ProviderId::OpenAiResp => ProviderConfig::openai(),
            other => {
                return Err(SourceError::Provider(format!(
                    "unsupported provider {other:?}"
                )));
            }
        };

        let mut builder = builder
            .base_url(source.base_url.clone())
            .api_key(creds.api_key().expose());
        if let Some(version) = &source.api_version {
            builder = builder.api_version(version.clone());
        }

        builder
            .build()
            .map_err(|error| SourceError::Provider(error.to_string()))
    }

    /// Attempts to connect to a registered local-agent source.
    ///
    /// Always fails with [`SourceError::LocalAgentUnsupported`] in this version:
    /// the [`LocalAgentBackend`] slot is reserved but no backend is implemented
    /// yet (`docs/DESIGN.md` §10 I2, `PLAN.md` R-C).
    ///
    /// # Errors
    ///
    /// - [`SourceError::UnknownSource`] when `source_id` names no local agent.
    /// - [`SourceError::LocalAgentUnsupported`] for every registered slot.
    pub fn connect_local_agent(
        &self,
        source_id: &str,
    ) -> Result<Box<dyn LocalAgentBackend>, SourceError> {
        if !self.local_agents.contains_key(source_id) {
            return Err(SourceError::UnknownSource(source_id.to_owned()));
        }
        Err(SourceError::LocalAgentUnsupported(source_id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::{LlmSource, LocalAgentKind, LocalAgentSlot, SourceError, SourceRegistry};
    use crate::Credentials;
    use agent_lib::model::extras::ProviderId;

    fn registry() -> SourceRegistry {
        let mut registry = SourceRegistry::new();
        registry.register_llm(
            LlmSource::new(
                "anthropic-main",
                ProviderId::Anthropic,
                "https://api.anthropic.com",
            )
            .with_name("Anthropic"),
        );
        registry.register_llm(LlmSource::new(
            "openai-main",
            ProviderId::OpenAiResp,
            "https://api.openai.com",
        ));
        registry
    }

    #[test]
    fn lists_registered_llm_sources_sorted() {
        let registry = registry();
        let ids: Vec<&str> = registry.llm_sources().iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec!["anthropic-main", "openai-main"]);
    }

    #[test]
    fn provider_config_builds_from_credentials_without_leaking_secret() {
        let registry = registry();
        let creds = Credentials::new("sk-anthropic-secret");

        let config = registry
            .provider_config("anthropic-main", &creds)
            .expect("known source builds");

        assert_eq!(config.provider(), ProviderId::Anthropic);
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("sk-anthropic-secret"),
            "provider config debug leaked secret: {rendered}"
        );
    }

    #[test]
    fn provider_config_rejects_unknown_source() {
        let registry = registry();
        let creds = Credentials::new("sk-x");
        let error = registry
            .provider_config("missing", &creds)
            .expect_err("unknown source fails");
        assert!(matches!(error, SourceError::UnknownSource(id) if id == "missing"));
    }

    #[test]
    fn local_agent_slot_exists_but_is_unimplemented() {
        let mut registry = registry();
        registry.register_local_agent(
            LocalAgentSlot::new("claude-code", LocalAgentKind::ClaudeCode).with_name("Claude Code"),
        );

        // The slot is present and listable ...
        let ids: Vec<&str> = registry.local_agents().iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec!["claude-code"]);

        // ... but connecting to it is explicitly not implemented yet. The Ok
        // variant is a trait object without `Debug`, so match rather than
        // `expect_err`.
        match registry.connect_local_agent("claude-code") {
            Err(SourceError::LocalAgentUnsupported(id)) => assert_eq!(id, "claude-code"),
            Ok(_) => panic!("local agent connect should be unimplemented"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn connect_unknown_local_agent_reports_unknown_source() {
        let registry = registry();
        match registry.connect_local_agent("nope") {
            Err(SourceError::UnknownSource(id)) => assert_eq!(id, "nope"),
            Ok(_) => panic!("unknown local agent should fail"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
