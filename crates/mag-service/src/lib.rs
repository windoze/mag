#![warn(missing_docs)]

//! Transport-neutral service protocol types for mag.
//!
//! This crate is the home of the `mag-service` layer. It owns the [`MagService`]
//! service facade (an object-safe async trait) plus the neutral service protocol
//! types: the [`Command`]/[`Event`] wire protocol, the [`ServiceEvent`] stream
//! model, and their payloads and identifier types. The types are pure serde data
//! and deliberately do not depend on `agent-lib`, even when they mirror concepts
//! from that crate. `mag-core::Engine` is the implementation of [`MagService`].
//!
//! # Contract stability
//!
//! As of the C5-R review (`TODO.md`), the [`MagService`] trait and its
//! [`Command`]/[`Event`] wire encoding are **frozen**: they form the stable
//! service contract that interface crates (ACP first, then tauri/web) build
//! against. Frozen means evolution is **additive and backward-compatible only** —
//! new trait methods, new enum variants, and new struct fields (guarded by
//! `#[non_exhaustive]`) may be added, but the semantics, names, and shapes of
//! existing items must not change. Any breaking change requires re-opening this
//! contract review rather than silently editing an interface crate around it.
//!
//! The pre-release removal of the `SessionConfig` wire type (session creation now
//! takes an optional agent-template name plus an optional runtime `cwd`, both
//! resolved against the current configuration snapshot) is one such re-opened
//! change: `provider`/`model`/`budget`/`routing`/`tool_profile` were properties
//! of the bound `[agents.<name>]` template, not per-session wire inputs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, path::PathBuf, str::FromStr};
use uuid::Uuid;

mod service;

pub use mag_config::ConfigDto;
pub use service::{
    MagService, ServiceError, ServiceEvent, SessionInfo, SessionStatusWire, UserInput,
};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
        #[cfg_attr(feature = "ts-export", ts(type = "string"))]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[doc = concat!(
                "Creates a `",
                stringify!($name),
                "` from an externally supplied UUID."
            )]
            #[must_use]
            pub const fn new(value: Uuid) -> Self {
                Self(value)
            }

            #[doc = concat!("Parses a UUID string into a `", stringify!($name), "`.")]
            ///
            /// # Errors
            ///
            /// Returns [`uuid::Error`] when `value` is not accepted by the UUID
            /// parser.
            pub fn parse_str(value: &str) -> Result<Self, uuid::Error> {
                Uuid::parse_str(value).map(Self::new)
            }

            #[doc = concat!("Returns the UUID wrapped by this `", stringify!($name), "`.")]
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            #[doc = concat!(
                "Consumes this `",
                stringify!($name),
                "` and returns its UUID."
            )]
            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse_str(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }
    };
}

define_id!(
    /// Identifies one mag session on the wire.
    SessionId
);

define_id!(
    /// Identifies one pending cross-transport interaction request.
    RequestId
);

define_id!(
    /// Identifies one externally initiated agent run.
    RunId
);

define_id!(
    /// Identifies one agent that requested a wire-visible action.
    AgentIdWire
);

define_id!(
    /// Identifies one agent step awaiting a wire-visible answer.
    StepIdWire
);

define_id!(
    /// Identifies one framework tool call awaiting a wire-visible answer.
    ToolCallIdWire
);

/// Commands accepted by the transport-neutral engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Create a new session.
    ///
    /// Both fields are optional. `agent` names the `[agents.<name>]` (or
    /// `[external_agents.<name>]`) template to bind; `None` binds the
    /// `[session].default_agent` (else the built-in `default`). `cwd` is the
    /// session's runtime working root; `None` keeps the agent's default `"."`.
    CreateSession {
        /// Runtime working root for the session, when the interface supplies one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        /// Agent-template name to bind; `None` uses the configured default agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// List currently known sessions.
    ListSessions,
    /// Resume an existing session.
    ResumeSession {
        /// Session to resume.
        id: SessionId,
    },
    /// Return the committed history for an existing session.
    GetSessionHistory {
        /// Session whose history should be returned.
        id: SessionId,
    },
    /// Delete an existing session and stop its driver.
    DeleteSession {
        /// Session to delete.
        id: SessionId,
    },
    /// Send a user message into a session.
    SendMessage {
        /// Session receiving the message.
        session_id: SessionId,
        /// User-visible text payload.
        text: String,
        /// Optional attachments supplied with the message.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<MessageAttachment>,
    },
    /// Inject a pivot message into a session's active run.
    PivotMessage {
        /// Session receiving the pivot message.
        session_id: SessionId,
        /// User-visible pivot text payload.
        text: String,
    },
    /// Cancel the active run for a session.
    CancelRun {
        /// Session whose active run should be cancelled.
        session_id: SessionId,
    },
    /// Resolve a pending interaction request.
    RespondInteraction {
        /// Session that owns the pending interaction.
        session_id: SessionId,
        /// Request identity emitted by `InteractionRequested`.
        request_id: RequestId,
        /// User or policy response to the interaction.
        response: InteractionResponseWire,
    },
    /// List configured AI sources.
    ListSources,
    /// Probe local coding-agent binaries and capabilities.
    ProbeLocalAgents,
    /// Return the current runtime configuration DTO.
    GetConfig,
    /// Replace the runtime configuration DTO.
    UpdateConfig {
        /// Configuration DTO to validate and apply to the runtime config store.
        config: ConfigDto,
    },
    /// Reload the runtime configuration from its backing source.
    ReloadConfig,
    /// Apply the current runtime configuration to live sessions at turn boundaries.
    ApplyConfig,
}

/// Events emitted by the transport-neutral engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A session was created.
    SessionCreated {
        /// Created session identity.
        id: SessionId,
        /// Resolved agent-template name the session bound to.
        agent: String,
        /// Runtime working root the session was created with, when set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
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
    /// The engine is waiting for an external interaction response.
    InteractionRequested {
        /// Session that owns the pending interaction.
        id: SessionId,
        /// Request identity to pass back in `RespondInteraction`.
        request_id: RequestId,
        /// Interaction details to render or decide.
        kind: InteractionKindWire,
        /// Origin attribution for the interaction (`docs/CLI.md` §3.3,
        /// decision D5); defaults to the root origin so events serialized
        /// before this field existed still deserialize.
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
    /// decision D1); the transport-facing twin of
    /// [`ServiceEvent::PivotQueued`].
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
        /// Human-readable reason the pivot was dropped.
        reason: String,
    },
    /// The runtime configuration changed; `revision` is the new snapshot's
    /// revision (`docs/CLI.md` §4.3, decisions D2/D4).
    ///
    /// This is a global event (no session scope): the new snapshot applies to
    /// *new* sessions immediately, while existing sessions keep their pinned
    /// snapshot until [`MagService::apply_config`] lands the change at a turn
    /// boundary (`docs/CLI.md` §4.4). The transport-facing twin of
    /// [`ServiceEvent::ConfigChanged`].
    ConfigChanged {
        /// Monotonic revision of the newly applied configuration snapshot.
        revision: u64,
    },
}

/// Machine-readable classification of a [`Event::RunError`].
///
/// Lets transports (for example the ACP bridge) map terminal failures onto
/// protocol-level stop reasons without string-matching the human-readable
/// `message`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RunErrorKind {
    /// Unclassified failure; the `message` carries the details.
    #[default]
    Other,
    /// The run was cancelled by the client.
    Cancelled,
    /// The agent loop hit its per-turn step / tool-round limit before the
    /// model produced a final response (agent-lib `FacadeError::LoopLimitExceeded`).
    LoopLimitExceeded,
    /// The session's configured per-run [`SessionBudget`] was exhausted
    /// (agent-lib `FacadeError::BudgetExhausted`).
    BudgetExhausted,
}

/// Per-run budget limits configurable on a session.
///
/// Mirrors agent-lib's `BudgetLimits` with a wire-friendly shape (wall time in
/// whole seconds). Every dimension is optional; an unset dimension is
/// unbounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct SessionBudget {
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

/// Optional user-message attachment metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct MessageAttachment {
    /// Optional display name for the attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional MIME type, such as `text/plain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional URI or path visible to the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Optional inline text data for small attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One committed history item returned by `get_session_history`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A user-authored message.
    UserMessage {
        /// User-visible text payload.
        text: String,
        /// Optional attachments supplied with the message.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<MessageAttachment>,
    },
    /// An assistant-authored text message.
    AssistantMessage {
        /// Assistant-visible text payload.
        text: String,
    },
    /// A completed tool call trace.
    ToolCall {
        /// Terminal tool trace reconstructed from committed history.
        trace: ToolTrace,
    },
    /// A completed delegation trace.
    Delegation {
        /// Terminal delegation trace reconstructed from committed history.
        trace: DelegationTrace,
    },
}

/// Final output reported for a successful run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct RunOutput {
    /// Final assistant text collected for the run.
    pub text: String,
    /// Optional usage accounting supplied by the model provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
}

/// Provider-neutral token usage summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct UsageInfo {
    /// Tokens read from the prompt or input stream.
    pub input_tokens: u64,
    /// Tokens generated by the assistant or output stream.
    pub output_tokens: u64,
    /// Total billable or provider-reported tokens.
    pub total_tokens: u64,
}

/// Trace payload for tool start and finish events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct ToolTrace {
    /// Optional run identity when the trace is tied to a specific run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Framework-level tool call identity.
    pub call_id: ToolCallIdWire,
    /// Tool name selected by the model.
    pub name: String,
    /// Tool input payload, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Tool output payload, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// Current lifecycle state for this trace snapshot.
    pub status: ToolStatusWire,
    /// Optional human-readable error or status detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Wire-visible lifecycle state for a tool trace.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ToolStatusWire {
    /// The tool call has started.
    Started,
    /// The tool call completed successfully.
    Finished,
    /// The tool call was denied before execution.
    Denied,
    /// The tool call was cancelled before completion.
    Cancelled,
    /// The tool call failed.
    Failed,
}

/// Trace payload for delegated child-agent work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct DelegationTrace {
    /// Optional parent run identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable delegate name or source key.
    pub delegate: String,
    /// Current lifecycle state for this trace snapshot.
    #[serde(default)]
    pub status: DelegationStatusWire,
    /// Optional delegated task description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Optional delegated output text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Optional failure or status message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional token usage reported by the delegate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
}

/// Wire-visible lifecycle state for a delegation trace.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatusWire {
    /// The delegation has started but has not reported a terminal outcome.
    #[default]
    Started,
    /// The delegation completed successfully.
    Finished,
    /// The delegation failed.
    Failed,
}

/// Wire-visible terminal state of an agent instance (`docs/dyn-agents.md`).
///
/// Unlike [`DelegationStatusWire`] there is no in-flight state: instance start
/// is reported by [`Event::AgentInstanceStarted`] itself, so only terminal
/// outcomes appear on [`Event::AgentInstanceFinished`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AgentInstanceStatusWire {
    /// The instance completed successfully; `report` carries the final text.
    Completed,
    /// The instance failed; `error` carries the failure detail.
    Failed,
    /// The instance was cancelled before completion.
    Cancelled,
}

/// Message emitted by a delegated child agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct DelegationMessageWire {
    /// Optional parent run identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable delegate name or source key.
    pub delegate: String,
    /// Message text emitted by the delegated agent.
    pub text: String,
}

/// Origin attribution for an interaction request (`docs/CLI.md` §3.3,
/// decision D5).
///
/// Sub-agent (delegate) interactions are not delivered over a separate
/// channel: they surface as ordinary interaction requests on the root
/// session's event stream, and this payload records where they came from so
/// interfaces can render attribution (for example `[from codex@depth1]`). It
/// is the wire counterpart of agent-lib's `InteractionOrigin { delegate,
/// depth }` (both sides keep the depth as `u32`).
///
/// The default is the *root* origin — `delegate: None, depth: 0` — meaning
/// the interaction was produced by the root session's own agent. Because the
/// event fields carrying this payload are `#[serde(default)]`, events
/// serialized before attribution existed still deserialize as
/// root-originated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct InteractionOrigin {
    /// Name of the delegate (sub-agent) that produced the interaction.
    ///
    /// `None` means the interaction was produced by the root session's own
    /// agent — the default for interactions that predate sub-agent
    /// attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate: Option<String>,
    /// Delegation depth of the producing agent: `0` is the root session's own
    /// agent, `1` a direct delegate, and so on.
    #[serde(default)]
    pub depth: u32,
}

impl InteractionOrigin {
    /// Returns `true` when the interaction was produced by the root session's
    /// own agent rather than by a delegate.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.delegate.is_none() && self.depth == 0
    }
}

/// Interaction request shown to a user or policy engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionKindWire {
    /// Degenerate yes/no approval for one framework tool call.
    Approval {
        /// Framework tool call awaiting approval.
        call_id: ToolCallIdWire,
        /// Approval requirement emitted by the tool policy.
        requirement: ApprovalRequirementWire,
    },
    /// Open-ended question.
    Question {
        /// Prompt shown to the responder.
        prompt: String,
    },
    /// Fixed-option choice.
    Choice {
        /// Prompt shown to the responder.
        prompt: String,
        /// Ordered choices; responses use zero-based indexes into this list.
        options: Vec<String>,
    },
    /// Privileged-action permission request.
    Permission {
        /// Stable identity used to correlate the eventual decision.
        action_id: String,
        /// Agent requesting the privileged action.
        actor: AgentIdWire,
        /// Class of privileged action being requested.
        category: PermissionCategoryWire,
        /// Estimated blast radius of granting the action.
        risk: PermissionRiskWire,
        /// Human-readable summary shown to the approver.
        summary: String,
        /// Structured description of the action subject.
        subject: Value,
        /// Optional rationale supplied by the requesting agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// Interaction response supplied by a user or policy engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionResponseWire {
    /// Response to an approval interaction.
    Approval {
        /// Step awaiting approval.
        step_id: StepIdWire,
        /// Tool call awaiting approval.
        call_id: ToolCallIdWire,
        /// External approval decision.
        decision: ApprovalDecisionWire,
        /// Optional model-visible denial, timeout, or cancellation message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Free-form answer to a question interaction.
    Answer {
        /// Text supplied by the responder.
        text: String,
    },
    /// Fixed-option answer to a choice interaction.
    Choice {
        /// Zero-based selected option index.
        index: usize,
    },
    /// Response to a permission interaction.
    Permission {
        /// Action identity from the permission request.
        action_id: String,
        /// External permission decision.
        decision: PermissionDecisionWire,
    },
}

/// Requirement produced by a tool approval policy.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequirementWire {
    /// Execute the tool without pausing for external approval.
    AutoApprove,
    /// Pause tool execution and ask an external approver.
    RequireApproval {
        /// Stable reason shown to the external approver.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// External decision for a tool approval request.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionWire {
    /// The tool may execute normally.
    Approve,
    /// The tool is denied.
    Deny,
    /// The approval window expired.
    Timeout,
    /// The pending tool execution should be cancelled.
    Cancel,
}

/// Class of privileged action requested by an agent.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PermissionCategoryWire {
    /// Executing a shell command.
    Shell,
    /// Reading from the filesystem.
    FileRead,
    /// Writing to the filesystem.
    FileWrite,
    /// Opening an outbound network connection.
    Network,
    /// Spawning a child agent.
    SpawnAgent,
    /// Invoking an MCP server capability.
    Mcp,
    /// Any other host-mediated action.
    Other,
}

/// Estimated blast radius of granting a privileged action.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PermissionRiskWire {
    /// Read-only or otherwise easily reversible action.
    Low,
    /// Local mutation with contained impact.
    Medium,
    /// Broad or hard-to-reverse mutation.
    High,
    /// Destructive or externally observable action.
    Critical,
}

/// External decision for a permission request.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionDecisionWire {
    /// The privileged action may proceed.
    Approve,
    /// The privileged action is refused.
    Deny {
        /// Stable reason shown to the requesting agent, if supplied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The pending privileged action should be cancelled.
    Cancel,
}

/// Information about a configured or probed AI source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(optional_fields))]
pub struct SourceInfo {
    /// Stable source key used by configuration.
    pub id: String,
    /// Human-readable source name.
    pub name: String,
    /// Source family.
    pub kind: SourceKindWire,
    /// Whether the source is currently usable.
    pub available: bool,
    /// Optional version string reported by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Optional executable path or endpoint identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional capability labels reported by the source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Wire-visible source family.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SourceKindWire {
    /// Hosted or API-backed model provider.
    LlmProvider,
    /// Local coding-agent binary such as Claude Code or Codex.
    LocalAgent,
    /// Tool runtime or registry source.
    ToolRuntime,
    /// Other source type.
    Other,
}

#[cfg(all(test, feature = "ts-export"))]
mod ts_exports {
    use super::*;
    use std::{fs, path::PathBuf};
    use ts_rs::{Config, TS};

    fn protocol_src_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/packages/protocol/src")
    }

    fn export_type<T>(config: &Config)
    where
        T: TS + 'static,
    {
        T::export_all(config).unwrap_or_else(|error| {
            panic!(
                "failed to export {} TypeScript bindings: {error}",
                T::name(config)
            )
        });
    }

    fn write_index(src_dir: &std::path::Path, generated_dir: &std::path::Path) {
        let mut modules = fs::read_dir(generated_dir)
            .expect("read generated protocol directory")
            .map(|entry| entry.expect("read generated protocol entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "ts"))
            .map(|path| {
                path.file_stem()
                    .expect("generated TypeScript file has a stem")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        modules.sort();

        let mut index = String::from(
            "/* @generated by `cargo test -p mag-service --features ts-export export_ts`. Do not edit. */\n",
        );
        for module in modules {
            index.push_str(&format!("export * from \"./generated/{module}\";\n"));
        }

        fs::write(src_dir.join("index.ts"), index).expect("write protocol index");
    }

    #[test]
    fn export_ts() {
        let src_dir = protocol_src_dir();
        let generated_dir = src_dir.join("generated");
        if generated_dir.exists() {
            fs::remove_dir_all(&generated_dir).expect("remove stale generated protocol bindings");
        }
        fs::create_dir_all(&generated_dir).expect("create generated protocol directory");

        let config = Config::new()
            .with_out_dir(&generated_dir)
            .with_large_int("number");

        export_type::<AgentIdWire>(&config);
        export_type::<AgentInstanceStatusWire>(&config);
        export_type::<ApprovalDecisionWire>(&config);
        export_type::<ApprovalRequirementWire>(&config);
        export_type::<Command>(&config);
        export_type::<ConfigDto>(&config);
        export_type::<DelegationMessageWire>(&config);
        export_type::<DelegationStatusWire>(&config);
        export_type::<DelegationTrace>(&config);
        export_type::<Event>(&config);
        export_type::<HistoryEntry>(&config);
        export_type::<InteractionKindWire>(&config);
        export_type::<InteractionOrigin>(&config);
        export_type::<InteractionResponseWire>(&config);
        export_type::<MessageAttachment>(&config);
        export_type::<PermissionCategoryWire>(&config);
        export_type::<PermissionDecisionWire>(&config);
        export_type::<PermissionRiskWire>(&config);
        export_type::<RequestId>(&config);
        export_type::<RunErrorKind>(&config);
        export_type::<RunId>(&config);
        export_type::<RunOutput>(&config);
        export_type::<ServiceError>(&config);
        export_type::<ServiceEvent>(&config);
        export_type::<SessionBudget>(&config);
        export_type::<SessionId>(&config);
        export_type::<SessionInfo>(&config);
        export_type::<SessionStatusWire>(&config);
        export_type::<SourceInfo>(&config);
        export_type::<SourceKindWire>(&config);
        export_type::<StepIdWire>(&config);
        export_type::<ToolCallIdWire>(&config);
        export_type::<ToolStatusWire>(&config);
        export_type::<ToolTrace>(&config);
        export_type::<UsageInfo>(&config);
        export_type::<UserInput>(&config);

        write_index(&src_dir, &generated_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Value, json};
    use std::fmt::Debug;

    fn uuid(offset: u128) -> Uuid {
        Uuid::from_u128(0x018f_0d9c_7b6a_7c12_8f31_0000_0000_0000 + offset)
    }

    fn session_id() -> SessionId {
        SessionId::new(uuid(1))
    }

    fn request_id() -> RequestId {
        RequestId::new(uuid(2))
    }

    fn run_id() -> RunId {
        RunId::new(uuid(3))
    }

    fn agent_id() -> AgentIdWire {
        AgentIdWire::new(uuid(4))
    }

    fn step_id() -> StepIdWire {
        StepIdWire::new(uuid(5))
    }

    fn call_id() -> ToolCallIdWire {
        ToolCallIdWire::new(uuid(6))
    }

    fn attachment() -> MessageAttachment {
        MessageAttachment {
            name: Some("note.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            uri: None,
            text: Some("attached text".to_owned()),
        }
    }

    fn output() -> RunOutput {
        RunOutput {
            text: "done".to_owned(),
            usage: Some(UsageInfo {
                input_tokens: 5,
                output_tokens: 7,
                total_tokens: 12,
            }),
        }
    }

    fn tool_trace(status: ToolStatusWire) -> ToolTrace {
        ToolTrace {
            run_id: Some(run_id()),
            call_id: call_id(),
            name: "read_file".to_owned(),
            input: Some(json!({ "path": "README.md" })),
            output: Some(json!({ "bytes": 10 })),
            status,
            message: None,
        }
    }

    fn delegation_trace(message: Option<&str>) -> DelegationTrace {
        DelegationTrace {
            run_id: Some(run_id()),
            delegate: "codex".to_owned(),
            status: DelegationStatusWire::Finished,
            task: Some("review patch".to_owned()),
            output: Some("looks ok".to_owned()),
            message: message.map(str::to_owned),
            usage: Some(UsageInfo {
                input_tokens: 8,
                output_tokens: 5,
                total_tokens: 13,
            }),
        }
    }

    fn delegation_message() -> DelegationMessageWire {
        DelegationMessageWire {
            run_id: Some(run_id()),
            delegate: "codex".to_owned(),
            text: "child agent update".to_owned(),
        }
    }

    fn interaction_kind() -> InteractionKindWire {
        InteractionKindWire::Permission {
            action_id: "action-1".to_owned(),
            actor: agent_id(),
            category: PermissionCategoryWire::FileWrite,
            risk: PermissionRiskWire::Medium,
            summary: "write a file".to_owned(),
            subject: json!({ "path": "src/lib.rs" }),
            reason: Some("apply requested edit".to_owned()),
        }
    }

    fn response() -> InteractionResponseWire {
        InteractionResponseWire::Permission {
            action_id: "action-1".to_owned(),
            decision: PermissionDecisionWire::Deny {
                reason: Some("outside workspace".to_owned()),
            },
        }
    }

    fn source() -> SourceInfo {
        SourceInfo {
            id: "codex".to_owned(),
            name: "Codex".to_owned(),
            kind: SourceKindWire::LocalAgent,
            available: true,
            version: Some("1.2.3".to_owned()),
            path: Some("/usr/local/bin/codex".to_owned()),
            capabilities: vec!["edit".to_owned(), "shell".to_owned()],
        }
    }

    fn assert_round_trip<T>(value: T)
    where
        T: Debug + PartialEq + Serialize + DeserializeOwned,
    {
        let encoded = serde_json::to_string(&value).expect("serialize value");
        let decoded = serde_json::from_str::<T>(&encoded).expect("deserialize value");
        assert_eq!(decoded, value);
    }

    fn assert_tag<T>(value: &T, expected: &str)
    where
        T: Serialize,
    {
        let json = serde_json::to_value(value).expect("serialize value");
        assert_eq!(json.get("type"), Some(&Value::String(expected.to_owned())));
    }

    #[test]
    fn command_variants_round_trip_and_keep_stable_tags() {
        let cases = vec![
            (
                Command::CreateSession {
                    cwd: Some(std::path::PathBuf::from("/work/session-root")),
                    agent: Some("default".to_owned()),
                },
                "create_session",
            ),
            (Command::ListSessions, "list_sessions"),
            (
                Command::ResumeSession { id: session_id() },
                "resume_session",
            ),
            (
                Command::GetSessionHistory { id: session_id() },
                "get_session_history",
            ),
            (
                Command::DeleteSession { id: session_id() },
                "delete_session",
            ),
            (
                Command::SendMessage {
                    session_id: session_id(),
                    text: "hello".to_owned(),
                    attachments: vec![attachment()],
                },
                "send_message",
            ),
            (
                Command::PivotMessage {
                    session_id: session_id(),
                    text: "steer this run".to_owned(),
                },
                "pivot_message",
            ),
            (
                Command::CancelRun {
                    session_id: session_id(),
                },
                "cancel_run",
            ),
            (
                Command::RespondInteraction {
                    session_id: session_id(),
                    request_id: request_id(),
                    response: response(),
                },
                "respond_interaction",
            ),
            (Command::ListSources, "list_sources"),
            (Command::ProbeLocalAgents, "probe_local_agents"),
            (Command::GetConfig, "get_config"),
            (
                Command::UpdateConfig {
                    config: ConfigDto::default(),
                },
                "update_config",
            ),
            (Command::ReloadConfig, "reload_config"),
            (Command::ApplyConfig, "apply_config"),
        ];

        for (command, expected_tag) in cases {
            assert_tag(&command, expected_tag);
            assert_round_trip(command);
        }
    }

    #[test]
    fn update_config_protocol_fixture_round_trips() {
        let raw =
            include_str!("../../../ui/packages/protocol/test/fixtures/update-config-command.json");
        let value = serde_json::from_str::<Value>(raw).expect("parse protocol JSON fixture");
        let command = serde_json::from_value::<Command>(value.clone())
            .expect("fixture is a valid Command wire payload");

        match &command {
            Command::UpdateConfig { config } => {
                assert!(config.providers.contains_key("anthropic"));
                assert!(config.agents.contains_key("default"));
            }
            other => panic!("expected update_config command fixture, got {other:?}"),
        }

        let encoded = serde_json::to_value(command).expect("serialize fixture command");
        assert_eq!(encoded, value);
    }

    #[test]
    fn event_variants_round_trip_and_keep_stable_tags() {
        let cases = vec![
            (
                Event::SessionCreated {
                    id: session_id(),
                    agent: "default".to_owned(),
                    cwd: Some(std::path::PathBuf::from("/work/session-root")),
                },
                "session_created",
            ),
            (
                Event::RunStarted {
                    id: session_id(),
                    run_id: run_id(),
                },
                "run_started",
            ),
            (
                Event::RunFinished {
                    id: session_id(),
                    output: output(),
                },
                "run_finished",
            ),
            (
                Event::RunError {
                    id: session_id(),
                    message: "failed".to_owned(),
                    kind: RunErrorKind::default(),
                },
                "run_error",
            ),
            (
                Event::TextDelta {
                    id: session_id(),
                    text: "hel".to_owned(),
                },
                "text_delta",
            ),
            (
                Event::ToolStarted {
                    id: session_id(),
                    trace: tool_trace(ToolStatusWire::Started),
                },
                "tool_started",
            ),
            (
                Event::ToolFinished {
                    id: session_id(),
                    trace: tool_trace(ToolStatusWire::Finished),
                },
                "tool_finished",
            ),
            (
                Event::InteractionRequested {
                    id: session_id(),
                    request_id: request_id(),
                    kind: interaction_kind(),
                    origin: InteractionOrigin::default(),
                },
                "interaction_requested",
            ),
            (
                Event::DelegationStarted {
                    id: session_id(),
                    trace: delegation_trace(None),
                },
                "delegation_started",
            ),
            (
                Event::DelegationFinished {
                    id: session_id(),
                    trace: delegation_trace(None),
                },
                "delegation_finished",
            ),
            (
                Event::DelegationFailed {
                    id: session_id(),
                    trace: delegation_trace(Some("delegate crashed")),
                },
                "delegation_failed",
            ),
            (
                Event::DelegationMessage {
                    id: session_id(),
                    message: delegation_message(),
                },
                "delegation_message",
            ),
            (
                Event::AgentInstanceStarted {
                    id: session_id(),
                    instance_id: "explorer-1".to_owned(),
                    agent_type: "explorer".to_owned(),
                    description: Some("map the codebase".to_owned()),
                    depth: 1,
                },
                "agent_instance_started",
            ),
            (
                Event::AgentInstanceFinished {
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
                Event::LocalAgentsProbed {
                    available: vec![source()],
                },
                "local_agents_probed",
            ),
            (Event::PivotQueued { id: session_id() }, "pivot_queued"),
            (Event::PivotApplied { id: session_id() }, "pivot_applied"),
            (
                Event::PivotDropped {
                    id: session_id(),
                    reason: "run cancelled".to_owned(),
                },
                "pivot_dropped",
            ),
            (Event::ConfigChanged { revision: 7 }, "config_changed"),
        ];

        for (event, expected_tag) in cases {
            assert_tag(&event, expected_tag);
            assert_round_trip(event);
        }
    }

    #[test]
    fn agent_instance_status_wire_round_trips_with_stable_tags() {
        let cases = vec![
            (AgentInstanceStatusWire::Completed, "completed"),
            (AgentInstanceStatusWire::Failed, "failed"),
            (AgentInstanceStatusWire::Cancelled, "cancelled"),
        ];

        for (status, expected_tag) in cases {
            let json = serde_json::to_value(status).expect("serialize status");
            assert_eq!(json, Value::String(expected_tag.to_owned()));
            assert_round_trip(status);
        }
    }

    #[test]
    fn history_entry_variants_round_trip_and_keep_stable_tags() {
        let cases = vec![
            (
                HistoryEntry::UserMessage {
                    text: "hello".to_owned(),
                    attachments: vec![attachment()],
                },
                "user_message",
            ),
            (
                HistoryEntry::AssistantMessage {
                    text: "done".to_owned(),
                },
                "assistant_message",
            ),
            (
                HistoryEntry::ToolCall {
                    trace: tool_trace(ToolStatusWire::Finished),
                },
                "tool_call",
            ),
            (
                HistoryEntry::Delegation {
                    trace: delegation_trace(None),
                },
                "delegation",
            ),
        ];

        for (entry, expected_tag) in cases {
            assert_tag(&entry, expected_tag);
            assert_round_trip(entry);
        }
    }

    #[test]
    fn legacy_delegation_trace_defaults_to_started_without_usage() {
        let decoded = serde_json::from_value::<DelegationTrace>(json!({
            "delegate": "codex"
        }))
        .expect("legacy delegation trace");

        assert_eq!(decoded.status, DelegationStatusWire::Started);
        assert_eq!(decoded.usage, None);
    }

    #[test]
    fn interaction_origin_defaults_to_root_and_round_trips() {
        let root = InteractionOrigin::default();
        assert_eq!(root.delegate, None);
        assert_eq!(root.depth, 0);
        assert!(root.is_root());

        let delegated = InteractionOrigin {
            delegate: Some("codex".to_owned()),
            depth: 1,
        };
        assert!(!delegated.is_root());
        assert_round_trip(delegated);
        assert_round_trip(root);

        // A bare `depth`-only payload (the serialized root origin) decodes
        // back to the root origin.
        let decoded = serde_json::from_value::<InteractionOrigin>(json!({ "depth": 0 }))
            .expect("serialized root origin");
        assert_eq!(decoded, InteractionOrigin::default());

        let invalid_root = InteractionOrigin {
            delegate: None,
            depth: 1,
        };
        assert!(
            !invalid_root.is_root(),
            "root attribution requires delegate=None and depth=0"
        );
    }

    #[test]
    fn interaction_requested_without_origin_deserializes_as_root() {
        // Legacy wire shape (pre-`docs/CLI.md` §3.3 attribution): no `origin`
        // key at all. It must decode with the default root origin so events
        // serialized before the field existed stay readable.
        let legacy = json!({
            "type": "interaction_requested",
            "id": session_id(),
            "request_id": request_id(),
            "kind": { "kind": "question", "prompt": "Proceed?" },
        });
        let decoded = serde_json::from_value::<Event>(legacy)
            .expect("legacy interaction_requested without origin");
        match decoded {
            Event::InteractionRequested { origin, .. } => {
                assert_eq!(origin, InteractionOrigin::default());
                assert!(origin.is_root());
            }
            other => panic!("expected interaction_requested, got {other:?}"),
        }
    }

    #[test]
    fn interaction_variants_round_trip() {
        let requirements = vec![
            ApprovalRequirementWire::AutoApprove,
            ApprovalRequirementWire::RequireApproval {
                reason: Some("writes files".to_owned()),
            },
        ];

        for requirement in requirements {
            assert_round_trip(requirement);
        }

        let requests = vec![
            InteractionKindWire::Approval {
                call_id: call_id(),
                requirement: ApprovalRequirementWire::RequireApproval {
                    reason: Some("needs review".to_owned()),
                },
            },
            InteractionKindWire::Question {
                prompt: "Which file?".to_owned(),
            },
            InteractionKindWire::Choice {
                prompt: "Pick one".to_owned(),
                options: vec!["a".to_owned(), "b".to_owned()],
            },
            interaction_kind(),
        ];

        for request in requests {
            assert_round_trip(request);
        }

        let responses = vec![
            InteractionResponseWire::Approval {
                step_id: step_id(),
                call_id: call_id(),
                decision: ApprovalDecisionWire::Approve,
                message: None,
            },
            InteractionResponseWire::Answer {
                text: "answer".to_owned(),
            },
            InteractionResponseWire::Choice { index: 1 },
            response(),
        ];

        for response in responses {
            assert_round_trip(response);
        }
    }

    #[test]
    fn create_session_omits_absent_cwd_and_agent() {
        let json = serde_json::to_value(Command::CreateSession {
            cwd: None,
            agent: None,
        })
        .expect("serialize create_session");
        assert_eq!(json["type"], Value::String("create_session".to_owned()));
        assert!(
            json.get("cwd").is_none() && json.get("agent").is_none(),
            "absent cwd/agent must not round-trip as null keys"
        );

        let decoded = serde_json::from_value::<Command>(json!({ "type": "create_session" }))
            .expect("create_session with no fields");
        assert_eq!(
            decoded,
            Command::CreateSession {
                cwd: None,
                agent: None
            }
        );
    }

    #[test]
    fn uuid_backed_ids_round_trip_as_strings() {
        let id = session_id();
        let json = serde_json::to_value(id).expect("serialize session id");
        assert_eq!(json, Value::String(id.to_string()));

        let parsed = SessionId::parse_str(&id.to_string()).expect("parse session id");
        assert_eq!(parsed, id);
        assert_round_trip(id);
        assert_round_trip(request_id());
        assert_round_trip(run_id());
    }

    #[test]
    fn small_wire_enums_round_trip_every_variant() {
        // §2.4 asks every wire type to be serde round-tripped at least once;
        // keep the small enums covered variant-by-variant so a future rename
        // or new variant cannot silently drift from the TS generated types.
        for kind in [
            RunErrorKind::Other,
            RunErrorKind::Cancelled,
            RunErrorKind::LoopLimitExceeded,
            RunErrorKind::BudgetExhausted,
        ] {
            assert_round_trip(kind);
        }
        for status in [
            ToolStatusWire::Started,
            ToolStatusWire::Finished,
            ToolStatusWire::Denied,
            ToolStatusWire::Cancelled,
            ToolStatusWire::Failed,
        ] {
            assert_round_trip(status);
        }
        for status in [
            DelegationStatusWire::Started,
            DelegationStatusWire::Finished,
            DelegationStatusWire::Failed,
        ] {
            assert_round_trip(status);
        }
        for decision in [
            ApprovalDecisionWire::Approve,
            ApprovalDecisionWire::Deny,
            ApprovalDecisionWire::Timeout,
            ApprovalDecisionWire::Cancel,
        ] {
            assert_round_trip(decision);
        }
        for decision in [
            PermissionDecisionWire::Approve,
            PermissionDecisionWire::Deny {
                reason: Some("outside workspace".to_owned()),
            },
            PermissionDecisionWire::Cancel,
        ] {
            assert_round_trip(decision);
        }
        for kind in [
            SourceKindWire::LlmProvider,
            SourceKindWire::LocalAgent,
            SourceKindWire::ToolRuntime,
            SourceKindWire::Other,
        ] {
            assert_round_trip(kind);
        }
        for status in [
            SessionStatusWire::Idle,
            SessionStatusWire::Running,
            SessionStatusWire::AwaitingInteraction,
        ] {
            assert_round_trip(status);
        }
    }

    #[test]
    fn service_error_round_trips_every_variant() {
        for error in [
            ServiceError::SessionNotFound { id: session_id() },
            ServiceError::InteractionNotFound {
                request_id: request_id(),
            },
            ServiceError::InvalidInput {
                message: "bad input".to_owned(),
            },
            ServiceError::NotPivotable {
                id: session_id(),
                reason: "no running run".to_owned(),
            },
            ServiceError::Unsupported {
                operation: "fly".to_owned(),
            },
            ServiceError::Config {
                message: "invalid field".to_owned(),
            },
            ServiceError::Backend {
                message: "engine down".to_owned(),
            },
        ] {
            assert_round_trip(error);
        }
    }
}
