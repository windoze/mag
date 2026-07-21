//! The `MagService` service facade: the single abstraction interface adapters
//! (ACP, tauri, web) target.
//!
//! `mag-service` owns the *abstract* contract only. [`MagService`] is an
//! object-safe async trait (`Arc<dyn MagService>`) implemented by
//! `mag-core::Engine`; the [`Command`](crate::Command)/[`Event`](crate::Event)
//! protocol is one transport-facing wire encoding of these methods and of the
//! neutral [`ServiceEvent`] stream (see `docs/DESIGN.md` §3.0/§4).
//!
//! The trait is deliberately shaped to the *near-complete* superset of front-end
//! needs (multi-session management, approval round-trips, delegation, source
//! probing) even though the first interface (ACP) only maps a subset. This keeps
//! later interfaces from having to change the trait (`docs/DESIGN.md` §11 risk 6).

use async_trait::async_trait;
use futures::stream::BoxStream;
use mag_config::ConfigDto;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

use crate::{
    AgentInstanceStatusWire, DelegationMessageWire, DelegationTrace, Event, HistoryEntry,
    InteractionKindWire, InteractionOrigin, InteractionResponseWire, RequestId, RunErrorKind,
    RunId, RunOutput, SessionConfig, SessionId, SourceInfo, ToolTrace,
};

/// Transport-neutral service facade implemented by `mag-core::Engine`.
///
/// State-changing operations are ordinary async methods; asynchronously produced
/// events (streamed text, tools, approval requests, delegation progress …) are
/// observed through the [`subscribe`](MagService::subscribe) event stream. This
/// keeps approval round-trips ([`respond_interaction`](MagService::respond_interaction))
/// and cancellation ([`cancel`](MagService::cancel)) as independent methods,
/// decoupled from the event stream, and naturally supports one session being
/// observed by several subscribers (multi-window GUI, coexisting interfaces).
///
/// The trait is object-safe so it can be injected as `Arc<dyn MagService>`.
#[async_trait]
pub trait MagService: Send + Sync {
    // —— Session management ——

    /// Creates a new session from `config` and returns its identity.
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the configuration is rejected or the
    /// backing engine fails to register the session.
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError>;

    /// Lists currently known sessions.
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the backing engine cannot enumerate
    /// sessions.
    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError>;

    /// Resumes a persisted session so it can accept further messages.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown, or another
    /// [`ServiceError`] when the engine cannot restore the session.
    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError>;

    /// Returns the committed history for a persisted or live session.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown, or another
    /// [`ServiceError`] when the implementation cannot restore the committed
    /// history snapshot.
    async fn get_session_history(&self, id: SessionId) -> Result<Vec<HistoryEntry>, ServiceError>;

    /// Deletes a session and stops its driver.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown, or another
    /// [`ServiceError`] when the engine cannot delete the session.
    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError>;

    // —— Conversation / runs ——

    /// Sends a user message into a session and returns the started run identity.
    ///
    /// The run's streamed output is observed through
    /// [`subscribe`](MagService::subscribe).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown, or another
    /// [`ServiceError`] when the run cannot be started.
    async fn send_message(&self, id: SessionId, input: UserInput) -> Result<RunId, ServiceError>;

    /// Cancels the active run for a session, if any.
    ///
    /// Cancellation is a side-channel that never borrows the session's agent
    /// mutably, so it is never starved by an in-flight run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown, or another
    /// [`ServiceError`] when cancellation cannot be delivered.
    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError>;

    /// Injects a pivot message into a session's in-progress run.
    ///
    /// This is the first layer of the two-layer pivot semantics (`docs/CLI.md`
    /// §3.2, decision D1): pivot only ever targets a run that is currently in
    /// progress. The message is queued for the run
    /// ([`ServiceEvent::PivotQueued`]) and applied at the next step boundary
    /// ([`ServiceEvent::PivotApplied`]), or reported dropped
    /// ([`ServiceEvent::PivotDropped`]) when the run ends before the pivot
    /// lands. The second layer — falling back to
    /// [`send_message`](MagService::send_message) when no run is in progress —
    /// is a caller-side convenience and is deliberately never performed here:
    /// pivot is pivot, and failure is reported as-is.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown,
    /// [`ServiceError::NotPivotable`] when the session has no in-progress run,
    /// [`ServiceError::Unsupported`] when the implementation does not support
    /// pivoting, or another [`ServiceError`] on delivery failure.
    async fn pivot_message(&self, _id: SessionId, _input: UserInput) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "pivot_message".to_owned(),
        })
    }

    // —— Approval / interaction round-trips ——

    /// Resolves a pending interaction (approval, question, choice, permission).
    ///
    /// The `request_id` is the identity emitted by
    /// [`ServiceEvent::InteractionRequested`]. Delivering the response wakes the
    /// paused driver so the run can continue.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::SessionNotFound`] when `id` is unknown,
    /// [`ServiceError::InteractionNotFound`] when `request_id` has no pending
    /// interaction, or another [`ServiceError`] on delivery failure.
    async fn respond_interaction(
        &self,
        id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError>;

    // —— Event stream (global or filtered by session) ——

    /// Subscribes to the neutral event stream.
    ///
    /// When `id` is `Some`, only events for that session (plus global events with
    /// no session scope) are delivered; when `None`, every event is delivered.
    /// The returned stream is `'static` so it can be moved into a transport task.
    fn subscribe(&self, id: Option<SessionId>) -> BoxStream<'static, ServiceEvent>;

    // —— Sources / capabilities ——

    /// Lists configured AI sources (LLM providers, local agents, tool runtimes).
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the source registry cannot be read.
    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError>;

    /// Probes local coding-agent binaries and reports their availability.
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the probe cannot be performed.
    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError>;

    // —— Runtime configuration ——

    /// Returns the current runtime configuration projected back to its DTO
    /// form (`docs/CLI.md` §4.2/§4.3, decision D4).
    ///
    /// The returned [`ConfigDto`] mirrors *any* configuration source (the TOML
    /// file is just one persistence form). Secret fields keep their reference
    /// shape (`{env = ".."}` / `{keyring = ".."}`); values are never
    /// materialized through this contract.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Unsupported`] when the implementation has no
    /// configuration backend, or [`ServiceError::Config`] when the current
    /// snapshot cannot be projected.
    async fn get_config(&self) -> Result<ConfigDto, ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "get_config".to_owned(),
        })
    }

    /// Replaces the runtime configuration with `config` (`docs/CLI.md` §4.3).
    ///
    /// The DTO is resolved into a new snapshot, written through to the config
    /// file, and swapped in with a bumped revision; subscribers observe a
    /// [`ServiceEvent::ConfigChanged`]. Effect timing follows decision D2
    /// (`docs/CLI.md` §4.4): the new snapshot applies to *new* sessions
    /// immediately but does **not** affect sessions that already pinned an
    /// older snapshot — use [`apply_config`](MagService::apply_config) to roll
    /// it onto live sessions. A failed validation or write leaves the current
    /// snapshot untouched.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] when the DTO fails validation or the
    /// write-through fails, [`ServiceError::Unsupported`] when the
    /// implementation has no configuration backend, or another
    /// [`ServiceError`] on backend failure.
    async fn update_config(&self, _config: ConfigDto) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "update_config".to_owned(),
        })
    }

    /// Re-reads the configuration file and swaps in a fresh snapshot
    /// (`docs/CLI.md` §4.3).
    ///
    /// On success the revision is bumped and subscribers observe a
    /// [`ServiceEvent::ConfigChanged`]. Effect timing follows decision D2
    /// (`docs/CLI.md` §4.4): the new snapshot applies to *new* sessions
    /// immediately but does **not** affect sessions that already pinned an
    /// older snapshot. A missing, corrupt, or invalid file keeps the current
    /// snapshot and reports the failure — the running process never crashes
    /// on a bad config.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] when the file cannot be read or
    /// validated, [`ServiceError::Unsupported`] when the implementation has no
    /// configuration backend, or another [`ServiceError`] on backend failure.
    async fn reload_config(&self) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "reload_config".to_owned(),
        })
    }

    /// Requests that the current configuration snapshot be applied to live
    /// sessions (`docs/CLI.md` §4.4, decision D2).
    ///
    /// The application is queued per session and executed at each session's
    /// **next turn boundary** through the turn-complete mechanism
    /// (`docs/CLI.md` §4.5); sessions without an in-progress run apply
    /// immediately. A run that is currently in flight is never mutated
    /// mid-turn.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Unsupported`] when the implementation has no
    /// configuration backend (or no turn-complete mechanism), or another
    /// [`ServiceError`] on backend failure.
    async fn apply_config(&self) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "apply_config".to_owned(),
        })
    }
}

/// Neutral user input supplied to [`MagService::send_message`].
///
/// This is the transport-neutral counterpart of the
/// [`Command::SendMessage`](crate::Command::SendMessage) wire payload.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct UserInput {
    /// User-visible text payload.
    pub text: String,
    /// Optional attachments supplied with the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::MessageAttachment>,
}

impl UserInput {
    /// Builds a plain-text input with no attachments.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }
}

/// Metadata describing one known session, returned by
/// [`MagService::list_sessions`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct SessionInfo {
    /// Stable session identity.
    pub id: SessionId,
    /// Configuration the session was created with.
    pub config: SessionConfig,
    /// User-facing title derived from the first user message, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Last activity timestamp using the service's Unix-epoch millisecond convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<u64>,
    /// Current live status of the session.
    #[serde(default)]
    pub status: SessionStatusWire,
}

impl SessionInfo {
    /// Builds a session listing entry with default optional metadata.
    #[must_use]
    pub fn new(id: SessionId, config: SessionConfig) -> Self {
        Self {
            id,
            config,
            title: None,
            last_active_at: None,
            status: SessionStatusWire::Idle,
        }
    }
}

/// Wire status for a listed session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusWire {
    /// No run is currently active and no interaction is waiting.
    #[default]
    Idle,
    /// A run is in progress.
    Running,
    /// The active run is parked waiting for an interaction response.
    AwaitingInteraction,
}

/// Neutral, serializable event produced by a [`MagService`] and observed through
/// [`MagService::subscribe`].
///
/// The transport-facing [`Event`](crate::Event) enum is a projection of this
/// model for tauri/web; ACP maps the relevant variants to its own notifications
/// (`docs/DESIGN.md` §3.0/§5). Variants mirror the near-complete capability set;
/// interfaces observe whichever subset they need.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServiceEvent {
    /// A session was created.
    SessionCreated {
        /// Created session identity.
        id: SessionId,
        /// Configuration stored for the session.
        config: SessionConfig,
    },
    /// A run started for a session.
    RunStarted {
        /// Session that owns the run.
        id: SessionId,
        /// Run identity used by downstream tracing.
        run_id: RunId,
    },
    /// A run finished successfully.
    RunFinished {
        /// Session that owns the finished run.
        id: SessionId,
        /// Final run output.
        output: RunOutput,
    },
    /// A run failed before producing a successful result.
    RunError {
        /// Session whose run failed.
        id: SessionId,
        /// Human-readable failure message.
        message: String,
        /// Machine-readable failure classification; defaults to
        /// [`RunErrorKind::Other`] so events serialized before this field
        /// existed still deserialize.
        #[serde(default)]
        kind: RunErrorKind,
    },
    /// A streamed text delta was produced by the model.
    TextDelta {
        /// Session receiving the delta.
        id: SessionId,
        /// Incremental text fragment.
        text: String,
    },
    /// A tool call started.
    ToolStarted {
        /// Session that owns the tool call.
        id: SessionId,
        /// Tool trace data available at start time.
        trace: ToolTrace,
    },
    /// A tool call finished.
    ToolFinished {
        /// Session that owns the tool call.
        id: SessionId,
        /// Tool trace data available at finish time.
        trace: ToolTrace,
    },
    /// The service is waiting for an external interaction response.
    ///
    /// Sub-agent (delegate) interactions pop up to the root session through
    /// this same variant, attributed by `origin` (`docs/CLI.md` §3.3,
    /// decision D5); interfaces handle every pending interaction through one
    /// queue without tracking per-delegate channels.
    InteractionRequested {
        /// Session that owns the pending interaction.
        id: SessionId,
        /// Request identity to pass back in
        /// [`MagService::respond_interaction`].
        request_id: RequestId,
        /// Interaction details to render or decide.
        kind: InteractionKindWire,
        /// Origin attribution for the interaction; defaults to the root
        /// origin ([`InteractionOrigin::default`]) so events serialized before
        /// this field existed still deserialize.
        #[serde(default)]
        origin: InteractionOrigin,
    },
    /// A delegated child-agent task started.
    DelegationStarted {
        /// Session that owns the delegation.
        id: SessionId,
        /// Delegation trace data available at start time.
        trace: DelegationTrace,
    },
    /// A delegated child-agent task finished.
    DelegationFinished {
        /// Session that owns the delegation.
        id: SessionId,
        /// Delegation trace data available at finish time.
        trace: DelegationTrace,
    },
    /// A delegated child-agent task failed.
    DelegationFailed {
        /// Session that owns the delegation.
        id: SessionId,
        /// Delegation trace data available at failure time.
        trace: DelegationTrace,
    },
    /// A delegated child agent emitted a message.
    DelegationMessage {
        /// Session that owns the delegation.
        id: SessionId,
        /// Delegated-agent message payload.
        message: DelegationMessageWire,
    },
    /// An agent instance spawned through the `agent` tool started
    /// (`docs/dyn-agents.md`, decision D3).
    AgentInstanceStarted {
        /// Session that owns the instance.
        id: SessionId,
        /// Stable instance identity (`"<type>-<n>"`, per-type counter).
        instance_id: String,
        /// Agent type (definition name) the instance was spawned from.
        agent_type: String,
        /// Optional human-readable task description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Nesting depth: `1` for a direct child of the session's root agent.
        depth: u32,
    },
    /// An agent instance reached a terminal state (`docs/dyn-agents.md`).
    AgentInstanceFinished {
        /// Session that owns the instance.
        id: SessionId,
        /// Stable instance identity (`"<type>-<n>"`, per-type counter).
        instance_id: String,
        /// Agent type (definition name) the instance was spawned from.
        agent_type: String,
        /// Terminal status of the instance.
        status: AgentInstanceStatusWire,
        /// Final report text when the instance completed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<String>,
        /// Failure or cancellation detail when the instance did not complete.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Local coding-agent probes completed.
    LocalAgentsProbed {
        /// Available sources discovered by the probe.
        available: Vec<SourceInfo>,
    },
    /// A pivot message was accepted into a session's pivot queue.
    ///
    /// First layer of the two-layer pivot semantics (`docs/CLI.md` §3.2,
    /// decision D1): emitted when [`MagService::pivot_message`] queues a pivot
    /// for an in-progress run; the pivot later lands at a step boundary
    /// ([`PivotApplied`](ServiceEvent::PivotApplied)) or is reported dropped
    /// ([`PivotDropped`](ServiceEvent::PivotDropped)) when the run ends first.
    PivotQueued {
        /// Session whose in-progress run the pivot targets.
        id: SessionId,
    },
    /// A queued pivot message was injected into the run at a step boundary.
    PivotApplied {
        /// Session whose run accepted the pivot.
        id: SessionId,
    },
    /// A queued pivot message was dropped because the run ended (finished,
    /// failed, or cancelled) before it could be applied at a step boundary.
    PivotDropped {
        /// Session whose run the pivot targeted.
        id: SessionId,
        /// Human-readable reason the pivot was dropped (for example which
        /// terminal run state preempted it).
        reason: String,
    },
    /// The runtime configuration changed; `revision` is the new snapshot's
    /// revision (`docs/CLI.md` §4.3, decisions D2/D4).
    ///
    /// This is a global event (no session scope), so
    /// [`session_id`](ServiceEvent::session_id) returns `None` and every
    /// subscriber — including per-session ones — observes it. The new
    /// snapshot applies to *new* sessions immediately; existing sessions keep
    /// their pinned snapshot until [`MagService::apply_config`] lands the
    /// change at a turn boundary (`docs/CLI.md` §4.4).
    ConfigChanged {
        /// Monotonic revision of the newly applied configuration snapshot.
        revision: u64,
    },
}

impl ServiceEvent {
    /// Returns the session this event is scoped to, if any.
    ///
    /// Session-scoped variants return `Some`; global events such as
    /// [`LocalAgentsProbed`](ServiceEvent::LocalAgentsProbed) and
    /// [`ConfigChanged`](ServiceEvent::ConfigChanged) return `None`.
    /// [`MagService::subscribe`] uses this to filter events per session.
    #[must_use]
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionCreated { id, .. }
            | Self::RunStarted { id, .. }
            | Self::RunFinished { id, .. }
            | Self::RunError { id, .. }
            | Self::TextDelta { id, .. }
            | Self::ToolStarted { id, .. }
            | Self::ToolFinished { id, .. }
            | Self::InteractionRequested { id, .. }
            | Self::DelegationStarted { id, .. }
            | Self::DelegationFinished { id, .. }
            | Self::DelegationFailed { id, .. }
            | Self::DelegationMessage { id, .. }
            | Self::AgentInstanceStarted { id, .. }
            | Self::AgentInstanceFinished { id, .. }
            | Self::PivotQueued { id, .. }
            | Self::PivotApplied { id, .. }
            | Self::PivotDropped { id, .. } => Some(*id),
            Self::LocalAgentsProbed { .. } | Self::ConfigChanged { .. } => None,
        }
    }
}

impl From<Event> for ServiceEvent {
    /// Projects a transport-facing [`Event`](crate::Event) into the neutral
    /// [`ServiceEvent`] observed through [`MagService::subscribe`].
    ///
    /// The two enums are structurally identical; the [`Event`](crate::Event)
    /// protocol is simply the tauri/web wire encoding of this neutral model
    /// (`docs/DESIGN.md` §3.0/§4), so the mapping is a variant-for-variant
    /// projection.
    fn from(event: Event) -> Self {
        match event {
            Event::SessionCreated { id, config } => Self::SessionCreated { id, config },
            Event::RunStarted { id, run_id } => Self::RunStarted { id, run_id },
            Event::RunFinished { id, output } => Self::RunFinished { id, output },
            Event::RunError { id, message, kind } => Self::RunError { id, message, kind },
            Event::TextDelta { id, text } => Self::TextDelta { id, text },
            Event::ToolStarted { id, trace } => Self::ToolStarted { id, trace },
            Event::ToolFinished { id, trace } => Self::ToolFinished { id, trace },
            Event::InteractionRequested {
                id,
                request_id,
                kind,
                origin,
            } => Self::InteractionRequested {
                id,
                request_id,
                kind,
                origin,
            },
            Event::DelegationStarted { id, trace } => Self::DelegationStarted { id, trace },
            Event::DelegationFinished { id, trace } => Self::DelegationFinished { id, trace },
            Event::DelegationFailed { id, trace } => Self::DelegationFailed { id, trace },
            Event::DelegationMessage { id, message } => Self::DelegationMessage { id, message },
            Event::AgentInstanceStarted {
                id,
                instance_id,
                agent_type,
                description,
                depth,
            } => Self::AgentInstanceStarted {
                id,
                instance_id,
                agent_type,
                description,
                depth,
            },
            Event::AgentInstanceFinished {
                id,
                instance_id,
                agent_type,
                status,
                report,
                error,
            } => Self::AgentInstanceFinished {
                id,
                instance_id,
                agent_type,
                status,
                report,
                error,
            },
            Event::LocalAgentsProbed { available } => Self::LocalAgentsProbed { available },
            Event::PivotQueued { id } => Self::PivotQueued { id },
            Event::PivotApplied { id } => Self::PivotApplied { id },
            Event::PivotDropped { id, reason } => Self::PivotDropped { id, reason },
            Event::ConfigChanged { revision } => Self::ConfigChanged { revision },
        }
    }
}

/// Error returned by [`MagService`] operations.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServiceError {
    /// The referenced session does not exist.
    SessionNotFound {
        /// Session identity that could not be found.
        id: SessionId,
    },
    /// No pending interaction matched the supplied request identity.
    InteractionNotFound {
        /// Request identity that had no pending interaction.
        request_id: RequestId,
    },
    /// The supplied input or configuration was rejected.
    InvalidInput {
        /// Human-readable rejection reason.
        message: String,
    },
    /// The session has no in-progress run that could accept a pivot.
    ///
    /// First layer of the two-layer pivot semantics (`docs/CLI.md` §3.2,
    /// decision D1): pivoting never falls back to sending a new message
    /// implicitly; the caller decides whether to retry as
    /// [`MagService::send_message`].
    NotPivotable {
        /// Session that could not accept a pivot.
        id: SessionId,
        /// Human-readable reason the pivot was rejected.
        reason: String,
    },
    /// The requested operation is not supported by this implementation.
    Unsupported {
        /// Operation name that is not supported.
        operation: String,
    },
    /// A configuration operation failed (`docs/CLI.md` §4, decisions D2/D4).
    ///
    /// Carries configuration errors surfaced by the runtime configuration
    /// system (validation failures, unreadable or corrupt config files,
    /// write-through failures) through the service contract. The message
    /// describes the failure — including the field path or file position when
    /// known — and never contains materialized secret values.
    Config {
        /// Human-readable configuration failure detail.
        message: String,
    },
    /// The backing engine failed to complete the operation.
    Backend {
        /// Human-readable failure detail.
        message: String,
    },
}

impl ServiceError {
    /// Returns the stable snake_case error kind used by REST error projection.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionNotFound { .. } => "session_not_found",
            Self::InteractionNotFound { .. } => "interaction_not_found",
            Self::InvalidInput { .. } => "invalid_input",
            Self::NotPivotable { .. } => "not_pivotable",
            Self::Unsupported { .. } => "unsupported",
            Self::Config { .. } => "config",
            Self::Backend { .. } => "backend",
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound { id } => write!(formatter, "session `{id}` not found"),
            Self::InteractionNotFound { request_id } => {
                write!(formatter, "interaction `{request_id}` not found")
            }
            Self::InvalidInput { message } => write!(formatter, "invalid input: {message}"),
            Self::NotPivotable { id, reason } => {
                write!(formatter, "session `{id}` is not pivotable: {reason}")
            }
            Self::Unsupported { operation } => {
                write!(formatter, "operation `{operation}` is not supported")
            }
            Self::Config { message } => write!(formatter, "config error: {message}"),
            Self::Backend { message } => write!(formatter, "backend error: {message}"),
        }
    }
}

impl Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DelegationStatusWire, PermissionCategoryWire, PermissionRiskWire, ToolStatusWire, UsageInfo,
    };
    use futures::{StreamExt, stream};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use uuid::Uuid;

    fn uuid(offset: u128) -> Uuid {
        Uuid::from_u128(0x018f_0d9c_7b6a_7c12_8f31_0000_0000_0000 + offset)
    }

    fn session_id() -> SessionId {
        SessionId::new(uuid(1))
    }

    fn config() -> SessionConfig {
        SessionConfig {
            provider: "openai".to_owned(),
            model: "gpt-5-codex".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: crate::RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn tool_trace() -> ToolTrace {
        ToolTrace {
            run_id: Some(RunId::new(uuid(3))),
            call_id: crate::ToolCallIdWire::new(uuid(6)),
            name: "read_file".to_owned(),
            input: Some(json!({ "path": "README.md" })),
            output: None,
            status: ToolStatusWire::Started,
            message: None,
        }
    }

    fn delegation_trace() -> DelegationTrace {
        DelegationTrace {
            run_id: Some(RunId::new(uuid(3))),
            delegate: "codex".to_owned(),
            status: DelegationStatusWire::Started,
            task: Some("review patch".to_owned()),
            output: None,
            message: None,
            usage: None,
        }
    }

    fn source() -> SourceInfo {
        SourceInfo {
            id: "codex".to_owned(),
            name: "Codex".to_owned(),
            kind: crate::SourceKindWire::LocalAgent,
            available: true,
            version: None,
            path: None,
            capabilities: Vec::new(),
        }
    }

    /// Compile-time proof that [`MagService`] is object-safe.
    const _: fn() = || {
        fn assert_object_safe(_: &dyn MagService) {}
        let _ = assert_object_safe;
    };

    struct DummyService;

    #[async_trait]
    impl MagService for DummyService {
        async fn create_session(&self, _config: SessionConfig) -> Result<SessionId, ServiceError> {
            Ok(session_id())
        }

        async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
            Ok(Vec::new())
        }

        async fn resume_session(&self, _id: SessionId) -> Result<(), ServiceError> {
            Ok(())
        }

        async fn get_session_history(
            &self,
            _id: SessionId,
        ) -> Result<Vec<HistoryEntry>, ServiceError> {
            Ok(Vec::new())
        }

        async fn delete_session(&self, _id: SessionId) -> Result<(), ServiceError> {
            Ok(())
        }

        async fn send_message(
            &self,
            _id: SessionId,
            _input: UserInput,
        ) -> Result<RunId, ServiceError> {
            Ok(RunId::new(uuid(3)))
        }

        async fn cancel(&self, _id: SessionId) -> Result<(), ServiceError> {
            Ok(())
        }

        async fn pivot_message(
            &self,
            _id: SessionId,
            _input: UserInput,
        ) -> Result<(), ServiceError> {
            Err(ServiceError::Unsupported {
                operation: "pivot_message".to_owned(),
            })
        }

        async fn respond_interaction(
            &self,
            _id: SessionId,
            _request_id: RequestId,
            _response: InteractionResponseWire,
        ) -> Result<(), ServiceError> {
            Ok(())
        }

        fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
            stream::empty().boxed()
        }

        async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
            Ok(Vec::new())
        }

        async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
            Ok(Vec::new())
        }

        async fn get_config(&self) -> Result<ConfigDto, ServiceError> {
            Err(ServiceError::Unsupported {
                operation: "get_config".to_owned(),
            })
        }

        async fn update_config(&self, _config: ConfigDto) -> Result<(), ServiceError> {
            Err(ServiceError::Unsupported {
                operation: "update_config".to_owned(),
            })
        }

        async fn reload_config(&self) -> Result<(), ServiceError> {
            Err(ServiceError::Unsupported {
                operation: "reload_config".to_owned(),
            })
        }

        async fn apply_config(&self) -> Result<(), ServiceError> {
            Err(ServiceError::Unsupported {
                operation: "apply_config".to_owned(),
            })
        }
    }

    #[test]
    fn trait_is_usable_behind_arc_dyn() {
        futures::executor::block_on(async {
            let service: Arc<dyn MagService> = Arc::new(DummyService);

            let id = service.create_session(config()).await.expect("create");
            assert_eq!(id, session_id());
            assert!(service.list_sessions().await.expect("list").is_empty());

            let mut events = service.subscribe(Some(id));
            assert!(events.next().await.is_none());
        });
    }

    #[test]
    fn session_info_legacy_json_defaults_new_metadata() {
        let legacy = json!({
            "id": session_id(),
            "config": config(),
        });

        let decoded = serde_json::from_value::<SessionInfo>(legacy)
            .expect("legacy SessionInfo without web metadata");

        assert_eq!(decoded, SessionInfo::new(session_id(), config()));
    }

    #[test]
    fn session_info_metadata_round_trips_with_stable_status_tags() {
        let mut info = SessionInfo::new(session_id(), config());
        info.title = Some("first prompt".to_owned());
        info.last_active_at = Some(1_721_234_567_890);
        info.status = SessionStatusWire::AwaitingInteraction;

        let json = serde_json::to_value(&info).expect("serialize SessionInfo");
        assert_eq!(
            json.get("status"),
            Some(&Value::String("awaiting_interaction".to_owned()))
        );

        let decoded = serde_json::from_value::<SessionInfo>(json).expect("deserialize SessionInfo");
        assert_eq!(decoded, info);
    }

    fn assert_round_trip(value: ServiceEvent, expected_tag: &str) {
        let json = serde_json::to_value(&value).expect("serialize event");
        assert_eq!(
            json.get("type"),
            Some(&Value::String(expected_tag.to_owned())),
            "stable tag for {value:?}",
        );
        let decoded = serde_json::from_value::<ServiceEvent>(json).expect("deserialize event");
        assert_eq!(decoded, value);
    }

    #[test]
    fn service_event_variants_round_trip_and_keep_stable_tags() {
        let cases = vec![
            (
                ServiceEvent::SessionCreated {
                    id: session_id(),
                    config: config(),
                },
                "session_created",
            ),
            (
                ServiceEvent::RunStarted {
                    id: session_id(),
                    run_id: RunId::new(uuid(3)),
                },
                "run_started",
            ),
            (
                ServiceEvent::RunFinished {
                    id: session_id(),
                    output: RunOutput {
                        text: "done".to_owned(),
                        usage: Some(UsageInfo {
                            input_tokens: 5,
                            output_tokens: 7,
                            total_tokens: 12,
                        }),
                    },
                },
                "run_finished",
            ),
            (
                ServiceEvent::RunError {
                    id: session_id(),
                    message: "boom".to_owned(),
                    kind: crate::RunErrorKind::default(),
                },
                "run_error",
            ),
            (
                ServiceEvent::TextDelta {
                    id: session_id(),
                    text: "hel".to_owned(),
                },
                "text_delta",
            ),
            (
                ServiceEvent::ToolStarted {
                    id: session_id(),
                    trace: tool_trace(),
                },
                "tool_started",
            ),
            (
                ServiceEvent::ToolFinished {
                    id: session_id(),
                    trace: tool_trace(),
                },
                "tool_finished",
            ),
            (
                ServiceEvent::InteractionRequested {
                    id: session_id(),
                    request_id: RequestId::new(uuid(2)),
                    kind: InteractionKindWire::Permission {
                        action_id: "action-1".to_owned(),
                        actor: crate::AgentIdWire::new(uuid(4)),
                        category: PermissionCategoryWire::FileWrite,
                        risk: PermissionRiskWire::Medium,
                        summary: "write a file".to_owned(),
                        subject: json!({ "path": "src/lib.rs" }),
                        reason: None,
                    },
                    origin: InteractionOrigin {
                        delegate: Some("codex".to_owned()),
                        depth: 1,
                    },
                },
                "interaction_requested",
            ),
            (
                ServiceEvent::DelegationStarted {
                    id: session_id(),
                    trace: delegation_trace(),
                },
                "delegation_started",
            ),
            (
                ServiceEvent::DelegationFinished {
                    id: session_id(),
                    trace: delegation_trace(),
                },
                "delegation_finished",
            ),
            (
                ServiceEvent::DelegationFailed {
                    id: session_id(),
                    trace: delegation_trace(),
                },
                "delegation_failed",
            ),
            (
                ServiceEvent::DelegationMessage {
                    id: session_id(),
                    message: DelegationMessageWire {
                        run_id: Some(RunId::new(uuid(3))),
                        delegate: "codex".to_owned(),
                        text: "child update".to_owned(),
                    },
                },
                "delegation_message",
            ),
            (
                ServiceEvent::AgentInstanceStarted {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    description: Some("map the codebase".to_owned()),
                    depth: 1,
                },
                "agent_instance_started",
            ),
            (
                ServiceEvent::AgentInstanceFinished {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    status: AgentInstanceStatusWire::Completed,
                    report: Some("found 3 call sites".to_owned()),
                    error: None,
                },
                "agent_instance_finished",
            ),
            (
                ServiceEvent::LocalAgentsProbed {
                    available: vec![source()],
                },
                "local_agents_probed",
            ),
            (
                ServiceEvent::PivotQueued { id: session_id() },
                "pivot_queued",
            ),
            (
                ServiceEvent::PivotApplied { id: session_id() },
                "pivot_applied",
            ),
            (
                ServiceEvent::PivotDropped {
                    id: session_id(),
                    reason: "run finished before the pivot landed".to_owned(),
                },
                "pivot_dropped",
            ),
            (
                ServiceEvent::ConfigChanged { revision: 7 },
                "config_changed",
            ),
        ];

        for (event, expected_tag) in cases {
            assert_round_trip(event, expected_tag);
        }
    }

    #[test]
    fn event_projects_into_matching_service_event() {
        let cases = vec![
            (
                Event::SessionCreated {
                    id: session_id(),
                    config: config(),
                },
                ServiceEvent::SessionCreated {
                    id: session_id(),
                    config: config(),
                },
            ),
            (
                Event::RunStarted {
                    id: session_id(),
                    run_id: RunId::new(uuid(3)),
                },
                ServiceEvent::RunStarted {
                    id: session_id(),
                    run_id: RunId::new(uuid(3)),
                },
            ),
            (
                Event::TextDelta {
                    id: session_id(),
                    text: "hi".to_owned(),
                },
                ServiceEvent::TextDelta {
                    id: session_id(),
                    text: "hi".to_owned(),
                },
            ),
            (
                Event::ToolStarted {
                    id: session_id(),
                    trace: tool_trace(),
                },
                ServiceEvent::ToolStarted {
                    id: session_id(),
                    trace: tool_trace(),
                },
            ),
            (
                Event::DelegationFailed {
                    id: session_id(),
                    trace: delegation_trace(),
                },
                ServiceEvent::DelegationFailed {
                    id: session_id(),
                    trace: delegation_trace(),
                },
            ),
            (
                Event::AgentInstanceStarted {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    description: Some("map the codebase".to_owned()),
                    depth: 1,
                },
                ServiceEvent::AgentInstanceStarted {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    description: Some("map the codebase".to_owned()),
                    depth: 1,
                },
            ),
            (
                Event::AgentInstanceFinished {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    status: AgentInstanceStatusWire::Failed,
                    report: None,
                    error: Some("step budget exhausted".to_owned()),
                },
                ServiceEvent::AgentInstanceFinished {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    status: AgentInstanceStatusWire::Failed,
                    report: None,
                    error: Some("step budget exhausted".to_owned()),
                },
            ),
            (
                Event::LocalAgentsProbed {
                    available: vec![source()],
                },
                ServiceEvent::LocalAgentsProbed {
                    available: vec![source()],
                },
            ),
            (
                Event::InteractionRequested {
                    id: session_id(),
                    request_id: RequestId::new(uuid(2)),
                    kind: InteractionKindWire::Question {
                        prompt: "Proceed?".to_owned(),
                    },
                    origin: InteractionOrigin {
                        delegate: Some("codex".to_owned()),
                        depth: 1,
                    },
                },
                ServiceEvent::InteractionRequested {
                    id: session_id(),
                    request_id: RequestId::new(uuid(2)),
                    kind: InteractionKindWire::Question {
                        prompt: "Proceed?".to_owned(),
                    },
                    origin: InteractionOrigin {
                        delegate: Some("codex".to_owned()),
                        depth: 1,
                    },
                },
            ),
            (
                Event::ConfigChanged { revision: 7 },
                ServiceEvent::ConfigChanged { revision: 7 },
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(ServiceEvent::from(event), expected);
        }
    }

    #[test]
    fn agent_instance_finished_projects_for_all_terminal_statuses() {
        let cases = vec![
            (
                AgentInstanceStatusWire::Completed,
                Some("report body".to_owned()),
                None,
            ),
            (
                AgentInstanceStatusWire::Failed,
                None,
                Some("boom".to_owned()),
            ),
            (
                AgentInstanceStatusWire::Cancelled,
                None,
                Some("supervisor cancelled".to_owned()),
            ),
        ];

        for (status, report, error) in cases {
            let event = Event::AgentInstanceFinished {
                id: session_id(),
                instance_id: "general-purpose-2".to_owned(),
                agent_type: "general-purpose".to_owned(),
                status,
                report,
                error,
            };
            let projected = ServiceEvent::from(event.clone());

            let ServiceEvent::AgentInstanceFinished {
                id,
                instance_id,
                agent_type,
                status: projected_status,
                ..
            } = &projected
            else {
                panic!("expected agent_instance_finished, got {projected:?}");
            };
            assert_eq!(*id, session_id());
            assert_eq!(instance_id, "general-purpose-2");
            assert_eq!(agent_type, "general-purpose");
            assert_eq!(*projected_status, status);
            assert_eq!(projected.session_id(), Some(session_id()));

            // The projection preserves every payload field exactly.
            assert_eq!(
                serde_json::to_value(&projected).expect("serialize projected"),
                serde_json::to_value(&event).expect("serialize event"),
            );

            let json = serde_json::to_value(&projected).expect("serialize event");
            assert_eq!(
                json.get("type"),
                Some(&Value::String("agent_instance_finished".to_owned())),
            );
            let decoded = serde_json::from_value::<ServiceEvent>(json).expect("deserialize event");
            assert_eq!(decoded, projected);
        }
    }

    #[test]
    fn service_event_session_id_is_none_only_for_global_events() {
        assert_eq!(
            ServiceEvent::TextDelta {
                id: session_id(),
                text: "hi".to_owned(),
            }
            .session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::PivotQueued { id: session_id() }.session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::PivotApplied { id: session_id() }.session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::PivotDropped {
                id: session_id(),
                reason: "run cancelled".to_owned(),
            }
            .session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::AgentInstanceStarted {
                id: session_id(),
                instance_id: "explorer-1".to_owned(),
                agent_type: "explorer".to_owned(),
                description: None,
                depth: 1,
            }
            .session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::AgentInstanceFinished {
                id: session_id(),
                instance_id: "explorer-1".to_owned(),
                agent_type: "explorer".to_owned(),
                status: AgentInstanceStatusWire::Cancelled,
                report: None,
                error: Some("supervisor cancelled".to_owned()),
            }
            .session_id(),
            Some(session_id()),
        );
        assert_eq!(
            ServiceEvent::LocalAgentsProbed {
                available: Vec::new(),
            }
            .session_id(),
            None,
        );
        assert_eq!(
            ServiceEvent::ConfigChanged { revision: 3 }.session_id(),
            None
        );
    }

    #[test]
    fn service_error_round_trips_and_displays() {
        let error = ServiceError::SessionNotFound { id: session_id() };
        assert!(error.to_string().contains("not found"));

        let json = serde_json::to_value(&error).expect("serialize error");
        assert_eq!(
            json.get("type"),
            Some(&Value::String("session_not_found".to_owned())),
        );
        let decoded = serde_json::from_value::<ServiceError>(json).expect("deserialize error");
        assert_eq!(decoded, error);

        assert_eq!(
            UserInput::text("hi"),
            UserInput {
                text: "hi".to_owned(),
                attachments: Vec::new(),
            },
        );
    }

    #[test]
    fn service_error_kind_matches_stable_tags_for_all_variants() {
        let cases = vec![
            ServiceError::SessionNotFound { id: session_id() },
            ServiceError::InteractionNotFound {
                request_id: RequestId::new(uuid(2)),
            },
            ServiceError::InvalidInput {
                message: "empty message".to_owned(),
            },
            ServiceError::NotPivotable {
                id: session_id(),
                reason: "no in-progress run".to_owned(),
            },
            ServiceError::Unsupported {
                operation: "pivot_message".to_owned(),
            },
            ServiceError::Config {
                message: "invalid config".to_owned(),
            },
            ServiceError::Backend {
                message: "store unavailable".to_owned(),
            },
        ];

        for error in cases {
            let json = serde_json::to_value(&error).expect("serialize error");
            assert_eq!(
                json.get("type"),
                Some(&Value::String(error.kind().to_owned()))
            );
        }
    }

    #[test]
    fn interaction_requested_without_origin_deserializes_as_root() {
        // Legacy event-stream shape (pre-`docs/CLI.md` §3.3 attribution): no
        // `origin` key at all. It must decode with the default root origin so
        // old recordings and old consumers keep working.
        let legacy = json!({
            "type": "interaction_requested",
            "id": session_id(),
            "request_id": RequestId::new(uuid(2)),
            "kind": { "kind": "question", "prompt": "Proceed?" },
        });
        let decoded = serde_json::from_value::<ServiceEvent>(legacy)
            .expect("legacy interaction_requested without origin");
        match decoded {
            ServiceEvent::InteractionRequested { origin, .. } => {
                assert_eq!(origin, InteractionOrigin::default());
                assert!(origin.is_root());
            }
            other => panic!("expected interaction_requested, got {other:?}"),
        }
    }

    #[test]
    fn not_pivotable_error_round_trips_and_displays() {
        let error = ServiceError::NotPivotable {
            id: session_id(),
            reason: "no in-progress run".to_owned(),
        };
        let display = error.to_string();
        assert!(display.contains("not pivotable"));
        assert!(display.contains("no in-progress run"));

        let json = serde_json::to_value(&error).expect("serialize error");
        assert_eq!(
            json.get("type"),
            Some(&Value::String("not_pivotable".to_owned())),
        );
        let decoded = serde_json::from_value::<ServiceError>(json).expect("deserialize error");
        assert_eq!(decoded, error);
    }

    #[test]
    fn pivot_message_is_callable_behind_arc_dyn() {
        futures::executor::block_on(async {
            let service: Arc<dyn MagService> = Arc::new(DummyService);

            let error = service
                .pivot_message(session_id(), UserInput::text("steer"))
                .await
                .expect_err("dummy service reports pivoting as unsupported");
            assert_eq!(
                error,
                ServiceError::Unsupported {
                    operation: "pivot_message".to_owned(),
                },
            );
        });
    }

    #[test]
    fn config_error_round_trips_and_displays() {
        let error = ServiceError::Config {
            message: "agents.default.provider: unknown provider \"missing\"".to_owned(),
        };
        let display = error.to_string();
        assert!(display.contains("config error"));
        assert!(display.contains("unknown provider"));

        let json = serde_json::to_value(&error).expect("serialize error");
        assert_eq!(json.get("type"), Some(&Value::String("config".to_owned())));
        let decoded = serde_json::from_value::<ServiceError>(json).expect("deserialize error");
        assert_eq!(decoded, error);
    }

    #[test]
    fn config_methods_are_callable_behind_arc_dyn() {
        futures::executor::block_on(async {
            let service: Arc<dyn MagService> = Arc::new(DummyService);

            assert_eq!(
                service.get_config().await,
                Err(ServiceError::Unsupported {
                    operation: "get_config".to_owned(),
                }),
            );
            assert_eq!(
                service.update_config(ConfigDto::default()).await,
                Err(ServiceError::Unsupported {
                    operation: "update_config".to_owned(),
                }),
            );
            assert_eq!(
                service.reload_config().await,
                Err(ServiceError::Unsupported {
                    operation: "reload_config".to_owned(),
                }),
            );
            assert_eq!(
                service.apply_config().await,
                Err(ServiceError::Unsupported {
                    operation: "apply_config".to_owned(),
                }),
            );
        });
    }
}
