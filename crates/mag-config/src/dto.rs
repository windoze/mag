//! DTO serde types mirroring the `docs/CLI.md` §4.2 configuration surface.
//!
//! Every field is `Option`/`Default`-friendly: any *partial* configuration is
//! legal at this layer (a GUI patch or a hand-written fragment may set only a
//! few keys). `BTreeMap` is used for all name-keyed maps so serialization is
//! deterministic, which keeps round-trips and golden tests stable.
//!
//! Cross references stay as plain names (`AgentDto::provider` is the *name* of
//! a `[providers.<name>]` entry). Resolving names into a shared `Arc` object
//! graph is the resolve half of the DTO↔DO conversion, layered on top of these
//! types.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::SecretRef;

/// Root DTO of the runtime configuration (`docs/CLI.md` §4.2
/// `ConfigFileDto`; named `ConfigDto` here because the DTO is source-agnostic
/// per decision D4 — a file is only one of its sources).
///
/// TOML mirror of the §4.2 example: `[providers.<name>]`, `[agents.<name>]`,
/// `[external_agents.<name>]`, `[tools.<name>]`, `[session]`, `[approval]`.
/// All sections are optional; an empty document deserializes to
/// `ConfigDto::default()`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
#[serde(default)]
pub struct ConfigDto {
    /// LLM provider definitions (`[providers.<name>]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "ts-export", ts(as = "Option<_>"))]
    pub providers: BTreeMap<String, ProviderDto>,
    /// Local agent definitions (`[agents.<name>]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "ts-export", ts(as = "Option<_>"))]
    pub agents: BTreeMap<String, AgentDto>,
    /// External agent sources (`[external_agents.<name>]`, decision D3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "ts-export", ts(as = "Option<_>"))]
    pub external_agents: BTreeMap<String, ExternalAgentDto>,
    /// Per-tool overrides (`[tools.<name>]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "ts-export", ts(as = "Option<_>"))]
    pub tools: BTreeMap<String, ToolDto>,
    /// Session defaults (`[session]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionDefaultsDto>,
    /// Approval defaults (`[approval]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalSectionDto>,
}

impl ConfigDto {
    /// Looks up a provider definition by name.
    pub fn provider(&self, name: &str) -> Option<&ProviderDto> {
        self.providers.get(name)
    }

    /// Looks up a local agent definition by name.
    pub fn agent(&self, name: &str) -> Option<&AgentDto> {
        self.agents.get(name)
    }

    /// Looks up an external agent definition by name.
    pub fn external_agent(&self, name: &str) -> Option<&ExternalAgentDto> {
        self.external_agents.get(name)
    }

    /// Looks up a per-tool override by tool name.
    pub fn tool(&self, name: &str) -> Option<&ToolDto> {
        self.tools.get(name)
    }

    /// Returns true when no section carries any content.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
            && self.agents.is_empty()
            && self.external_agents.is_empty()
            && self.tools.is_empty()
            && self.session.is_none()
            && self.approval.is_none()
    }
}

/// An LLM provider definition (`[providers.<name>]`, e.g.
/// `[providers.anthropic]`).
///
/// Field order matters for TOML serialization: scalar values first, table
/// values (`api_key`, `params`) last.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct ProviderDto {
    /// Wire protocol spoken by the provider (e.g. `"anthropic"`, `"openai"`).
    /// The set of supported protocols is validated at resolve time, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<String>,
    /// Endpoint base URL (e.g. `"https://api.anthropic.com"` or a local proxy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// API key **reference** (`api_key = { env = "VAR" }` or
    /// `{ keyring = "NAME" }`). Only the reference form is accepted; the
    /// secret value itself never enters the configuration (decision:
    /// `docs/CLI.md` §4.1 item 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretRef>,
    /// Free-form provider parameter table forwarded to the client at assembly
    /// time (e.g. `max_retries`, timeouts, headers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(type = "Record<string, unknown>", optional))]
    pub params: Option<BTreeMap<String, toml::Value>>,
}

/// A local agent definition (`[agents.<name>]`, e.g. `[agents.default]`).
///
/// Cross references (`provider`, `tools` entries) are plain names; existence
/// checks happen at resolve time (DTO→DO), which reports dangling references
/// with field paths.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct AgentDto {
    /// Name of the `[providers.<name>]` entry backing this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model identifier passed to the provider (e.g. `"claude-sonnet-4-5"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tool names exposed to this agent (e.g. `["read_file", "grep"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// System prompt override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Free-form role label (used for delegation descriptions / UI display).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Per-agent budget override (falls back to `[session]` defaults).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetDto>,
}

/// An external agent source (`[external_agents.<name>]`, decision D3).
///
/// `kind = "acp"` agents are spawned from `command`; working-directory
/// isolation is agent-lib's responsibility.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct ExternalAgentDto {
    /// Source kind (currently `"acp"`); validated at resolve time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Spawn command line, argv form (e.g. `["peer-agent", "--acp"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Extra environment variables for the spawned process. Values may be
    /// plain strings; secrets belong in [`SecretRef`] fields of the resolved
    /// runtime layer, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// Advertised capability tags surfaced through `list_sources()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// A per-tool override (`[tools.<name>]`, e.g. `[tools.shell]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct ToolDto {
    /// Approval policy override: `"ask"` | `"allow"` | `"deny"` (mapped to
    /// `ApprovalPolicy` at assembly time; validated at resolve time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    /// Master switch for the tool (`false` removes it from tool profiles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Session defaults (`[session]`). Only affects *new* sessions
/// (`docs/CLI.md` §4.4): existing sessions pin the snapshot captured at
/// creation time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct SessionDefaultsDto {
    /// Delegation routing mode (e.g. `"model_routed"`); validated at resolve
    /// time against the service contract's routing modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<String>,
    /// Default persistence directory for session storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persist_path: Option<PathBuf>,
    /// Default per-run budget for new sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetDto>,
}

/// Approval defaults (`[approval]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct ApprovalSectionDto {
    /// Default approval policy for tools without a `[tools.<name>]` override:
    /// `"ask"` | `"allow"` | `"deny"`; validated at resolve time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_policy: Option<String>,
    /// How long an approval prompt waits before timing out, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Per-run budget limits (`budget = { max_tokens = 200000 }`).
///
/// Field names intentionally mirror the service contract's `SessionBudget`
/// shape so the DTO↔DO conversion is a field-by-field projection. Every
/// dimension is optional; an unset dimension is unbounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct BudgetDto {
    /// Maximum agent steps per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    /// Maximum total tokens per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Maximum cost per run, in micro-units of the provider's currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_micros: Option<u64>,
    /// Maximum wall-clock time per run, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_time_secs: Option<u64>,
}

impl BudgetDto {
    /// Returns true when every dimension is unset.
    pub fn is_empty(&self) -> bool {
        self.max_steps.is_none()
            && self.max_tokens.is_none()
            && self.max_cost_micros.is_none()
            && self.max_wall_time_secs.is_none()
    }
}
