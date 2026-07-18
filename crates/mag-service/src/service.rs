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
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

use crate::{
    DelegationMessageWire, DelegationTrace, Event, InteractionKindWire, InteractionResponseWire,
    RequestId, RunId, RunOutput, SessionConfig, SessionId, SourceInfo, ToolTrace,
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
}

/// Neutral user input supplied to [`MagService::send_message`].
///
/// This is the transport-neutral counterpart of the
/// [`Command::SendMessage`](crate::Command::SendMessage) wire payload.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct SessionInfo {
    /// Stable session identity.
    pub id: SessionId,
    /// Configuration the session was created with.
    pub config: SessionConfig,
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
    InteractionRequested {
        /// Session that owns the pending interaction.
        id: SessionId,
        /// Request identity to pass back in
        /// [`MagService::respond_interaction`].
        request_id: RequestId,
        /// Interaction details to render or decide.
        kind: InteractionKindWire,
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
    /// Local coding-agent probes completed.
    LocalAgentsProbed {
        /// Available sources discovered by the probe.
        available: Vec<SourceInfo>,
    },
}

impl ServiceEvent {
    /// Returns the session this event is scoped to, if any.
    ///
    /// Session-scoped variants return `Some`; global events such as
    /// [`LocalAgentsProbed`](ServiceEvent::LocalAgentsProbed) return `None`.
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
            | Self::DelegationMessage { id, .. } => Some(*id),
            Self::LocalAgentsProbed { .. } => None,
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
            Event::RunError { id, message } => Self::RunError { id, message },
            Event::TextDelta { id, text } => Self::TextDelta { id, text },
            Event::ToolStarted { id, trace } => Self::ToolStarted { id, trace },
            Event::ToolFinished { id, trace } => Self::ToolFinished { id, trace },
            Event::InteractionRequested {
                id,
                request_id,
                kind,
            } => Self::InteractionRequested {
                id,
                request_id,
                kind,
            },
            Event::DelegationStarted { id, trace } => Self::DelegationStarted { id, trace },
            Event::DelegationFinished { id, trace } => Self::DelegationFinished { id, trace },
            Event::DelegationFailed { id, trace } => Self::DelegationFailed { id, trace },
            Event::DelegationMessage { id, message } => Self::DelegationMessage { id, message },
            Event::LocalAgentsProbed { available } => Self::LocalAgentsProbed { available },
        }
    }
}

/// Error returned by [`MagService`] operations.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The requested operation is not supported by this implementation.
    Unsupported {
        /// Operation name that is not supported.
        operation: String,
    },
    /// The backing engine failed to complete the operation.
    Backend {
        /// Human-readable failure detail.
        message: String,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound { id } => write!(formatter, "session `{id}` not found"),
            Self::InteractionNotFound { request_id } => {
                write!(formatter, "interaction `{request_id}` not found")
            }
            Self::InvalidInput { message } => write!(formatter, "invalid input: {message}"),
            Self::Unsupported { operation } => {
                write!(formatter, "operation `{operation}` is not supported")
            }
            Self::Backend { message } => write!(formatter, "backend error: {message}"),
        }
    }
}

impl Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionCategoryWire, PermissionRiskWire, ToolStatusWire, UsageInfo};
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
            routing: crate::RoutingMode::ModelRouted,
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
            task: Some("review patch".to_owned()),
            output: None,
            message: None,
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
                ServiceEvent::LocalAgentsProbed {
                    available: vec![source()],
                },
                "local_agents_probed",
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
                Event::LocalAgentsProbed {
                    available: vec![source()],
                },
                ServiceEvent::LocalAgentsProbed {
                    available: vec![source()],
                },
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(ServiceEvent::from(event), expected);
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
            ServiceEvent::LocalAgentsProbed {
                available: Vec::new(),
            }
            .session_id(),
            None,
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
}
