//! Engine assembly from a runtime configuration snapshot (`docs/CLI.md`
//! §4.2/§4.3, decision D4).
//!
//! [`Engine::from_config`] is the production assembly constructor: it reads the
//! [`ConfigService`]'s current [`ConfigSnapshot`] and builds every runtime
//! handle the snapshot describes —
//!
//! - **LLM client** from the `agents.default` entry's provider, resolving the
//!   provider's `api_key` [`SecretRef`] **here**, at assembly time (env var
//!   read; keyring references report an explicit unsupported error in this
//!   build — see [`EngineError::Secret`]). Error messages name the provider and
//!   the reference, never a secret value (`docs/CLI.md` §4.1).
//! - **Tool registry** from the built-in tool set minus every `[tools.<name>]`
//!   entry with `enabled = false`. A `[tools.<name>]` entry naming no
//!   registered plugin is warned about and ignored (existence against the
//!   registry is the assembly layer's job, per `ConfigSnapshot::resolve`).
//! - **Source registry** (`mag-sources`): one [`LlmSource`] per
//!   `[providers.<name>]` entry plus one reserved [`LocalAgentSlot`] per
//!   `[external_agents.<name>]` entry (decision D3). The same config entries
//!   are projected into spawnable agent definitions when each session driver
//!   is built (`docs/dyn-agents.md` §3.2). ACP delegation is compiled behind
//!   this crate's default `external-acp` feature; disabling that feature
//!   makes any `[external_agents.*]` ACP entry a clear assembly error instead
//!   of a silent no-op.
//! - **Persistence** from `[session].persist_path` (a directory holding the
//!   SQLite database file), falling back to a private in-memory store.
//!
//! The **approval strategy** (`[approval]` + `[tools.<name>].approval`) and the
//! **session↔agent binding** are *not* baked into the engine at assembly time:
//! they are derived from the config service's current snapshot every time a
//! session actor spawns (see [`ApprovalOverrides`] and [`SessionBinding`]), so
//! a session (re)built after a configuration update picks up the new values —
//! the strongest semantics agent-lib's build-time approval surface allows
//! (`docs/CLI.md` §4.4, decision D2).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_lib::{
    adapter::{anthropic::AnthropicAdapter, openai_resp::OpenAiRespAdapter},
    client::LlmClient,
    facade::ProviderConfig,
    model::extras::ProviderId,
};
use mag_config::{
    AgentDefinitionRegistry, ApprovalPolicyKind, ConfigSnapshot, ExternalAgentKind, ProviderWire,
    ResolvedProvider, SecretRef,
};
use mag_service::{SessionBudget, SessionConfig};
use mag_sources::{
    Credentials, LlmSource, LocalAgentKind, LocalAgentSlot, Secret, SourceError, SourceRegistry,
};
use mag_tools::ToolRegistry;

use crate::{
    config::ConfigService,
    engine::Engine,
    persistence::{Persistence, PersistenceError},
};

/// Name of the `agents.<name>` entry a session binds to when its wire
/// `SessionConfig.provider` is empty or names no configured entry and the
/// configuration sets no `[session].default_agent` (`docs/CLI.md` §4.2; the
/// fallback keeps ACP's placeholder provider label and legacy free-form
/// labels working).
pub(crate) const DEFAULT_AGENT_NAME: &str = "default";

/// Effective default agent name for a snapshot: the configured
/// `[session].default_agent` when set, else the well-known `default` entry.
fn default_agent_name(snapshot: &ConfigSnapshot) -> &str {
    snapshot
        .session_defaults()
        .default_agent()
        .unwrap_or(DEFAULT_AGENT_NAME)
}

/// Filename of the SQLite session store created inside a configured
/// `[session].persist_path` directory.
const SESSION_DB_FILENAME: &str = "mag-sessions.db";

/// An error raised by [`Engine::from_config`] assembly.
///
/// No variant ever carries a secret value: secret failures name the provider
/// and the *reference* (environment variable / keyring entry name) only
/// (`docs/CLI.md` §4.1).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// A provider's `api_key` secret reference could not be resolved.
    #[error("secret resolution failed for provider `{provider}` ({reference}): {reason}")]
    Secret {
        /// Name of the `[providers.<name>]` entry whose reference failed.
        provider: String,
        /// The reference as written in the configuration
        /// (`{env = "VAR"}` / `{keyring = "NAME"}`), never a value.
        reference: String,
        /// Why the resolution failed (missing variable, unsupported kind).
        reason: String,
    },
    /// The agent-lib provider configuration could not be built (for example a
    /// blank base URL or key after resolution).
    #[error("provider assembly failed: {0}")]
    Provider(String),
    /// The session persistence store could not be opened.
    #[error("session persistence error: {0}")]
    Persistence(#[from] PersistenceError),
    /// The configuration names an external agent runtime that this build did
    /// not enable.
    #[error("external agent `{agent}` of kind `{kind}` requires the mag-core `{feature}` feature")]
    ExternalAgentUnsupported {
        /// Name of the `[external_agents.<name>]` entry.
        agent: String,
        /// Configured external-agent kind, such as `acp`.
        kind: String,
        /// Cargo feature that enables this runtime.
        feature: &'static str,
    },
    /// A filesystem operation failed (for example creating the configured
    /// persistence directory).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Engine {
    /// Assembles an engine from the configuration served by `config_service`
    /// (`docs/CLI.md` §4.2 startup assembly).
    ///
    /// The assembled engine holds the same [`ConfigService`], so the full
    /// `MagService` configuration surface (`get_config` / `update_config` /
    /// `reload_config` / `apply_config`) is live and
    /// [`ServiceEvent::ConfigChanged`](mag_service::ServiceEvent::ConfigChanged)
    /// is emitted on successful updates.
    ///
    /// # LLM client
    ///
    /// The engine's shared LLM client is built from the **default agent
    /// entry's provider** — the entry named by `[session].default_agent`
    /// when configured, else `agents.default`: its `base_url` (with a
    /// protocol default when unset)
    /// and its `api_key` [`SecretRef`], which is resolved here — environment
    /// variables are read at this point; `{keyring = "..."}` references report
    /// an explicit [`EngineError::Secret`] because this build has no keyring
    /// backend (the `mag-sources` `os-keyring` feature is off). A missing or
    /// empty variable, a provider without an `api_key` reference, or a keyring
    /// reference fails assembly with an error naming the provider and the
    /// reference — never a secret value. When the snapshot has no default
    /// agent entry or that entry names no provider (for example the
    /// built-in default configuration of a missing config file), the engine is
    /// assembled **without** a client: session management works and run
    /// attempts report `ServiceError::Backend`, mirroring [`Engine::new`].
    ///
    /// # Session ↔ agent binding
    ///
    /// A session created on a configuration-backed engine binds to an
    /// `agents.<name>` entry through its wire `SessionConfig.provider`:
    ///
    /// - An empty string, or a name with no matching `[agents.<name>]`
    ///   entry, binds the **default agent** entry — the one named by
    ///   `[session].default_agent` when configured, else the `default`
    ///   entry (an unmatched name is logged at warn level — this keeps
    ///   ACP's placeholder provider and legacy labels useful).
    /// - The bound entry supplies the session's `model`, tool surface, and
    ///   `system_prompt` when set (`docs/CLI.md` §4.4: new sessions use the
    ///   current DO graph); the wire `SessionConfig` values are the fallback
    ///   for fields the entry leaves unset.
    /// - Effective per-run budget: an explicit `SessionConfig.budget` wins,
    ///   then `agents.<name>.budget`, then `[session].budget`.
    /// - [`apply_config`](mag_service::MagService::apply_config) reconfigures
    ///   the session from **its bound entry** at turn boundaries.
    ///
    /// Engines built by the other constructors have no configuration backend
    /// and keep the legacy behaviour exactly: `provider` is a free-form label
    /// and the wire `SessionConfig` is used as-is.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the default provider's secret cannot be
    /// resolved, the provider configuration cannot be built, or the configured
    /// persistence path cannot be opened. When this crate is built without the
    /// default `external-acp` feature, an `[external_agents.*]` ACP entry reports
    /// [`EngineError::ExternalAgentUnsupported`] during assembly. Assembly
    /// failure is a diagnosable error, never a silent downgrade (`docs/CLI.md`
    /// §4.2).
    pub fn from_config(config_service: Arc<ConfigService>) -> Result<Self, EngineError> {
        Self::from_config_impl(config_service, None)
    }

    /// Like [`from_config`](Engine::from_config), but when the configuration
    /// sets no `[session].persist_path`, sessions are persisted under
    /// `default_persist_dir` instead of a private in-memory store. Interface
    /// binaries use this to give sessions a durable default home; an explicit
    /// `[session].persist_path` always wins.
    ///
    /// # Errors
    ///
    /// Same contract as [`from_config`](Engine::from_config), with the
    /// persistence errors now also covering `default_persist_dir`.
    pub fn from_config_with_default_persist_path(
        config_service: Arc<ConfigService>,
        default_persist_dir: &Path,
    ) -> Result<Self, EngineError> {
        Self::from_config_impl(config_service, Some(default_persist_dir))
    }

    fn from_config_impl(
        config_service: Arc<ConfigService>,
        default_persist_dir: Option<&Path>,
    ) -> Result<Self, EngineError> {
        let snapshot = config_service.current();
        reject_external_agents_without_feature(&snapshot)?;
        let tools = assemble_tool_registry(&snapshot);
        let sources = assemble_source_registry(&snapshot);
        let client = assemble_llm_client(&snapshot, &sources)?;
        let store = open_session_store(&snapshot, default_persist_dir)?;
        Ok(Self::assemble(
            client,
            Arc::new(tools),
            store,
            Some(config_service),
            sources,
        ))
    }
}

#[cfg(feature = "external-acp")]
fn reject_external_agents_without_feature(_snapshot: &ConfigSnapshot) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(not(feature = "external-acp"))]
fn reject_external_agents_without_feature(snapshot: &ConfigSnapshot) -> Result<(), EngineError> {
    if let Some(external) = snapshot
        .external_agents()
        .values()
        .find(|external| matches!(external.effective_kind(), ExternalAgentKind::Acp))
    {
        return Err(EngineError::ExternalAgentUnsupported {
            agent: external.name().to_owned(),
            kind: "acp".to_owned(),
            feature: "external-acp",
        });
    }
    Ok(())
}

/// Names of the facade-level agent-instance tools the session driver appends
/// to every surface (`docs/dyn-agents.md` §5.1, M3-5): `[tools.<name>]`
/// entries for them are legitimate even though no registry plugin bears the
/// name.
const INSTANCE_TOOL_NAMES: [&str; 3] = [
    crate::instances::spawn::AGENT_TOOL_NAME,
    crate::instances::spawn::AGENT_RESULT_TOOL_NAME,
    crate::instances::spawn::AGENT_CANCEL_TOOL_NAME,
];

/// Builds the tool registry for an engine assembled from `snapshot`
/// (`docs/CLI.md` §4.2): the built-in minimal tool set minus every
/// `[tools.<name>]` entry with `enabled = false`.
///
/// A `[tools.<name>]` entry naming no registered plugin is warned about and
/// ignored — the approval half of such an entry can never match a projected
/// tool either, so ignoring it entirely keeps assembly robust against
/// forward-written configuration. The `agent` / `agent_result` /
/// `agent_cancel` instance tools ( `docs/dyn-agents.md` §5.1) are exempt from
/// that warning: they are facade-level tools the session driver appends to
/// every surface, so `[tools.agent]`-style entries legitimately name no
/// registry plugin while their `approval` tiers still apply (M3-5).
pub(crate) fn assemble_tool_registry(snapshot: &ConfigSnapshot) -> ToolRegistry {
    let builtins = ToolRegistry::with_builtins();
    let mut registry = ToolRegistry::new();
    for plugin in builtins.plugins() {
        let disabled = snapshot
            .tool(plugin.name())
            .is_some_and(|tool| !tool.is_enabled());
        if disabled {
            tracing::info!(tool = plugin.name(), "tool disabled by configuration");
        } else {
            registry = registry.register(Arc::clone(plugin));
        }
    }
    for name in snapshot.tools().keys() {
        if !builtins.plugins().iter().any(|p| p.name() == name)
            && !INSTANCE_TOOL_NAMES.contains(&name.as_str())
        {
            tracing::warn!(
                tool = name.as_str(),
                "tool override names no registered tool plugin; entry ignored"
            );
        }
    }
    registry
}

/// Builds the `mag-sources` registry from `snapshot` (`docs/CLI.md` §4.2/§4.6):
/// one [`LlmSource`] per provider entry and one reserved [`LocalAgentSlot`]
/// per external-agent entry. The slots make external ACP sources listable; the
/// same entries are projected into spawnable agent definitions by
/// [`SessionBinding`] and merged into the session driver's definition table
/// (`docs/dyn-agents.md` §3.2).
pub(crate) fn assemble_source_registry(snapshot: &ConfigSnapshot) -> SourceRegistry {
    let mut registry = SourceRegistry::new();
    for (name, provider) in snapshot.providers() {
        let id = match provider.wire() {
            ProviderWire::Anthropic => ProviderId::Anthropic,
            ProviderWire::OpenAi => ProviderId::OpenAiResp,
        };
        let base_url = provider
            .base_url()
            .map(str::to_owned)
            .unwrap_or_else(|| default_base_url(provider.wire()).to_owned());
        registry.register_llm(LlmSource::new(name.clone(), id, base_url));
    }
    for (name, external) in snapshot.external_agents() {
        match external.effective_kind() {
            ExternalAgentKind::Acp => {
                registry
                    .register_local_agent(LocalAgentSlot::new(name.clone(), LocalAgentKind::Acp));
            }
        }
    }
    registry
}

/// The well-known endpoint a provider defaults to when the configuration sets
/// no `base_url`.
fn default_base_url(wire: ProviderWire) -> &'static str {
    match wire {
        ProviderWire::Anthropic => "https://api.anthropic.com",
        ProviderWire::OpenAi => "https://api.openai.com",
    }
}

/// Builds the engine's shared LLM client from the default agent entry's
/// provider (`[session].default_agent` when configured, else `agents.default`),
/// resolving the provider's secret reference (see the
/// [`Engine::from_config`] rustdoc for the full contract).
///
/// Returns `Ok(None)` when the snapshot defines no default agent or the
/// default agent names no provider — the engine then runs clientless and
/// reports run attempts as backend errors, mirroring [`Engine::new`].
fn assemble_llm_client(
    snapshot: &ConfigSnapshot,
    sources: &SourceRegistry,
) -> Result<Option<Arc<dyn LlmClient>>, EngineError> {
    let Some(agent) = snapshot.agent(default_agent_name(snapshot)) else {
        return Ok(None);
    };
    let Some(provider) = agent.provider() else {
        return Ok(None);
    };
    let credentials = resolve_provider_secret(provider)?;
    let provider_config = sources
        .provider_config(provider.name(), &credentials)
        .map_err(source_error)?;
    Ok(Some(client_for_provider(provider_config)?))
}

/// Resolves a provider's `api_key` [`SecretRef`] into [`Credentials`].
///
/// This is the single point where a configuration secret materializes
/// (`docs/CLI.md` §4.1 item 4: "取值发生在装配 ProviderConfig 的最后一刻").
/// Only the default agent's provider reaches this function; references on
/// other providers stay unresolved until a consumer needs them. Failures name
/// the provider and the reference, never a value.
fn resolve_provider_secret(provider: &ResolvedProvider) -> Result<Credentials, EngineError> {
    let Some(reference) = provider.api_key() else {
        return Err(EngineError::Secret {
            provider: provider.name().to_owned(),
            reference: format!("providers.{}.api_key", provider.name()),
            reason: "no api_key secret reference configured".to_owned(),
        });
    };
    let value = match reference {
        SecretRef::Env(name) => match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                return Err(EngineError::Secret {
                    provider: provider.name().to_owned(),
                    reference: reference.to_string(),
                    reason: format!("environment variable `{name}` is not set or is empty"),
                });
            }
        },
        SecretRef::Keyring(_) => {
            return Err(EngineError::Secret {
                provider: provider.name().to_owned(),
                reference: reference.to_string(),
                reason: "keyring secret references are not supported by this build \
                         (the mag-sources `os-keyring` feature is disabled); use an \
                         `{env = \"...\"}` reference instead"
                    .to_owned(),
            });
        }
    };
    Ok(Credentials::new(Secret::new(value)))
}

/// Maps a `mag-sources` error into an [`EngineError`]. [`SourceError`]
/// messages are documented to never contain secret material.
fn source_error(error: SourceError) -> EngineError {
    EngineError::Provider(error.to_string())
}

/// Builds the concrete agent-lib adapter client for a [`ProviderConfig`]'s
/// wire protocol. The configuration's wire strings are validated at resolve
/// time, so both `ProviderId` variants are always constructible here; the
/// wildcard arm only guards future agent-lib variants.
fn client_for_provider(provider_config: ProviderConfig) -> Result<Arc<dyn LlmClient>, EngineError> {
    let (endpoint, provider) = provider_config.into_parts();
    match provider {
        ProviderId::Anthropic => Ok(Arc::new(AnthropicAdapter::new(endpoint))),
        ProviderId::OpenAiResp => Ok(Arc::new(OpenAiRespAdapter::new(endpoint))),
        other => Err(EngineError::Provider(format!(
            "unsupported provider protocol {other:?}"
        ))),
    }
}

/// Opens the session persistence store selected by `snapshot`: a SQLite
/// database inside the configured `[session].persist_path` directory (created
/// when missing), else inside `default_persist_dir` when the caller supplies
/// one, or a private in-memory store when neither is set.
fn open_session_store(
    snapshot: &ConfigSnapshot,
    default_persist_dir: Option<&Path>,
) -> Result<Arc<Persistence>, EngineError> {
    let dir = snapshot
        .session_defaults()
        .persist_path()
        .map(PathBuf::from)
        .or_else(|| default_persist_dir.map(PathBuf::from));
    match dir {
        Some(dir) => {
            std::fs::create_dir_all(&dir)?;
            Ok(Arc::new(Persistence::open(dir.join(SESSION_DB_FILENAME))?))
        }
        None => Ok(crate::engine::in_memory_store()),
    }
}

/// The session ↔ agent binding resolved from a wire [`SessionConfig`] and the
/// current configuration snapshot (`docs/CLI.md` §4.4; see the
/// [`Engine::from_config`] rustdoc for the full contract).
///
/// A binding is resolved every time a session actor spawns — on `create_session`
/// and on `resume_session` — so a session (re)built after a configuration
/// update picks up the current snapshot. Fields left unset by the bound entry
/// are `None`, and the caller falls back to the wire `SessionConfig` values.
#[derive(Clone, Debug)]
pub(crate) struct SessionBinding {
    agent_name: String,
    model: Option<String>,
    tools: Option<Vec<String>>,
    system_prompt: Option<String>,
    /// Effective per-run budget: explicit wire budget, else the bound entry's,
    /// else the `[session]` default; `None` when all are unset.
    budget: Option<SessionBudget>,
    /// The TOML layer of the session's agent-definition table
    /// (`docs/dyn-agents.md` §3.2 source 4): every other configured
    /// `agents.<name>` entry as a local definition plus every
    /// `external_agents.<name>` ACP entry as an external one. The session
    /// driver merges it over the builtin/user/project layers; the bound entry
    /// itself is excluded.
    agent_definitions: AgentDefinitionRegistry,
    /// The session's configured default subagent toolset
    /// (`[session].default_subagent_tools`, `docs/dyn-agents.md` §7): seeds
    /// the shared spawn state; a definition without `tools` falls back to it
    /// (and then to the full session registry when unset).
    default_subagent_tools: Option<Vec<String>>,
}

impl SessionBinding {
    /// Resolves the binding for `config` against `snapshot` (`None` for an
    /// engine without a configuration backend — every field falls back to the
    /// wire `SessionConfig`).
    pub(crate) fn resolve(config: &SessionConfig, snapshot: Option<&ConfigSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self {
                agent_name: DEFAULT_AGENT_NAME.to_owned(),
                model: None,
                tools: None,
                system_prompt: None,
                budget: config.budget,
                agent_definitions: AgentDefinitionRegistry::default(),
                default_subagent_tools: None,
            };
        };

        let requested = config.provider.trim();
        let default_name = default_agent_name(snapshot);
        let agent_name = if requested.is_empty() {
            default_name.to_owned()
        } else if snapshot.agent(requested).is_some() {
            requested.to_owned()
        } else {
            tracing::warn!(
                provider = requested,
                fallback = default_name,
                "session binding names no configured agent entry; falling back to the default agent"
            );
            default_name.to_owned()
        };
        let entry = snapshot.agent(&agent_name);

        let tools = entry.and_then(|agent| agent.tools_list()).map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.is_enabled())
                .map(|tool| tool.name().to_owned())
                .collect::<Vec<_>>()
        });
        let budget = config
            .budget
            .or_else(|| entry.and_then(|agent| agent.budget()).map(session_budget))
            .or_else(|| snapshot.session_defaults().budget().map(session_budget));

        // Every other configured agent entry and every external ACP entry
        // becomes a spawnable agent definition (`docs/dyn-agents.md` §3.2);
        // the bound entry is excluded from its own table.
        let agent_definitions = AgentDefinitionRegistry::from_toml_snapshot(snapshot, &agent_name);

        Self {
            agent_name,
            model: entry.and_then(|agent| agent.model().map(str::to_owned)),
            tools,
            system_prompt: entry.and_then(|agent| agent.system_prompt().map(str::to_owned)),
            budget,
            agent_definitions,
            default_subagent_tools: snapshot
                .session_defaults()
                .default_subagent_tools()
                .map(<[String]>::to_vec),
        }
    }

    /// The `agents.<name>` entry this session is bound to; consulted by
    /// `apply_config` reconfigurations.
    pub(crate) fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// The bound entry's model override, when it sets one.
    pub(crate) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The bound entry's enabled tool list, when it constrains the surface.
    /// `Some(&[])` is a real constraint (an explicit `tools = []` exposes no
    /// tools); `None` means the entry sets no `tools` key and the surface is
    /// unconstrained.
    pub(crate) fn tools(&self) -> Option<&[String]> {
        self.tools.as_deref()
    }

    /// The bound entry's system prompt override, when it sets one.
    pub(crate) fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// The effective per-run budget (explicit wire budget first, then the
    /// bound entry's, then the `[session]` default).
    pub(crate) fn budget(&self) -> Option<SessionBudget> {
        self.budget
    }

    /// The TOML layer of the session's agent-definition table
    /// (`docs/dyn-agents.md` §3.2 source 4); empty for an engine without a
    /// configuration backend. The session driver merges it over the
    /// builtin/user/project layers and rebuilds it on `apply_config` (M3-5).
    pub(crate) fn agent_definitions(&self) -> &AgentDefinitionRegistry {
        &self.agent_definitions
    }

    /// The session's configured default subagent toolset
    /// (`[session].default_subagent_tools`, `docs/dyn-agents.md` §7); `None`
    /// leaves definitions without `tools` on the full session registry.
    pub(crate) fn default_subagent_tools(&self) -> Option<&[String]> {
        self.default_subagent_tools.as_deref()
    }
}

/// Projects a resolved configuration [`Budget`](mag_config::Budget) onto the
/// wire [`SessionBudget`] shape `SessionDriver` already consumes.
fn session_budget(budget: mag_config::Budget) -> SessionBudget {
    SessionBudget {
        max_steps: budget.max_steps(),
        max_tokens: budget.max_tokens(),
        max_cost_micros: budget.max_cost_micros(),
        max_wall_time_secs: budget.max_wall_time_secs(),
    }
}

/// Approval strategy derived from the configuration snapshot (`[approval]` +
/// `[tools.<name>].approval`, `docs/CLI.md` §4.2).
///
/// agent-lib bakes the `ApprovalPolicy` into a facade agent at build time and
/// offers no reconfigure variant, so the strongest possible semantics is
/// **derived fresh at every session (re)build** (actor spawn), which is where
/// this value is computed — an approval change takes effect for newly built
/// sessions without an engine restart (`docs/CLI.md` §4.4's "policy assembled
/// at run start" degraded to agent-lib's build-time surface, recorded in
/// M3-5).
#[derive(Clone, Debug, Default)]
pub(crate) struct ApprovalOverrides {
    default_tier: ApprovalPolicyKind,
    per_tool: BTreeMap<String, ApprovalPolicyKind>,
}

impl ApprovalOverrides {
    /// Derives the overrides from `snapshot`: `[approval].default_policy`
    /// (effective default `allow`, matching agent-lib's own default tier) plus
    /// every `[tools.<name>].approval` entry. `approval.timeout_secs` has no
    /// agent-lib surface yet and is intentionally not projected (recorded for
    /// M3-R).
    pub(crate) fn from_snapshot(snapshot: &ConfigSnapshot) -> Self {
        Self {
            default_tier: snapshot.approval().effective_default_policy(),
            per_tool: snapshot
                .tools()
                .values()
                .filter_map(|tool| tool.approval().map(|kind| (tool.name().to_owned(), kind)))
                .collect(),
        }
    }

    /// The whole-agent default tier.
    pub(crate) fn default_tier(&self) -> ApprovalPolicyKind {
        self.default_tier
    }

    /// Per-tool tier overrides, keyed by tool name.
    pub(crate) fn per_tool(&self) -> &BTreeMap<String, ApprovalPolicyKind> {
        &self.per_tool
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use mag_config::ConfigDto;
    use mag_service::{MagService, RoutingMode, ServiceError, UserInput};

    use super::*;

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// `TempConfigDir` helper in `config.rs` tests).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "mag-asm-{tag}-{}-{nanos}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn service_for(toml: &str) -> (TempDir, Arc<ConfigService>) {
        let dir = TempDir::new("svc");
        fs::write(dir.config_path(), toml).expect("write config");
        let service = ConfigService::load_or_default(dir.config_path()).expect("load config");
        (dir, Arc::new(service))
    }

    fn snapshot(toml: &str) -> ConfigSnapshot {
        let dto = ConfigDto::parse_str(toml).expect("config parses");
        ConfigSnapshot::resolve(&dto, 0).expect("config resolves")
    }

    fn session_config(provider: &str, model: &str) -> SessionConfig {
        SessionConfig {
            provider: provider.to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    /// The `docs/CLI.md` §4.2 example config, with the secret pointed at a
    /// test-only environment variable.
    #[cfg(feature = "external-acp")]
    const SAMPLE_TOML: &str = r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "MAG_ASM_TEST_SAMPLE_API_KEY" }

[providers.local_proxy]
wire = "openai"
base_url = "http://127.0.0.1:8317"
api_key = { keyring = "mag/local_proxy" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "list_dir", "grep", "shell"]

[agents.reviewer]
provider = "local_proxy"
model = "gpt-5-codex"
tools = ["read_file", "grep"]

[external_agents.peer_acp]
kind = "acp"
command = ["peer-agent", "--acp"]

[tools.shell]
approval = "ask"

[session]
routing = "model_routed"
budget = { max_tokens = 200000 }
"#;

    /// Sets a test-only environment variable for the duration of a closure
    /// (unique names per test, so parallel tests never interfere). `unsafe`
    /// because environment mutation is process-global (edition 2024).
    #[cfg(feature = "external-acp")]
    fn with_env_var<T>(name: &str, value: &str, test: impl FnOnce() -> T) -> T {
        unsafe { std::env::set_var(name, value) };
        let result = test();
        unsafe { std::env::remove_var(name) };
        result
    }

    #[cfg(feature = "external-acp")]
    #[test]
    fn from_config_assembles_the_sample_config() {
        with_env_var("MAG_ASM_TEST_SAMPLE_API_KEY", "sk-asm-test", || {
            let (_dir, service) = service_for(SAMPLE_TOML);

            let engine = Engine::from_config(service).expect("sample config assembles");

            // Sources registry: every provider + the external agent slot.
            let llm_ids: Vec<&str> = engine
                .sources()
                .llm_sources()
                .iter()
                .map(|source| source.id())
                .collect();
            assert_eq!(llm_ids, vec!["anthropic", "local_proxy"]);
            let slot_ids: Vec<&str> = engine
                .sources()
                .local_agents()
                .iter()
                .map(|slot| slot.id())
                .collect();
            assert_eq!(slot_ids, vec!["peer_acp"]);

            // The assembled client is live (session creation works), and the
            // configuration surface projects the snapshot losslessly — the
            // secret stays in reference form.
            let dto = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(engine.get_config())
                .expect("get config");
            let original = ConfigDto::parse_str(SAMPLE_TOML).expect("sample parses");
            assert_eq!(dto, original);
        });
    }

    #[cfg(not(feature = "external-acp"))]
    #[test]
    fn from_config_rejects_external_acp_config_when_feature_is_disabled() {
        let (_dir, service) = service_for(
            r#"
[external_agents.peer]
kind = "acp"
command = ["peer-agent", "--acp"]
"#,
        );

        let error = Engine::from_config(service).expect_err("external ACP requires a feature");

        let message = error.to_string();
        assert!(
            matches!(error, EngineError::ExternalAgentUnsupported { .. }),
            "expected ExternalAgentUnsupported, got: {message}"
        );
        assert!(message.contains("peer"), "agent is named: {message}");
        assert!(message.contains("acp"), "kind is named: {message}");
        assert!(
            message.contains("external-acp"),
            "required feature is named: {message}"
        );
    }

    #[tokio::test]
    async fn from_config_with_a_missing_file_yields_a_clientless_engine() {
        let dir = TempDir::new("missing");
        let service = Arc::new(
            ConfigService::load_or_default(dir.config_path()).expect("missing file yields default"),
        );

        let engine = Engine::from_config(service).expect("default config assembles");

        assert!(engine.sources().llm_sources().is_empty());
        assert!(engine.sources().local_agents().is_empty());
        let session = engine
            .create_session(session_config("default", "any-model"))
            .await
            .expect("create session");
        let error = engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect_err("clientless engine cannot run");
        assert!(
            matches!(error, ServiceError::Backend { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn from_config_reports_a_missing_env_secret_with_the_reference_name() {
        // Deliberately never set: the error must name this variable.
        let config = r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "MAG_ASM_TEST_DEFINITELY_MISSING_SECRET" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
"#;
        let (_dir, service) = service_for(config);

        let error = Engine::from_config(service).expect_err("missing secret fails assembly");

        let message = error.to_string();
        assert!(
            matches!(error, EngineError::Secret { .. }),
            "expected Secret, got: {message}"
        );
        assert!(message.contains("anthropic"), "provider named: {message}");
        assert!(
            message.contains("MAG_ASM_TEST_DEFINITELY_MISSING_SECRET"),
            "reference named: {message}"
        );
        assert!(!message.contains("sk-"), "no secret material: {message}");
    }

    #[test]
    fn from_config_reports_a_keyring_reference_as_unsupported() {
        let config = r#"
[providers.local_proxy]
wire = "openai"
base_url = "http://127.0.0.1:8317"
api_key = { keyring = "mag/local_proxy" }

[agents.default]
provider = "local_proxy"
model = "gpt-5-codex"
"#;
        let (_dir, service) = service_for(config);

        let error = Engine::from_config(service).expect_err("keyring reference fails assembly");

        let message = error.to_string();
        assert!(
            matches!(error, EngineError::Secret { .. }),
            "expected Secret, got: {message}"
        );
        assert!(message.contains("local_proxy"), "provider named: {message}");
        assert!(
            message.contains("keyring"),
            "reference kind named: {message}"
        );
        assert!(
            message.contains("mag/local_proxy"),
            "entry named: {message}"
        );
    }

    #[tokio::test]
    async fn from_config_uses_the_configured_persist_path() {
        let dir = TempDir::new("persist");
        let persist_dir = dir.0.join("data");
        let config = format!(
            "[session]\npersist_path = {:?}\n",
            persist_dir.to_string_lossy()
        );
        fs::write(dir.config_path(), &config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));

        let engine = Engine::from_config(service).expect("assembles");
        engine
            .create_session(session_config("default", "any-model"))
            .await
            .expect("create session");

        assert!(
            persist_dir.join(SESSION_DB_FILENAME).exists(),
            "session store created under the configured persist_path"
        );
    }

    #[tokio::test]
    async fn from_config_with_default_persist_path_persists_without_a_config_key() {
        let dir = TempDir::new("default-persist");
        let persist_dir = dir.0.join("data");
        // No config file at all: the default configuration sets no
        // [session].persist_path, so the caller-supplied default applies.
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));

        let engine =
            Engine::from_config_with_default_persist_path(Arc::clone(&service), &persist_dir)
                .expect("assembles");
        let session = engine
            .create_session(session_config("default", "any-model"))
            .await
            .expect("create session");

        assert!(
            persist_dir.join(SESSION_DB_FILENAME).exists(),
            "session store created under the default persist dir"
        );

        // A second engine over the same default dir finds the session
        // persisted by the first (the store is the listing's source of truth;
        // resuming would additionally need an LLM client this config lacks).
        drop(engine);
        let engine2 = Engine::from_config_with_default_persist_path(service, &persist_dir)
            .expect("assembles");
        let sessions = engine2.list_sessions().await.expect("list sessions");
        assert!(
            sessions.iter().any(|info| info.id == session),
            "persisted session visible to a restarted engine"
        );
    }

    #[tokio::test]
    async fn configured_persist_path_wins_over_the_default() {
        let dir = TempDir::new("persist-wins");
        let configured = dir.0.join("configured");
        let fallback = dir.0.join("fallback");
        let config = format!(
            "[session]\npersist_path = {:?}\n",
            configured.to_string_lossy()
        );
        fs::write(dir.config_path(), &config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));

        let engine =
            Engine::from_config_with_default_persist_path(service, &fallback).expect("assembles");
        engine
            .create_session(session_config("default", "any-model"))
            .await
            .expect("create session");

        assert!(
            configured.join(SESSION_DB_FILENAME).exists(),
            "configured persist_path used"
        );
        assert!(
            !fallback.join(SESSION_DB_FILENAME).exists(),
            "default dir untouched when persist_path is configured"
        );
    }

    #[test]
    fn assemble_tools_filters_disabled_entries_and_tolerates_unknown_names() {
        let snapshot = snapshot(
            r#"
[tools.shell]
enabled = false

[tools.ghost]
approval = "ask"
"#,
        );

        let registry = assemble_tool_registry(&snapshot);

        let names: Vec<&str> = registry.plugins().iter().map(|p| p.name()).collect();
        assert!(!names.contains(&"shell"), "shell disabled: {names:?}");
        assert!(names.contains(&"read_file"), "builtins kept: {names:?}");
    }

    #[test]
    fn session_binding_resolves_entries_fallbacks_and_budget_precedence() {
        let snapshot = snapshot(
            r#"
[agents.default]
model = "model-d"
tools = ["read_file"]

[agents.reviewer]
model = "model-r"
system_prompt = "Review carefully."
budget = { max_tokens = 1000 }

[session]
budget = { max_steps = 5 }
"#,
        );

        // Empty provider → default entry.
        let binding = SessionBinding::resolve(&session_config("", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "default");
        assert_eq!(binding.model(), Some("model-d"));
        assert_eq!(binding.tools(), Some(&["read_file".to_owned()][..]));
        // The default entry has no budget → falls back to [session].budget.
        assert_eq!(binding.budget().and_then(|b| b.max_steps), Some(5));

        // A named entry binds directly.
        let binding =
            SessionBinding::resolve(&session_config("reviewer", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "reviewer");
        assert_eq!(binding.model(), Some("model-r"));
        assert_eq!(binding.system_prompt(), Some("Review carefully."));
        assert_eq!(binding.budget().and_then(|b| b.max_tokens), Some(1000));

        // An unknown provider name falls back to the default entry.
        let binding =
            SessionBinding::resolve(&session_config("ghost", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "default");

        // An explicit wire budget wins over every configured default.
        let mut config = session_config("reviewer", "wire-model");
        config.budget = Some(SessionBudget {
            max_tokens: Some(7),
            ..SessionBudget::default()
        });
        let binding = SessionBinding::resolve(&config, Some(&snapshot));
        assert_eq!(binding.budget().and_then(|b| b.max_tokens), Some(7));

        // No configuration backend: everything falls back to the wire config.
        let binding = SessionBinding::resolve(&config, None);
        assert_eq!(binding.agent_name(), "default");
        assert_eq!(binding.model(), None);
        assert_eq!(binding.budget().and_then(|b| b.max_tokens), Some(7));
    }

    /// `[session].default_agent` selects the entry an empty or unmatched
    /// provider binds to, in place of the well-known `default` entry.
    #[test]
    fn session_binding_honors_the_configured_default_agent() {
        let snapshot = snapshot(
            r#"
[agents.default]
model = "model-d"

[agents.main]
model = "model-m"

[session]
default_agent = "main"
"#,
        );

        // Empty provider binds the configured default agent, which is not
        // listed in its own definition table.
        let binding = SessionBinding::resolve(&session_config("", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "main");
        assert_eq!(binding.model(), Some("model-m"));
        assert!(
            binding.agent_definitions().get("main").is_none(),
            "the bound entry is excluded from its own definition table"
        );
        assert!(
            binding.agent_definitions().get("default").is_some(),
            "the other entry is a spawnable definition"
        );

        // An unknown name falls back to the configured default agent.
        let binding =
            SessionBinding::resolve(&session_config("ghost", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "main");

        // An explicitly named existing entry still binds directly.
        let binding =
            SessionBinding::resolve(&session_config("default", "wire-model"), Some(&snapshot));
        assert_eq!(binding.agent_name(), "default");
        assert_eq!(binding.model(), Some("model-d"));
    }

    /// An explicit `tools = []` constrains the session to *no* tools
    /// (`Some(&[])`), while an absent `tools` key leaves the surface
    /// unconstrained (`None`) — the two are not the same configuration.
    #[test]
    fn session_binding_distinguishes_an_explicit_empty_tool_list() {
        let snapshot = snapshot(
            r#"
[agents.default]

[agents.empty]
tools = []
"#,
        );

        let binding = SessionBinding::resolve(&session_config("default", "m"), Some(&snapshot));
        assert_eq!(binding.tools(), None, "no tools key: unconstrained");

        let binding = SessionBinding::resolve(&session_config("empty", "m"), Some(&snapshot));
        assert_eq!(
            binding.tools(),
            Some(&[][..]),
            "explicit empty list: no tools"
        );
    }

    /// Every configured `agents.<name>` entry except the session's bound one
    /// projects into the binding's TOML definition layer
    /// (`docs/dyn-agents.md` §3.2 source 4): the definition carries the
    /// entry's model/system/tools/role, a missing `role` falls back to a
    /// name-derived description, and disabled tools are filtered out exactly
    /// as on the bound entry.
    #[test]
    fn session_binding_projects_definitions_from_the_other_agent_entries() {
        use mag_config::{AgentKindDef, DefinitionSource};

        let snapshot = snapshot(
            r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
system_prompt = "Research thoroughly."
role = "Researches topics and reports findings."
tools = ["read_file", "shell"]

[agents.helper]

[tools.shell]
enabled = false
"#,
        );

        // Bound to `default`: `researcher` and `helper` are definitions.
        let binding = SessionBinding::resolve(&session_config("default", "m"), Some(&snapshot));
        let definitions = binding.agent_definitions();
        assert_eq!(definitions.len(), 2);
        let helper = definitions.get("helper").expect("helper definition");
        assert_eq!(helper.description, "Local subagent `helper`");
        assert_eq!(
            helper.kind,
            AgentKindDef::Local {
                model: None,
                tools: None,
                max_steps: None,
                allow_subagents: true,
            },
            "no model/tools keys: inherit and unconstrained"
        );
        assert_eq!(helper.source, DefinitionSource::Toml);
        let researcher = definitions
            .get("researcher")
            .expect("researcher definition");
        assert_eq!(
            researcher.description,
            "Researches topics and reports findings."
        );
        assert_eq!(researcher.body, "Research thoroughly.");
        assert_eq!(
            researcher.kind,
            AgentKindDef::Local {
                model: Some("model-r".to_owned()),
                tools: Some(vec!["read_file".to_owned()]),
                max_steps: None,
                allow_subagents: true,
            },
            "disabled tools are filtered from the definition surface"
        );
        assert!(
            definitions.get("default").is_none(),
            "the bound entry is excluded from its own definition table"
        );

        // Bound to `researcher`: `default` itself becomes a definition.
        let binding = SessionBinding::resolve(&session_config("researcher", "m"), Some(&snapshot));
        assert!(binding.agent_definitions().get("default").is_some());
        assert!(binding.agent_definitions().get("helper").is_some());
        assert!(binding.agent_definitions().get("researcher").is_none());

        // No configuration backend: no definitions.
        let binding = SessionBinding::resolve(&session_config("default", "m"), None);
        assert!(binding.agent_definitions().is_empty());
    }
}
