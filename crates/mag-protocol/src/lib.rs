#![warn(missing_docs)]

//! Transport-neutral command and event protocol types for mag.
//!
//! This crate owns the wire contract between front doors and `mag-core`. The
//! types are pure serde data and deliberately do not depend on `agent-lib`, even
//! when they mirror concepts from that crate.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, str::FromStr};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Create a new session with the supplied configuration.
    CreateSession {
        /// Session configuration to persist and use for the driver.
        config: SessionConfig,
    },
    /// List currently known sessions.
    ListSessions,
    /// Resume an existing session.
    ResumeSession {
        /// Session to resume.
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
}

/// Events emitted by the transport-neutral engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
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
    /// The engine is waiting for an external interaction response.
    InteractionRequested {
        /// Session that owns the pending interaction.
        id: SessionId,
        /// Request identity to pass back in `RespondInteraction`.
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
    /// Local coding-agent probes completed.
    LocalAgentsProbed {
        /// Available sources discovered by the probe.
        available: Vec<SourceInfo>,
    },
}

/// Configuration used when creating or resuming a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// AI provider or source key, such as `openai` or `local`.
    pub provider: String,
    /// Model identifier understood by the selected provider.
    pub model: String,
    /// Named tool profile to attach to the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_profile: Option<String>,
    /// Routing strategy for future delegation support.
    #[serde(default)]
    pub routing: RoutingMode,
}

/// Delegation routing mode reserved in the session configuration.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// Supervisor model decides when and where to delegate.
    #[default]
    ModelRouted,
    /// A dispatcher component decides which delegate should receive a task.
    Dispatcher,
}

/// Optional user-message attachment metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Final output reported for a successful run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunOutput {
    /// Final assistant text collected for the run.
    pub text: String,
    /// Optional usage accounting supplied by the model provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
}

/// Provider-neutral token usage summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct DelegationTrace {
    /// Optional parent run identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable delegate name or source key.
    pub delegate: String,
    /// Optional delegated task description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Optional delegated output text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Optional failure or status message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Interaction request shown to a user or policy engine.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

    fn config() -> SessionConfig {
        SessionConfig {
            provider: "openai".to_owned(),
            model: "gpt-5-codex".to_owned(),
            tool_profile: Some("default".to_owned()),
            routing: RoutingMode::ModelRouted,
        }
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
            task: Some("review patch".to_owned()),
            output: Some("looks ok".to_owned()),
            message: message.map(str::to_owned),
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
                Command::CreateSession { config: config() },
                "create_session",
            ),
            (Command::ListSessions, "list_sessions"),
            (
                Command::ResumeSession { id: session_id() },
                "resume_session",
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
        ];

        for (command, expected_tag) in cases {
            assert_tag(&command, expected_tag);
            assert_round_trip(command);
        }
    }

    #[test]
    fn event_variants_round_trip_and_keep_stable_tags() {
        let cases = vec![
            (
                Event::SessionCreated {
                    id: session_id(),
                    config: config(),
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
                Event::LocalAgentsProbed {
                    available: vec![source()],
                },
                "local_agents_probed",
            ),
        ];

        for (event, expected_tag) in cases {
            assert_tag(&event, expected_tag);
            assert_round_trip(event);
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
    fn session_config_defaults_to_model_routed() {
        let decoded = serde_json::from_value::<SessionConfig>(json!({
            "provider": "openai",
            "model": "gpt-5-codex"
        }))
        .expect("config with default routing");

        assert_eq!(decoded.routing, RoutingMode::ModelRouted);
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
}
