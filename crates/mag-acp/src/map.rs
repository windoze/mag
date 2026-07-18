//! Pure, IO-free mappings between ACP protocol types and mag-service wire types.
//!
//! Everything here is a pure function so it can be unit-tested fully offline
//! (`docs/ACP.md` §4). This module currently covers the two directions of the
//! `SessionId` mapping (`docs/ACP.md` §4 "SessionId") and the conservative
//! agent-capability declaration used by `initialize` (`docs/ACP.md` §3.1/§7).

use std::fmt;

use agent_client_protocol::schema::v1 as acp;
use mag_service::{RoutingMode, ServiceEvent, SessionConfig, ToolStatusWire, ToolTrace, UserInput};

/// Default AI provider used for ACP-created sessions.
///
/// ACP's `session/new` does not carry an LLM selection (`docs/ACP.md` §3.2), so
/// the first version falls back to a fixed provider. This mirrors the provider
/// used by the frozen `mag-service` contract's own fixtures.
pub const DEFAULT_PROVIDER: &str = "openai";

/// Default model identifier used for ACP-created sessions.
///
/// Like [`DEFAULT_PROVIDER`], this is a placeholder default applied because ACP
/// does not select a model (`docs/ACP.md` §3.2). A later interface/config source
/// can override it without changing this mapping.
pub const DEFAULT_MODEL: &str = "gpt-5-codex";

/// Converts a mag [`SessionId`](mag_service::SessionId) into an ACP
/// [`SessionId`](acp::SessionId).
///
/// The first version deliberately avoids a bidirectional lookup table by using
/// the string form of the mag `SessionId` (a UUID) directly as the ACP
/// `SessionId` value (`docs/ACP.md` §4).
#[must_use]
pub fn mag_session_id_to_acp(id: mag_service::SessionId) -> acp::SessionId {
    acp::SessionId::new(id.to_string())
}

/// Error returned when an inbound ACP [`SessionId`](acp::SessionId) does not
/// correspond to a valid mag [`SessionId`](mag_service::SessionId).
///
/// A mag `SessionId` is a UUID, so an ACP session id string that does not parse
/// as a UUID is a protocol error. The offending string is preserved so callers
/// can surface a meaningful diagnostic. This local type keeps mag-acp's public
/// API free of the underlying `uuid` crate's error type, preserving the crate's
/// tight dependency boundary (`mag-service` + `agent-client-protocol` only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSessionId {
    /// The ACP session id string that failed to parse as a mag `SessionId`.
    pub value: String,
}

impl fmt::Display for InvalidSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid ACP session id (not a valid mag session id): {:?}",
            self.value
        )
    }
}

impl std::error::Error for InvalidSessionId {}

/// Converts an ACP [`SessionId`](acp::SessionId) back into a mag
/// [`SessionId`](mag_service::SessionId).
///
/// This is the inverse of [`mag_session_id_to_acp`]: it parses the ACP session
/// id string as a UUID (`docs/ACP.md` §4).
///
/// # Errors
///
/// Returns [`InvalidSessionId`] when `id` is not a valid mag `SessionId`
/// (i.e. not a UUID string), which the caller should treat as a protocol error.
pub fn acp_session_id_to_mag(
    id: &acp::SessionId,
) -> Result<mag_service::SessionId, InvalidSessionId> {
    let value: &str = id.0.as_ref();
    mag_service::SessionId::parse_str(value).map_err(|_| InvalidSessionId {
        value: value.to_owned(),
    })
}

/// Builds a mag [`SessionConfig`] from an ACP
/// [`NewSessionRequest`](acp::NewSessionRequest) (`docs/ACP.md` §3.2).
///
/// The request's absolute `cwd` becomes the session working root and is carried
/// through unchanged as [`SessionConfig::cwd`] (`Some(req.cwd)`), so mag-core can
/// place it as the facade agent's worktree (M1-3). It is never dropped on the
/// mag-acp side.
///
/// ACP does not carry an LLM selection, so `provider`/`model` fall back to the
/// fixed [`DEFAULT_PROVIDER`] / [`DEFAULT_MODEL`] constants; `tool_profile` is
/// left unset and `routing` uses [`RoutingMode::default`]. The first version
/// ignores `additional_directories` and `mcp_servers` (later source integration
/// points, out of scope here per `docs/ACP.md` §3.2).
#[must_use]
pub fn new_session_request_to_config(req: &acp::NewSessionRequest) -> SessionConfig {
    SessionConfig {
        provider: DEFAULT_PROVIDER.to_owned(),
        model: DEFAULT_MODEL.to_owned(),
        tool_profile: None,
        cwd: Some(req.cwd.clone()),
        routing: RoutingMode::default(),
    }
}

/// Builds the [`AgentCapabilities`](acp::AgentCapabilities) mag advertises during
/// `initialize`.
///
/// Capabilities are declared honestly and conservatively (`docs/ACP.md`
/// §3.1/§7): only bits mag actually supports are turned on. The first version is
/// text-first and does not yet advertise session resumption:
///
/// - `load_session = false` (revisited in M4-2 once mag-core restore readiness is
///   confirmed);
/// - `prompt_capabilities.image` / `.audio` / `.embedded_context` all `false`
///   (multimodal input is out of scope for the first version);
/// - `mcp_capabilities` / `session_capabilities` / `auth` are left at their
///   conservative defaults (unset), and no `auth_methods` are offered.
#[must_use]
pub fn agent_capabilities() -> acp::AgentCapabilities {
    acp::AgentCapabilities::new()
        .load_session(false)
        .prompt_capabilities(
            acp::PromptCapabilities::new()
                .image(false)
                .audio(false)
                .embedded_context(false),
        )
}

/// Wraps a text payload as an ACP
/// [`AgentMessageChunk`](acp::SessionUpdate::AgentMessageChunk) session update.
///
/// This is the streamed representation of assistant text (`docs/ACP.md` §4): the
/// text becomes a [`ContentBlock::Text`](acp::ContentBlock::Text) inside a
/// [`ContentChunk`](acp::ContentChunk).
fn agent_message_chunk(text: impl Into<String>) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::from(
        text.into(),
    )))
}

/// Maps a mag [`ToolStatusWire`] onto the ACP
/// [`ToolCallStatus`](acp::ToolCallStatus) lifecycle (`docs/ACP.md` §4).
///
/// ACP has a coarser lifecycle than mag: it distinguishes only
/// pending/in-progress/completed/failed. mag's `Started` maps to
/// [`InProgress`](acp::ToolCallStatus::InProgress); the successful `Finished`
/// terminal maps to [`Completed`](acp::ToolCallStatus::Completed); and every
/// unsuccessful terminal (`Denied`/`Cancelled`/`Failed`) conservatively maps to
/// [`Failed`](acp::ToolCallStatus::Failed) rather than inventing a finer ACP
/// status. `ToolStatusWire` is `#[non_exhaustive]`, so any future variant falls
/// back to [`InProgress`](acp::ToolCallStatus::InProgress) (the least committal
/// non-terminal state) without fabricating a terminal outcome.
fn tool_status_to_acp(status: ToolStatusWire) -> acp::ToolCallStatus {
    match status {
        ToolStatusWire::Started => acp::ToolCallStatus::InProgress,
        ToolStatusWire::Finished => acp::ToolCallStatus::Completed,
        ToolStatusWire::Denied | ToolStatusWire::Cancelled | ToolStatusWire::Failed => {
            acp::ToolCallStatus::Failed
        }
        _ => acp::ToolCallStatus::InProgress,
    }
}

/// Builds an ACP [`ToolCall`](acp::ToolCall) from a mag [`ToolTrace`] captured at
/// tool-start time (`docs/ACP.md` §4).
///
/// The framework tool-call id ([`call_id`](ToolTrace::call_id), a UUID) becomes
/// the ACP [`ToolCallId`](acp::ToolCallId) so start and finish updates address the
/// same call; the tool [`name`](ToolTrace::name) is used as the human-readable
/// title. The [`status`](ToolTrace::status) is mapped via `tool_status_to_acp`
/// and the tool [`input`](ToolTrace::input) (if any) is carried through unchanged
/// as [`raw_input`](acp::ToolCall::raw_input). The tool [`kind`](acp::ToolKind) is
/// left at its default ([`Other`](acp::ToolKind::Other)) rather than guessing a
/// category from the name.
#[must_use]
pub fn map_tool_call(trace: &ToolTrace) -> acp::ToolCall {
    acp::ToolCall::new(trace.call_id.to_string(), trace.name.clone())
        .status(tool_status_to_acp(trace.status))
        .raw_input(trace.input.clone())
}

/// Builds an ACP [`ToolCallUpdate`](acp::ToolCallUpdate) from a mag [`ToolTrace`]
/// captured at tool-finish time (`docs/ACP.md` §4).
///
/// This reports the terminal state and result of the call identified by
/// [`call_id`](ToolTrace::call_id): the [`status`](ToolTrace::status) is mapped
/// via `tool_status_to_acp`, the tool [`output`](ToolTrace::output) (if any) is
/// carried through as [`raw_output`](acp::ToolCallUpdateFields::raw_output), and a
/// human-readable [`message`](ToolTrace::message) (e.g. an error detail) is
/// surfaced as a text content block so clients can render it.
#[must_use]
pub fn map_tool_call_update(trace: &ToolTrace) -> acp::ToolCallUpdate {
    let mut fields = acp::ToolCallUpdateFields::new()
        .status(tool_status_to_acp(trace.status))
        .raw_output(trace.output.clone());
    if let Some(message) = &trace.message {
        fields = fields.content(vec![acp::ToolCallContent::from(message.clone())]);
    }
    acp::ToolCallUpdate::new(trace.call_id.to_string(), fields)
}

/// Namespaced ACP [`ToolCallId`](acp::ToolCallId) for a delegated child-agent task.
///
/// mag's [`DelegationTrace`](mag_service::DelegationTrace) has no framework tool
/// call id, so the delegation lifecycle is keyed by the stable
/// [`delegate`](mag_service::DelegationTrace::delegate) name under a `delegate:`
/// prefix. This keeps the id stable across a delegation's start/finish updates
/// while remaining disjoint from real tool [`call_id`](ToolTrace::call_id)s (which
/// are UUIDs), so the two id spaces never collide.
fn delegation_tool_call_id(delegate: &str) -> String {
    format!("delegate:{delegate}")
}

/// Maps a mag [`ServiceEvent`] onto the ACP [`SessionUpdate`](acp::SessionUpdate)
/// streamed via `session/update`, or [`None`] when the event produces no update
/// (`docs/ACP.md` §3.4/§4).
///
/// Streaming mappings:
///
/// - [`TextDelta`](ServiceEvent::TextDelta) →
///   [`AgentMessageChunk`](acp::SessionUpdate::AgentMessageChunk).
/// - [`ToolStarted`](ServiceEvent::ToolStarted) →
///   [`ToolCall`](acp::SessionUpdate::ToolCall) via [`map_tool_call`].
/// - [`ToolFinished`](ServiceEvent::ToolFinished) →
///   [`ToolCallUpdate`](acp::SessionUpdate::ToolCallUpdate) via
///   [`map_tool_call_update`].
/// - Delegation lifecycle is represented as a tool (not fabricated `Plan`
///   semantics; `PLAN.md` R-4): [`DelegationStarted`](ServiceEvent::DelegationStarted)
///   → an in-progress [`ToolCall`](acp::SessionUpdate::ToolCall);
///   [`DelegationFinished`](ServiceEvent::DelegationFinished) /
///   [`DelegationFailed`](ServiceEvent::DelegationFailed) → a completed/failed
///   [`ToolCallUpdate`](acp::SessionUpdate::ToolCallUpdate); an intermediate
///   [`DelegationMessage`](ServiceEvent::DelegationMessage) degrades to an
///   [`AgentMessageChunk`](acp::SessionUpdate::AgentMessageChunk) of its text.
///
/// Returns [`None`] for events that deliberately produce no update:
/// [`InteractionRequested`](ServiceEvent::InteractionRequested) is handled by
/// `session/request_permission` (M3); [`RunFinished`](ServiceEvent::RunFinished) /
/// [`RunError`](ServiceEvent::RunError) decide the prompt turn's stop reason (see
/// [`run_terminal_to_stop_reason`]); and lifecycle-only events
/// ([`SessionCreated`](ServiceEvent::SessionCreated),
/// [`RunStarted`](ServiceEvent::RunStarted),
/// [`LocalAgentsProbed`](ServiceEvent::LocalAgentsProbed)) carry no client-facing
/// content. `ServiceEvent` is `#[non_exhaustive]`, so any future variant is
/// conservatively ignored rather than mapped to a fabricated ACP update.
#[must_use]
pub fn service_event_to_session_update(event: &ServiceEvent) -> Option<acp::SessionUpdate> {
    match event {
        ServiceEvent::TextDelta { text, .. } => Some(agent_message_chunk(text.clone())),
        ServiceEvent::ToolStarted { trace, .. } => {
            Some(acp::SessionUpdate::ToolCall(map_tool_call(trace)))
        }
        ServiceEvent::ToolFinished { trace, .. } => Some(acp::SessionUpdate::ToolCallUpdate(
            map_tool_call_update(trace),
        )),
        ServiceEvent::DelegationStarted { trace, .. } => {
            let title = match &trace.task {
                Some(task) => format!("Delegate {}: {task}", trace.delegate),
                None => format!("Delegate {}", trace.delegate),
            };
            let tool_call = acp::ToolCall::new(delegation_tool_call_id(&trace.delegate), title)
                .status(acp::ToolCallStatus::InProgress);
            Some(acp::SessionUpdate::ToolCall(tool_call))
        }
        ServiceEvent::DelegationFinished { trace, .. } => {
            let mut fields =
                acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed);
            if let Some(output) = &trace.output {
                fields = fields.content(vec![acp::ToolCallContent::from(output.clone())]);
            }
            Some(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(delegation_tool_call_id(&trace.delegate), fields),
            ))
        }
        ServiceEvent::DelegationFailed { trace, .. } => {
            let mut fields = acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Failed);
            if let Some(message) = &trace.message {
                fields = fields.content(vec![acp::ToolCallContent::from(message.clone())]);
            }
            Some(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(delegation_tool_call_id(&trace.delegate), fields),
            ))
        }
        ServiceEvent::DelegationMessage { message, .. } => {
            Some(agent_message_chunk(message.text.clone()))
        }
        ServiceEvent::InteractionRequested { .. }
        | ServiceEvent::RunFinished { .. }
        | ServiceEvent::RunError { .. }
        | ServiceEvent::SessionCreated { .. }
        | ServiceEvent::RunStarted { .. }
        | ServiceEvent::LocalAgentsProbed { .. } => None,
        _ => None,
    }
}

/// Collapses a slice of ACP [`ContentBlock`](acp::ContentBlock)s into a mag
/// [`UserInput`] (`docs/ACP.md` §4).
///
/// The first version is text-first: it concatenates the text of every
/// [`ContentBlock::Text`](acp::ContentBlock::Text) block (joined with newlines so
/// distinct blocks stay separated) into [`UserInput::text`]. Non-text blocks
/// ([`Image`](acp::ContentBlock::Image), [`Audio`](acp::ContentBlock::Audio),
/// [`ResourceLink`](acp::ContentBlock::ResourceLink),
/// [`Resource`](acp::ContentBlock::Resource)) are ignored: the corresponding
/// `prompt_capabilities` are not advertised (see [`agent_capabilities`]), so per
/// ACP capability negotiation they will not arrive. `ContentBlock` is
/// `#[non_exhaustive]`, so any future block type is likewise ignored.
#[must_use]
pub fn content_blocks_to_user_input(blocks: &[acp::ContentBlock]) -> UserInput {
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            acp::ContentBlock::Text(content) => Some(content.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    UserInput::text(text)
}

/// Derives the ACP prompt-turn [`StopReason`](acp::StopReason) from a terminal mag
/// [`ServiceEvent`], or [`None`] for non-terminal events (`docs/ACP.md` §3.4).
///
/// A successful [`RunFinished`](ServiceEvent::RunFinished) ends the turn with
/// [`EndTurn`](acp::StopReason::EndTurn); a [`RunError`](ServiceEvent::RunError)
/// maps to [`Refusal`](acp::StopReason::Refusal). mag's event stream does not yet
/// distinguish token/turn-limit exhaustion, so `MaxTokens`/`MaxTurnRequests` are
/// not produced here (`PLAN.md` R-5); cancellation is handled by the
/// `session/cancel` path (M4), not this mapping. Every other event returns
/// [`None`], signalling the prompt pump that the turn has not yet terminated.
#[must_use]
pub fn run_terminal_to_stop_reason(event: &ServiceEvent) -> Option<acp::StopReason> {
    match event {
        ServiceEvent::RunFinished { .. } => Some(acp::StopReason::EndTurn),
        ServiceEvent::RunError { .. } => Some(acp::StopReason::Refusal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed, valid UUID string so tests can build a mag `SessionId` without
    // pulling in the `uuid` crate (keeps mag-acp's dependency boundary intact).
    const SAMPLE_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn session_id_round_trips_mag_to_acp_to_mag() {
        let mag_id = mag_service::SessionId::parse_str(SAMPLE_UUID).expect("valid uuid");

        let acp_id = mag_session_id_to_acp(mag_id);
        assert_eq!(acp_id.0.as_ref(), SAMPLE_UUID);

        let back = acp_session_id_to_mag(&acp_id).expect("round-trip parses");
        assert_eq!(back, mag_id);
    }

    #[test]
    fn acp_session_id_to_mag_rejects_non_uuid() {
        let bogus = acp::SessionId::new("not-a-uuid");
        let err = acp_session_id_to_mag(&bogus).expect_err("non-uuid must be rejected");
        assert_eq!(err.value, "not-a-uuid");
        // Display should mention the offending value for diagnostics.
        assert!(err.to_string().contains("not-a-uuid"));
    }

    #[test]
    fn new_session_request_carries_cwd_and_defaults() {
        let cwd = std::path::PathBuf::from("/abs/work/root");
        let req = acp::NewSessionRequest::new(cwd.clone());

        let config = new_session_request_to_config(&req);

        // The absolute cwd is carried through unchanged (never dropped).
        assert_eq!(config.cwd, Some(cwd));
        // Provider/model fall back to the fixed defaults (ACP carries no LLM
        // selection).
        assert_eq!(config.provider, DEFAULT_PROVIDER);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.tool_profile, None);
        assert_eq!(config.routing, mag_service::RoutingMode::default());
    }

    #[test]
    fn agent_capabilities_are_conservative() {
        let caps = agent_capabilities();
        assert!(!caps.load_session, "load_session must start disabled");
        assert!(!caps.prompt_capabilities.image, "image must be disabled");
        assert!(!caps.prompt_capabilities.audio, "audio must be disabled");
        assert!(
            !caps.prompt_capabilities.embedded_context,
            "embedded_context must be disabled"
        );
    }

    use mag_service::{DelegationMessageWire, DelegationTrace, RunOutput, ToolCallIdWire};

    // A distinct valid UUID for the framework tool-call id in tool traces.
    const TOOL_CALL_UUID: &str = "11111111-2222-3333-4444-555555555555";

    fn sample_session_id() -> mag_service::SessionId {
        mag_service::SessionId::parse_str(SAMPLE_UUID).expect("valid uuid")
    }

    fn tool_call_id() -> ToolCallIdWire {
        ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid")
    }

    fn tool_trace(status: ToolStatusWire) -> ToolTrace {
        ToolTrace {
            run_id: None,
            call_id: tool_call_id(),
            name: "read_file".to_owned(),
            input: Some(serde_json::json!({ "path": "README.md" })),
            output: None,
            status,
            message: None,
        }
    }

    #[test]
    fn text_delta_maps_to_agent_message_chunk() {
        let event = ServiceEvent::TextDelta {
            id: sample_session_id(),
            text: "hello".to_owned(),
        };

        let update = service_event_to_session_update(&event).expect("text delta produces update");
        match update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(text) => assert_eq!(text.text, "hello"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected agent message chunk, got {other:?}"),
        }
    }

    #[test]
    fn tool_started_maps_to_tool_call_with_input_and_running_status() {
        let event = ServiceEvent::ToolStarted {
            id: sample_session_id(),
            trace: tool_trace(ToolStatusWire::Started),
        };

        let update = service_event_to_session_update(&event).expect("tool start produces update");
        match update {
            acp::SessionUpdate::ToolCall(call) => {
                assert_eq!(call.tool_call_id.0.as_ref(), TOOL_CALL_UUID);
                assert_eq!(call.title, "read_file");
                assert_eq!(call.status, acp::ToolCallStatus::InProgress);
                assert_eq!(
                    call.raw_input,
                    Some(serde_json::json!({ "path": "README.md" }))
                );
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn tool_finished_maps_to_tool_call_update_terminal_state() {
        let mut trace = tool_trace(ToolStatusWire::Finished);
        trace.output = Some(serde_json::json!({ "bytes": 42 }));
        let event = ServiceEvent::ToolFinished {
            id: sample_session_id(),
            trace,
        };

        let update = service_event_to_session_update(&event).expect("tool finish produces update");
        match update {
            acp::SessionUpdate::ToolCallUpdate(call_update) => {
                assert_eq!(call_update.tool_call_id.0.as_ref(), TOOL_CALL_UUID);
                assert_eq!(
                    call_update.fields.status,
                    Some(acp::ToolCallStatus::Completed)
                );
                assert_eq!(
                    call_update.fields.raw_output,
                    Some(serde_json::json!({ "bytes": 42 }))
                );
            }
            other => panic!("expected tool call update, got {other:?}"),
        }
    }

    #[test]
    fn tool_failed_status_maps_to_failed_and_surfaces_message() {
        let mut trace = tool_trace(ToolStatusWire::Failed);
        trace.message = Some("boom".to_owned());
        let update = map_tool_call_update(&trace);

        assert_eq!(update.fields.status, Some(acp::ToolCallStatus::Failed));
        let content = update
            .fields
            .content
            .expect("failure message becomes content");
        match content.as_slice() {
            [acp::ToolCallContent::Content(block)] => match &block.content {
                acp::ContentBlock::Text(text) => assert_eq!(text.text, "boom"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected a single content block, got {other:?}"),
        }
    }

    #[test]
    fn denied_and_cancelled_tool_statuses_map_to_failed() {
        for status in [ToolStatusWire::Denied, ToolStatusWire::Cancelled] {
            let update = map_tool_call_update(&tool_trace(status));
            assert_eq!(
                update.fields.status,
                Some(acp::ToolCallStatus::Failed),
                "{status:?} should map to Failed"
            );
        }
    }

    #[test]
    fn delegation_started_maps_to_in_progress_tool_call() {
        let event = ServiceEvent::DelegationStarted {
            id: sample_session_id(),
            trace: DelegationTrace {
                run_id: None,
                delegate: "reviewer".to_owned(),
                task: Some("audit diff".to_owned()),
                output: None,
                message: None,
            },
        };

        let update = service_event_to_session_update(&event).expect("delegation start maps");
        match update {
            acp::SessionUpdate::ToolCall(call) => {
                assert_eq!(call.tool_call_id.0.as_ref(), "delegate:reviewer");
                assert_eq!(call.status, acp::ToolCallStatus::InProgress);
                assert!(call.title.contains("reviewer"));
                assert!(call.title.contains("audit diff"));
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn delegation_finished_and_failed_map_to_matching_tool_call_updates() {
        let finished = ServiceEvent::DelegationFinished {
            id: sample_session_id(),
            trace: DelegationTrace {
                run_id: None,
                delegate: "reviewer".to_owned(),
                task: None,
                output: Some("looks good".to_owned()),
                message: None,
            },
        };
        match service_event_to_session_update(&finished).expect("finish maps") {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert_eq!(update.tool_call_id.0.as_ref(), "delegate:reviewer");
                assert_eq!(update.fields.status, Some(acp::ToolCallStatus::Completed));
                assert!(update.fields.content.is_some());
            }
            other => panic!("expected tool call update, got {other:?}"),
        }

        let failed = ServiceEvent::DelegationFailed {
            id: sample_session_id(),
            trace: DelegationTrace {
                run_id: None,
                delegate: "reviewer".to_owned(),
                task: None,
                output: None,
                message: Some("timeout".to_owned()),
            },
        };
        match service_event_to_session_update(&failed).expect("failure maps") {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert_eq!(update.tool_call_id.0.as_ref(), "delegate:reviewer");
                assert_eq!(update.fields.status, Some(acp::ToolCallStatus::Failed));
            }
            other => panic!("expected tool call update, got {other:?}"),
        }
    }

    #[test]
    fn delegation_message_degrades_to_agent_message_chunk() {
        let event = ServiceEvent::DelegationMessage {
            id: sample_session_id(),
            message: DelegationMessageWire {
                run_id: None,
                delegate: "reviewer".to_owned(),
                text: "progress note".to_owned(),
            },
        };

        match service_event_to_session_update(&event).expect("delegation message maps") {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(text) => assert_eq!(text.text, "progress note"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected agent message chunk, got {other:?}"),
        }
    }

    #[test]
    fn non_update_events_produce_no_session_update() {
        let events = [
            ServiceEvent::RunFinished {
                id: sample_session_id(),
                output: RunOutput {
                    text: String::new(),
                    usage: None,
                },
            },
            ServiceEvent::RunError {
                id: sample_session_id(),
                message: "nope".to_owned(),
            },
            ServiceEvent::RunStarted {
                id: sample_session_id(),
                run_id: mag_service::RunId::parse_str(SAMPLE_UUID).expect("valid uuid"),
            },
            ServiceEvent::LocalAgentsProbed { available: vec![] },
        ];
        for event in &events {
            assert!(
                service_event_to_session_update(event).is_none(),
                "{event:?} must not produce a session update"
            );
        }
    }

    #[test]
    fn content_blocks_concatenate_text_and_ignore_non_text() {
        let blocks = vec![
            acp::ContentBlock::from("first"),
            acp::ContentBlock::Image(acp::ImageContent::new("base64", "image/png")),
            acp::ContentBlock::from("second"),
        ];

        let input = content_blocks_to_user_input(&blocks);

        assert_eq!(input.text, "first\nsecond");
        assert!(input.attachments.is_empty());
    }

    #[test]
    fn content_blocks_empty_yields_empty_input() {
        let input = content_blocks_to_user_input(&[]);
        assert_eq!(input.text, "");
        assert!(input.attachments.is_empty());
    }

    #[test]
    fn stop_reason_maps_terminal_events_only() {
        let finished = ServiceEvent::RunFinished {
            id: sample_session_id(),
            output: RunOutput {
                text: "done".to_owned(),
                usage: None,
            },
        };
        assert_eq!(
            run_terminal_to_stop_reason(&finished),
            Some(acp::StopReason::EndTurn)
        );

        let errored = ServiceEvent::RunError {
            id: sample_session_id(),
            message: "bad".to_owned(),
        };
        assert_eq!(
            run_terminal_to_stop_reason(&errored),
            Some(acp::StopReason::Refusal)
        );

        let streaming = ServiceEvent::TextDelta {
            id: sample_session_id(),
            text: "chunk".to_owned(),
        };
        assert_eq!(run_terminal_to_stop_reason(&streaming), None);
    }
}
