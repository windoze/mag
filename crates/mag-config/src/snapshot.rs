//! Domain objects (DO): the resolved `Arc` object graph the program actually
//! uses (`docs/CLI.md` §4.2, decision D4).
//!
//! The DTO layer ([`ConfigDto`]) is a serde mirror of any configuration
//! source; the DO layer is what sessions pin and engines assemble from.
//! [`ConfigSnapshot::resolve`] validates and normalizes a DTO into an
//! immutable, reference-counted object tree; [`ConfigSnapshot::project`]
//! writes the tree back to a DTO losslessly (secret references stay
//! references — values are never materialized, `docs/CLI.md` §4.1).
//!
//! # Naming
//!
//! Type names follow the §4.2 structure diagram (`ConfigSnapshot`,
//! `Arc<ResolvedProvider>`, `Arc<ResolvedAgent>`, extended consistently to
//! `ResolvedExternalAgent` / `ResolvedTool`). The TODO M3-2 task book lists
//! the same nodes as `LlmConfig` / `AgentConfig` / `ExternalAgentConfig` /
//! `ToolConfig` — the correspondences are:
//!
//! | §4.2 diagram (this module) | M3-2 task book | DTO counterpart |
//! |----------------------------|----------------|-----------------|
//! | [`ConfigSnapshot`]         | `ConfigSnapshot` | [`ConfigDto`] |
//! | [`ResolvedProvider`]       | `LlmConfig`    | [`ProviderDto`] |
//! | [`ResolvedAgent`]          | `AgentConfig`  | [`AgentDto`] |
//! | [`ResolvedExternalAgent`]  | `ExternalAgentConfig` | [`ExternalAgentDto`] |
//! | [`ResolvedTool`]           | `ToolConfig`   | [`ToolDto`] |
//! | [`SessionDefaults`]        | `SessionDefaults` | [`SessionDefaultsDto`] |
//! | [`ApprovalConfig`]         | `ApprovalConfig` | [`ApprovalSectionDto`] |
//!
//! # Snapshot isolation
//!
//! A session pins an `Arc<ConfigSnapshot>` at creation time; a later
//! `update_config` resolves a **new** snapshot (new nodes, bumped
//! `revision`). Nodes are immutable once built, so the old snapshot keeps
//! serving pinned sessions unchanged (`docs/CLI.md` §4.4, decision D2).
//! Cloning a snapshot is cheap: it copies `Arc` handles, never node data.
//!
//! # Raw vs. effective values
//!
//! Nodes keep the DTO's raw `Option` fields so projection is lossless (an
//! unset key stays unset; no ghost entries appear on write-back). Defaults
//! are filled through `effective_*` / `is_*` accessors, documented per field.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use crate::dto::{
    AgentDto, ApprovalSectionDto, BudgetDto, ExternalAgentDto, ProviderDto, SessionDefaultsDto,
    ToolDto,
};
use crate::{ConfigDto, ConfigError, SecretRef};

/// The wire protocol a provider speaks (`[providers.<name>].wire`).
///
/// Mirrors the protocols agent-lib adapters implement (Anthropic Messages,
/// OpenAI Responses); validated at resolve time (`docs/CLI.md` §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderWire {
    /// Anthropic Messages protocol.
    Anthropic,
    /// OpenAI Responses protocol.
    OpenAi,
}

impl ProviderWire {
    /// The canonical config string (`"anthropic"` / `"openai"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }
}

impl fmt::Display for ProviderWire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderWire {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown wire protocol {other:?} (expected \"anthropic\" or \"openai\")"
            )),
        }
    }
}

/// An approval policy tier (`ask` | `allow` | `deny`), mapped to agent-lib's
/// `ApprovalPolicy` at assembly time (`docs/CLI.md` §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalPolicyKind {
    /// Pause and prompt the user before running the tool.
    Ask,
    /// Run without prompting.
    Allow,
    /// Refuse the call outright.
    Deny,
}

impl ApprovalPolicyKind {
    /// The canonical config string (`"ask"` / `"allow"` / `"deny"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

impl Default for ApprovalPolicyKind {
    /// Matches agent-lib's `ApprovalPolicy::default()` tier (`auto_allow`):
    /// typed tools run by default unless a config entry opts them into `ask`.
    fn default() -> Self {
        Self::Allow
    }
}

impl fmt::Display for ApprovalPolicyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApprovalPolicyKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ask" => Ok(Self::Ask),
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => Err(format!(
                "unknown approval policy {other:?} (expected \"ask\", \"allow\", or \"deny\")"
            )),
        }
    }
}

/// Delegation routing mode (`[session].routing`).
///
/// String forms mirror the service contract's `RoutingMode` serde names, so
/// the DO→wire projection at assembly time is a direct enum mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoutingModeKind {
    /// Supervisor model decides when and where to delegate
    /// (`"model_routed"`; matches the service contract's default).
    #[default]
    ModelRouted,
    /// A dispatcher component decides which delegate receives a task
    /// (`"dispatcher"`).
    Dispatcher,
}

impl RoutingModeKind {
    /// The canonical config string (`"model_routed"` / `"dispatcher"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ModelRouted => "model_routed",
            Self::Dispatcher => "dispatcher",
        }
    }
}

impl fmt::Display for RoutingModeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RoutingModeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "model_routed" => Ok(Self::ModelRouted),
            "dispatcher" => Ok(Self::Dispatcher),
            other => Err(format!(
                "unknown routing mode {other:?} (expected \"model_routed\" or \"dispatcher\")"
            )),
        }
    }
}

/// External agent source kind (`[external_agents.<name>].kind`, decision D3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExternalAgentKind {
    /// Agent Client Protocol subprocess (`"acp"`; currently the only kind).
    #[default]
    Acp,
}

impl ExternalAgentKind {
    /// The canonical config string (`"acp"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Acp => "acp",
        }
    }
}

impl fmt::Display for ExternalAgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExternalAgentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "acp" => Ok(Self::Acp),
            other => Err(format!(
                "unknown external agent kind {other:?} (expected \"acp\")"
            )),
        }
    }
}

/// Per-run budget limits (DO counterpart of [`BudgetDto`]).
///
/// Field-by-field projection of the DTO type; every dimension stays optional,
/// an unset dimension is unbounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    max_steps: Option<u64>,
    max_tokens: Option<u64>,
    max_cost_micros: Option<u64>,
    max_wall_time_secs: Option<u64>,
}

impl Budget {
    /// Maximum agent steps per run (`None` = unbounded).
    pub fn max_steps(&self) -> Option<u64> {
        self.max_steps
    }

    /// Maximum total tokens per run (`None` = unbounded).
    pub fn max_tokens(&self) -> Option<u64> {
        self.max_tokens
    }

    /// Maximum cost per run in micro-units (`None` = unbounded).
    pub fn max_cost_micros(&self) -> Option<u64> {
        self.max_cost_micros
    }

    /// Maximum wall-clock time per run in seconds (`None` = unbounded).
    pub fn max_wall_time_secs(&self) -> Option<u64> {
        self.max_wall_time_secs
    }

    /// Returns true when every dimension is unset.
    pub fn is_empty(&self) -> bool {
        self.max_steps.is_none()
            && self.max_tokens.is_none()
            && self.max_cost_micros.is_none()
            && self.max_wall_time_secs.is_none()
    }

    /// Projects back to the DTO form (field-by-field, lossless).
    fn to_dto(self) -> BudgetDto {
        BudgetDto {
            max_steps: self.max_steps,
            max_tokens: self.max_tokens,
            max_cost_micros: self.max_cost_micros,
            max_wall_time_secs: self.max_wall_time_secs,
        }
    }
}

impl From<BudgetDto> for Budget {
    fn from(dto: BudgetDto) -> Self {
        Self {
            max_steps: dto.max_steps,
            max_tokens: dto.max_tokens,
            max_cost_micros: dto.max_cost_micros,
            max_wall_time_secs: dto.max_wall_time_secs,
        }
    }
}

impl From<Budget> for BudgetDto {
    fn from(budget: Budget) -> Self {
        budget.to_dto()
    }
}

/// A resolved LLM provider node (`docs/CLI.md` §4.2 diagram
/// `Arc<ResolvedProvider>`; the M3-2 task book calls this node `LlmConfig`).
///
/// `wire` is required and validated at resolve time — a provider without a
/// protocol cannot be assembled. The secret stays a [`SecretRef`]; resolving
/// it to a value is the assembly layer's job (`Engine::from_config`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProvider {
    name: String,
    wire: ProviderWire,
    base_url: Option<String>,
    api_key: Option<SecretRef>,
    params: Option<BTreeMap<String, toml::Value>>,
}

impl ResolvedProvider {
    /// The provider's section name (`[providers.<name>]`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The wire protocol, validated at resolve time.
    pub fn wire(&self) -> ProviderWire {
        self.wire
    }

    /// Endpoint base URL, when configured.
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// API key **reference**, when configured. Never a value.
    pub fn api_key(&self) -> Option<&SecretRef> {
        self.api_key.as_ref()
    }

    /// Free-form provider parameter table, when configured.
    pub fn params(&self) -> Option<&BTreeMap<String, toml::Value>> {
        self.params.as_ref()
    }

    fn to_dto(&self) -> ProviderDto {
        ProviderDto {
            wire: Some(self.wire.as_str().to_string()),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            params: self.params.clone(),
        }
    }
}

/// A resolved per-tool override node (`docs/CLI.md` §4.2 diagram `tools`
/// node; the M3-2 task book calls this node `ToolConfig`).
///
/// Two flavours exist in a snapshot: *explicit* nodes come from
/// `[tools.<name>]` entries and live in [`ConfigSnapshot::tools`]; *implicit*
/// nodes are synthesized for tool names an agent references without a
/// `[tools.<name>]` override (all fields unset, i.e. defaults). Implicit
/// nodes never appear in the projected DTO's `tools` map, keeping the
/// DTO→DO→DTO round-trip lossless.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTool {
    name: String,
    approval: Option<ApprovalPolicyKind>,
    enabled: Option<bool>,
}

impl ResolvedTool {
    /// The tool name this override applies to.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Raw approval override, when the config sets one.
    pub fn approval(&self) -> Option<ApprovalPolicyKind> {
        self.approval
    }

    /// Raw enabled flag, when the config sets one.
    pub fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    /// Effective enabled flag: tools are enabled unless explicitly disabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn to_dto(&self) -> ToolDto {
        ToolDto {
            approval: self.approval.map(|a| a.as_str().to_string()),
            enabled: self.enabled,
        }
    }
}

/// A resolved local agent node (`docs/CLI.md` §4.2 diagram
/// `Arc<ResolvedAgent>`; the M3-2 task book calls this node `AgentConfig`).
///
/// Cross references are resolved into shared `Arc` handles: [`provider`]
/// points at the same node as [`ConfigSnapshot::providers`], and each entry
/// of [`tools`] points at either an explicit [`ConfigSnapshot::tools`] node
/// or a per-name shared implicit node (the §4.2 "object tree/forest" — not a
/// nested value copy).
///
/// [`provider`]: ResolvedAgent::provider
/// [`tools`]: ResolvedAgent::tools
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAgent {
    name: String,
    provider: Option<Arc<ResolvedProvider>>,
    model: Option<String>,
    tools: Option<Vec<Arc<ResolvedTool>>>,
    system_prompt: Option<String>,
    role: Option<String>,
    budget: Option<Budget>,
}

impl ResolvedAgent {
    /// The agent's section name (`[agents.<name>]`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved provider backing this agent, when configured. Shares the
    /// `Arc` with [`ConfigSnapshot::providers`].
    pub fn provider(&self) -> Option<&Arc<ResolvedProvider>> {
        self.provider.as_ref()
    }

    /// Model identifier passed to the provider, when configured.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Resolved tool nodes exposed to this agent (empty slice when the config
    /// sets no tool list). Explicit overrides share the `Arc` with
    /// [`ConfigSnapshot::tools`]; names without an override share a
    /// synthesized default node per name.
    ///
    /// This accessor collapses "no `tools` key" and an explicit `tools = []`
    /// into the same empty slice; use [`tools_list`](Self::tools_list) when
    /// the distinction matters (an absent list imposes no constraint, an
    /// explicit empty list constrains the agent to *no* tools).
    pub fn tools(&self) -> &[Arc<ResolvedTool>] {
        self.tools.as_deref().unwrap_or(&[])
    }

    /// The tool list exactly as configured: `None` when the DTO sets no
    /// `tools` key (the agent imposes no tool constraint), `Some` — possibly
    /// empty — when the key is present. An explicit `tools = []` therefore
    /// reads as "expose no tools", not "unconstrained".
    pub fn tools_list(&self) -> Option<&[Arc<ResolvedTool>]> {
        self.tools.as_deref()
    }

    /// System prompt override, when configured.
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Free-form role label, when configured.
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// Per-agent budget override, when configured (falls back to
    /// [`SessionDefaults::budget`]).
    pub fn budget(&self) -> Option<Budget> {
        self.budget
    }

    fn to_dto(&self) -> AgentDto {
        AgentDto {
            provider: self.provider.as_ref().map(|p| p.name().to_string()),
            model: self.model.clone(),
            tools: self
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(|t| t.name().to_string()).collect()),
            system_prompt: self.system_prompt.clone(),
            role: self.role.clone(),
            budget: self.budget.map(Budget::to_dto),
        }
    }
}

/// A resolved external agent source node (`docs/CLI.md` §4.2 diagram
/// `external_agents` node, decision D3; the M3-2 task book calls this node
/// `ExternalAgentConfig`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedExternalAgent {
    name: String,
    kind: Option<ExternalAgentKind>,
    command: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    capabilities: Option<Vec<String>>,
}

impl ResolvedExternalAgent {
    /// The source's section name (`[external_agents.<name>]`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Raw kind, when the config sets one.
    pub fn kind(&self) -> Option<ExternalAgentKind> {
        self.kind
    }

    /// Effective kind: defaults to [`ExternalAgentKind::Acp`].
    pub fn effective_kind(&self) -> ExternalAgentKind {
        self.kind.unwrap_or_default()
    }

    /// Spawn command line in argv form (empty slice when unset).
    pub fn command(&self) -> &[String] {
        self.command.as_deref().unwrap_or(&[])
    }

    /// Extra environment variables for the spawned process, when configured.
    pub fn env(&self) -> Option<&BTreeMap<String, String>> {
        self.env.as_ref()
    }

    /// Advertised capability tags (empty slice when unset).
    pub fn capabilities(&self) -> &[String] {
        self.capabilities.as_deref().unwrap_or(&[])
    }

    fn to_dto(&self) -> ExternalAgentDto {
        ExternalAgentDto {
            kind: self.kind.map(|k| k.as_str().to_string()),
            command: self.command.clone(),
            env: self.env.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

/// Resolved session defaults (DO counterpart of [`SessionDefaultsDto`]).
///
/// Only affects *new* sessions (`docs/CLI.md` §4.4): existing sessions pin
/// the snapshot captured at creation time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionDefaults {
    routing: Option<RoutingModeKind>,
    persist_path: Option<PathBuf>,
    budget: Option<Budget>,
    default_agent: Option<String>,
}

impl SessionDefaults {
    /// An all-unset node, used when the DTO has no `[session]` section.
    const EMPTY: Self = Self {
        routing: None,
        persist_path: None,
        budget: None,
        default_agent: None,
    };

    /// Raw routing mode, when the config sets one.
    pub fn routing(&self) -> Option<RoutingModeKind> {
        self.routing
    }

    /// Effective routing mode: defaults to
    /// [`RoutingModeKind::ModelRouted`] (the service contract's default).
    pub fn effective_routing(&self) -> RoutingModeKind {
        self.routing.unwrap_or_default()
    }

    /// Default persistence directory, when configured.
    pub fn persist_path(&self) -> Option<&PathBuf> {
        self.persist_path.as_ref()
    }

    /// Default per-run budget for new sessions, when configured.
    pub fn budget(&self) -> Option<Budget> {
        self.budget
    }

    /// Name of the `[agents.<name>]` entry new sessions bind to by default,
    /// when configured (`None` falls back to the `default` entry).
    pub fn default_agent(&self) -> Option<&str> {
        self.default_agent.as_deref()
    }

    fn to_dto(&self) -> SessionDefaultsDto {
        SessionDefaultsDto {
            routing: self.routing.map(|r| r.as_str().to_string()),
            persist_path: self.persist_path.clone(),
            budget: self.budget.map(Budget::to_dto),
            default_agent: self.default_agent.clone(),
        }
    }
}

/// Resolved approval defaults (DO counterpart of [`ApprovalSectionDto`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalConfig {
    default_policy: Option<ApprovalPolicyKind>,
    timeout_secs: Option<u64>,
}

impl ApprovalConfig {
    /// An all-unset node, used when the DTO has no `[approval]` section.
    const EMPTY: Self = Self {
        default_policy: None,
        timeout_secs: None,
    };

    /// Raw default policy, when the config sets one.
    pub fn default_policy(&self) -> Option<ApprovalPolicyKind> {
        self.default_policy
    }

    /// Effective default policy for tools without a `[tools.<name>]`
    /// override: defaults to [`ApprovalPolicyKind::Allow`], matching
    /// agent-lib's `ApprovalPolicy::default()` tier (`auto_allow`).
    pub fn effective_default_policy(&self) -> ApprovalPolicyKind {
        self.default_policy.unwrap_or_default()
    }

    /// Approval prompt timeout in seconds, when configured.
    pub fn timeout_secs(&self) -> Option<u64> {
        self.timeout_secs
    }

    fn to_dto(&self) -> ApprovalSectionDto {
        ApprovalSectionDto {
            default_policy: self.default_policy.map(|p| p.as_str().to_string()),
            timeout_secs: self.timeout_secs,
        }
    }
}

/// An immutable, reference-counted configuration object graph
/// (`docs/CLI.md` §4.2 diagram `ResolvedConfig / ConfigSnapshot`).
///
/// Built by [`ConfigSnapshot::resolve`] from a [`ConfigDto`] with a
/// caller-supplied `revision` stamp. Every node is shared through `Arc` and
/// never mutated after construction, so cloning the snapshot is cheap (`Arc`
/// copies only) and a pinned snapshot is unaffected by later updates
/// (snapshot isolation, decision D2).
#[derive(Clone, Debug)]
pub struct ConfigSnapshot {
    revision: u64,
    providers: BTreeMap<String, Arc<ResolvedProvider>>,
    agents: BTreeMap<String, Arc<ResolvedAgent>>,
    external_agents: BTreeMap<String, Arc<ResolvedExternalAgent>>,
    tools: BTreeMap<String, Arc<ResolvedTool>>,
    session_defaults: Option<Arc<SessionDefaults>>,
    approval: Option<Arc<ApprovalConfig>>,
}

impl ConfigSnapshot {
    /// Resolves a [`ConfigDto`] into an immutable DO graph (DTO→DO,
    /// `docs/CLI.md` §4.2).
    ///
    /// The pipeline is: structural [`ConfigDto::validate`] → semantic checks
    /// (enum strings, cross references) → instantiate `Arc` nodes in
    /// topological order (providers and tools before the agents referencing
    /// them) → stamp `revision`. Any failure aborts the whole resolve; no
    /// partial graph is produced.
    ///
    /// Validation rules, all reported as [`ConfigError::Validation`] with a
    /// dotted field path:
    ///
    /// - `providers.<name>.wire` — required; must be `"anthropic"` or
    ///   `"openai"`.
    /// - `agents.<name>.provider` — when set, must name an existing
    ///   `[providers.<name>]` entry (dangling references are rejected).
    /// - `tools.<name>.approval` / `approval.default_policy` — must be
    ///   `"ask"` / `"allow"` / `"deny"`.
    /// - `session.routing` — must be `"model_routed"` or `"dispatcher"`.
    /// - `session.default_agent` — when set, must name an existing
    ///   `[agents.<name>]` entry (dangling references are rejected).
    /// - `external_agents.<name>.kind` — must be `"acp"`.
    ///
    /// Tool names referenced by `agents.<name>.tools` are *not* required to
    /// have a `[tools.<name>]` entry: names without an override resolve to a
    /// shared default [`ResolvedTool`] (enabled, no approval override). The
    /// §4.2 example config relies on this (`read_file`, `grep`, … have no
    /// `[tools]` entry); existence against the actual tool registry is the
    /// assembly layer's job (`Engine::from_config`).
    pub fn resolve(dto: &ConfigDto, revision: u64) -> Result<Self, ConfigError> {
        dto.validate()?;

        // Providers first: agents hold Arc references into this map.
        let mut providers = BTreeMap::new();
        for (name, p) in &dto.providers {
            let path = format!("providers.{name}.wire");
            let wire = p
                .wire
                .as_deref()
                .ok_or_else(|| {
                    ConfigError::validation(
                        path.clone(),
                        "missing wire protocol (expected \"anthropic\" or \"openai\")",
                    )
                })?
                .parse::<ProviderWire>()
                .map_err(|message| ConfigError::validation(path, message))?;
            providers.insert(
                name.clone(),
                Arc::new(ResolvedProvider {
                    name: name.clone(),
                    wire,
                    base_url: p.base_url.clone(),
                    api_key: p.api_key.clone(),
                    params: p.params.clone(),
                }),
            );
        }

        // Explicit tool overrides.
        let mut tools = BTreeMap::new();
        for (name, t) in &dto.tools {
            let approval = t
                .approval
                .as_deref()
                .map(|s| {
                    s.parse::<ApprovalPolicyKind>().map_err(|message| {
                        ConfigError::validation(format!("tools.{name}.approval"), message)
                    })
                })
                .transpose()?;
            tools.insert(
                name.clone(),
                Arc::new(ResolvedTool {
                    name: name.clone(),
                    approval,
                    enabled: t.enabled,
                }),
            );
        }

        // External agent sources (independent of providers/agents).
        let mut external_agents = BTreeMap::new();
        for (name, e) in &dto.external_agents {
            let kind = e
                .kind
                .as_deref()
                .map(|s| {
                    s.parse::<ExternalAgentKind>().map_err(|message| {
                        ConfigError::validation(format!("external_agents.{name}.kind"), message)
                    })
                })
                .transpose()?;
            external_agents.insert(
                name.clone(),
                Arc::new(ResolvedExternalAgent {
                    name: name.clone(),
                    kind,
                    command: e.command.clone(),
                    env: e.env.clone(),
                    capabilities: e.capabilities.clone(),
                }),
            );
        }

        // Session defaults.
        let session_defaults = dto
            .session
            .as_ref()
            .map(|s| {
                let routing = s
                    .routing
                    .as_deref()
                    .map(|r| {
                        r.parse::<RoutingModeKind>()
                            .map_err(|message| ConfigError::validation("session.routing", message))
                    })
                    .transpose()?;
                Ok::<_, ConfigError>(Arc::new(SessionDefaults {
                    routing,
                    persist_path: s.persist_path.clone(),
                    budget: s.budget.map(Budget::from),
                    default_agent: s.default_agent.clone(),
                }))
            })
            .transpose()?;

        // Approval defaults.
        let approval = dto
            .approval
            .as_ref()
            .map(|a| {
                let default_policy = a
                    .default_policy
                    .as_deref()
                    .map(|p| {
                        p.parse::<ApprovalPolicyKind>().map_err(|message| {
                            ConfigError::validation("approval.default_policy", message)
                        })
                    })
                    .transpose()?;
                Ok::<_, ConfigError>(Arc::new(ApprovalConfig {
                    default_policy,
                    timeout_secs: a.timeout_secs,
                }))
            })
            .transpose()?;

        // Agents last: they reference providers and tools by name. Tool names
        // without an explicit override share one synthesized default node per
        // name (cached in `implicit_tools`).
        let mut implicit_tools: BTreeMap<String, Arc<ResolvedTool>> = BTreeMap::new();
        let mut agents = BTreeMap::new();
        for (name, a) in &dto.agents {
            let provider = a
                .provider
                .as_deref()
                .map(|p| {
                    providers.get(p).cloned().ok_or_else(|| {
                        ConfigError::validation(
                            format!("agents.{name}.provider"),
                            format!("unknown provider {p:?} (no [providers.{p}] entry)"),
                        )
                    })
                })
                .transpose()?;
            let tools_for_agent = a.tools.as_ref().map(|names| {
                names
                    .iter()
                    .map(|tool_name| {
                        tools.get(tool_name).cloned().unwrap_or_else(|| {
                            implicit_tools
                                .entry(tool_name.clone())
                                .or_insert_with(|| {
                                    Arc::new(ResolvedTool {
                                        name: tool_name.clone(),
                                        approval: None,
                                        enabled: None,
                                    })
                                })
                                .clone()
                        })
                    })
                    .collect::<Vec<_>>()
            });
            agents.insert(
                name.clone(),
                Arc::new(ResolvedAgent {
                    name: name.clone(),
                    provider,
                    model: a.model.clone(),
                    tools: tools_for_agent,
                    system_prompt: a.system_prompt.clone(),
                    role: a.role.clone(),
                    budget: a.budget.map(Budget::from),
                }),
            );
        }

        // `[session].default_agent` must name an existing agent entry (same
        // dangling-reference rule as `agents.<name>.provider`).
        if let Some(default_agent) = session_defaults
            .as_ref()
            .and_then(|s| s.default_agent.as_deref())
            && !agents.contains_key(default_agent)
        {
            return Err(ConfigError::validation(
                "session.default_agent",
                format!("unknown agent {default_agent:?} (no [agents.{default_agent}] entry)"),
            ));
        }

        Ok(Self {
            revision,
            providers,
            agents,
            external_agents,
            tools,
            session_defaults,
            approval,
        })
    }

    /// Projects the DO graph back to a [`ConfigDto`] (DO→DTO, `docs/CLI.md`
    /// §4.2 write-through).
    ///
    /// The projection is lossless: raw `Option` fields are preserved exactly
    /// (an unset key stays unset), secret fields keep their [`SecretRef`]
    /// form, and implicit tool nodes synthesized at resolve time never
    /// reappear as `[tools.<name>]` entries. `revision` is DO-only metadata
    /// and is not projected.
    pub fn project(&self) -> ConfigDto {
        ConfigDto {
            providers: self
                .providers
                .iter()
                .map(|(name, p)| (name.clone(), p.to_dto()))
                .collect(),
            agents: self
                .agents
                .iter()
                .map(|(name, a)| (name.clone(), a.to_dto()))
                .collect(),
            external_agents: self
                .external_agents
                .iter()
                .map(|(name, e)| (name.clone(), e.to_dto()))
                .collect(),
            tools: self
                .tools
                .iter()
                .map(|(name, t)| (name.clone(), t.to_dto()))
                .collect(),
            session: self.session_defaults.as_ref().map(|s| s.to_dto()),
            approval: self.approval.as_ref().map(|a| a.to_dto()),
        }
    }

    /// The revision stamp supplied at resolve time (`docs/CLI.md` §4.3:
    /// monotonically increasing per successful apply).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// All resolved providers, keyed by name.
    pub fn providers(&self) -> &BTreeMap<String, Arc<ResolvedProvider>> {
        &self.providers
    }

    /// Looks up a resolved provider by name.
    pub fn provider(&self, name: &str) -> Option<&Arc<ResolvedProvider>> {
        self.providers.get(name)
    }

    /// All resolved local agents, keyed by name.
    pub fn agents(&self) -> &BTreeMap<String, Arc<ResolvedAgent>> {
        &self.agents
    }

    /// Looks up a resolved local agent by name.
    pub fn agent(&self, name: &str) -> Option<&Arc<ResolvedAgent>> {
        self.agents.get(name)
    }

    /// All resolved external agent sources, keyed by name.
    pub fn external_agents(&self) -> &BTreeMap<String, Arc<ResolvedExternalAgent>> {
        &self.external_agents
    }

    /// Looks up a resolved external agent source by name.
    pub fn external_agent(&self, name: &str) -> Option<&Arc<ResolvedExternalAgent>> {
        self.external_agents.get(name)
    }

    /// All explicit per-tool overrides, keyed by tool name. Implicit default
    /// nodes synthesized for agent tool references are not listed here.
    pub fn tools(&self) -> &BTreeMap<String, Arc<ResolvedTool>> {
        &self.tools
    }

    /// Looks up an explicit per-tool override by tool name.
    pub fn tool(&self, name: &str) -> Option<&Arc<ResolvedTool>> {
        self.tools.get(name)
    }

    /// Session defaults; an all-unset node when the config has no `[session]`
    /// section.
    pub fn session_defaults(&self) -> &SessionDefaults {
        self.session_defaults
            .as_deref()
            .unwrap_or(&SessionDefaults::EMPTY)
    }

    /// Approval defaults; an all-unset node when the config has no
    /// `[approval]` section.
    pub fn approval(&self) -> &ApprovalConfig {
        self.approval.as_deref().unwrap_or(&ApprovalConfig::EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_strings_round_trip() {
        for (s, wire) in [
            ("anthropic", ProviderWire::Anthropic),
            ("openai", ProviderWire::OpenAi),
        ] {
            assert_eq!(s.parse::<ProviderWire>(), Ok(wire));
            assert_eq!(wire.as_str(), s);
        }
        for (s, policy) in [
            ("ask", ApprovalPolicyKind::Ask),
            ("allow", ApprovalPolicyKind::Allow),
            ("deny", ApprovalPolicyKind::Deny),
        ] {
            assert_eq!(s.parse::<ApprovalPolicyKind>(), Ok(policy));
            assert_eq!(policy.as_str(), s);
        }
        for (s, routing) in [
            ("model_routed", RoutingModeKind::ModelRouted),
            ("dispatcher", RoutingModeKind::Dispatcher),
        ] {
            assert_eq!(s.parse::<RoutingModeKind>(), Ok(routing));
            assert_eq!(routing.as_str(), s);
        }
        assert_eq!(
            "acp".parse::<ExternalAgentKind>(),
            Ok(ExternalAgentKind::Acp)
        );
        assert_eq!(ExternalAgentKind::Acp.as_str(), "acp");
    }

    #[test]
    fn enum_parse_rejects_unknown_values_with_expected_list() {
        assert!("grok".parse::<ProviderWire>().unwrap_err().contains("grok"));
        assert!(
            "maybe"
                .parse::<ApprovalPolicyKind>()
                .unwrap_err()
                .contains("expected \"ask\", \"allow\", or \"deny\"")
        );
        assert!(
            "random"
                .parse::<RoutingModeKind>()
                .unwrap_err()
                .contains("expected \"model_routed\" or \"dispatcher\"")
        );
        assert!(
            "mcp"
                .parse::<ExternalAgentKind>()
                .unwrap_err()
                .contains("expected \"acp\"")
        );
    }

    #[test]
    fn budget_dto_projection_is_lossless() {
        let dto = BudgetDto {
            max_steps: Some(10),
            max_tokens: None,
            max_cost_micros: Some(5),
            max_wall_time_secs: None,
        };
        let budget = Budget::from(dto);
        assert_eq!(budget.max_steps(), Some(10));
        assert_eq!(budget.max_tokens(), None);
        assert!(!budget.is_empty());
        assert_eq!(BudgetDto::from(budget), dto);
        assert!(Budget::default().is_empty());
    }
}
