//! Handler-level tests for the `session/prompt` pump (`docs/ACP.md` §3.4, M2-2).
//!
//! These tests drive the real `agent-client-protocol` client against
//! [`mag_acp::serve`] over an in-memory pipe, with a *scripted*
//! `Arc<dyn MagService>` injected on the service side. The scripted service emits
//! a fixed sequence of [`ServiceEvent`]s from `subscribe` and records the order in
//! which `subscribe` / `send_message` were called (and the mapped input), so the
//! tests can assert that the pump:
//!
//! - subscribes *before* it sends the message (no lost events, `docs/ACP.md` §3.4);
//! - translates each event into the expected `session/update` notification (§4);
//! - ends the turn with the stop reason derived from the terminal event
//!   (`RunFinished` → `EndTurn`, `RunError` → `Refusal`, empty stream → `EndTurn`).
//!
//! No network, real credentials, real Zed, or subprocesses are involved, and every
//! test completes well under a second.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, PromptRequest, SessionNotification, SessionUpdate, StopReason,
    ToolCallStatus,
};
use agent_client_protocol::{Agent, Channel, Client, ConnectionTo, on_receive_notification};
use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use mag_service::{
    InteractionResponseWire, MagService, RequestId, RunId, RunOutput, ServiceError, ServiceEvent,
    SessionConfig, SessionId, SessionInfo, SourceInfo, ToolCallIdWire, ToolStatusWire, ToolTrace,
    UserInput,
};

// A fixed, valid UUID the scripted service accepts as the prompt's session id and
// stamps onto every emitted event. It must parse as a UUID so the pump's
// `acp_session_id_to_mag` mapping succeeds (`docs/ACP.md` §4).
const SESSION_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
// A distinct valid UUID for the framework tool-call id carried by tool traces.
const TOOL_CALL_UUID: &str = "11111111-2222-3333-4444-555555555555";
// A valid UUID the scripted `send_message` returns as the started run id.
const RUN_UUID: &str = "00000000-0000-0000-0000-000000000001";

/// Records ordered method calls so tests can assert `subscribe` ran before
/// `send_message` (the anti-race invariant of `docs/ACP.md` §3.4).
type CallLog = Arc<Mutex<Vec<&'static str>>>;

/// A scripted service whose `subscribe` replays a fixed event list and whose
/// `send_message` records the mapped [`UserInput`]. Both methods append to a
/// shared [`CallLog`] so ordering can be asserted.
struct ScriptedService {
    events: Vec<ServiceEvent>,
    call_log: CallLog,
    recorded_input: Arc<Mutex<Option<UserInput>>>,
}

impl ScriptedService {
    fn new(events: Vec<ServiceEvent>) -> Self {
        Self {
            events,
            call_log: Arc::new(Mutex::new(Vec::new())),
            recorded_input: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl MagService for ScriptedService {
    async fn create_session(&self, _config: SessionConfig) -> Result<SessionId, ServiceError> {
        Ok(SessionId::parse_str(SESSION_UUID).expect("valid uuid"))
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

    async fn send_message(&self, _id: SessionId, input: UserInput) -> Result<RunId, ServiceError> {
        self.call_log.lock().expect("lock").push("send_message");
        *self.recorded_input.lock().expect("lock") = Some(input);
        Ok(RunId::parse_str(RUN_UUID).expect("valid uuid"))
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
        self.call_log.lock().expect("lock").push("subscribe");
        stream::iter(self.events.clone()).boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }
}

fn session_id() -> SessionId {
    SessionId::parse_str(SESSION_UUID).expect("valid uuid")
}

fn tool_trace(status: ToolStatusWire) -> ToolTrace {
    ToolTrace {
        run_id: None,
        call_id: ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
        name: "read_file".to_owned(),
        input: Some(serde_json::json!({ "path": "README.md" })),
        output: None,
        status,
        message: None,
    }
}

/// Outcome of driving one `session/prompt` turn through the pump.
struct PromptOutcome {
    stop_reason: StopReason,
    updates: Vec<SessionUpdate>,
}

/// Drives `initialize` then a single `session/prompt` from the real ACP client,
/// through an in-memory pipe, into [`mag_acp::serve`] backed by `service`.
///
/// The client registers a `session/update` notification sink so the caller can
/// assert the exact sequence the pump produced, and returns the negotiated
/// [`StopReason`] alongside those updates.
async fn drive_prompt(service: Arc<dyn MagService>, prompt_text: &str) -> PromptOutcome {
    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service, agent_transport).await });

    let collected: Arc<Mutex<Vec<SessionUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let prompt_text = prompt_text.to_owned();

    let client = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx: ConnectionTo<Agent>| {
                sink.lock().expect("lock").push(notification.update);
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(client_transport, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let response = cx
                .send_request(PromptRequest::new(
                    mag_acp::map::mag_session_id_to_acp(
                        SessionId::parse_str(SESSION_UUID).expect("valid uuid"),
                    ),
                    vec![ContentBlock::from(prompt_text)],
                ))
                .block_task()
                .await?;
            Ok(response.stop_reason)
        });

    let stop_reason = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("session/prompt round-trip must not hang")
        .expect("session/prompt round-trip must succeed");

    server.abort();

    let updates = collected.lock().expect("lock").clone();
    PromptOutcome {
        stop_reason,
        updates,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_pump_streams_updates_and_ends_on_run_finished() {
    // Script: two text deltas, a tool start, a tool finish, then a successful
    // terminal. `RunFinished` produces no update — it only decides the stop reason.
    let events = vec![
        ServiceEvent::TextDelta {
            id: session_id(),
            text: "Hello ".to_owned(),
        },
        ServiceEvent::TextDelta {
            id: session_id(),
            text: "world".to_owned(),
        },
        ServiceEvent::ToolStarted {
            id: session_id(),
            trace: tool_trace(ToolStatusWire::Started),
        },
        ServiceEvent::ToolFinished {
            id: session_id(),
            trace: tool_trace(ToolStatusWire::Finished),
        },
        ServiceEvent::RunFinished {
            id: session_id(),
            output: RunOutput {
                text: "Hello world".to_owned(),
                usage: None,
            },
        },
    ];
    let service = Arc::new(ScriptedService::new(events));
    let call_log = Arc::clone(&service.call_log);
    let recorded_input = Arc::clone(&service.recorded_input);

    let outcome = drive_prompt(service, "please read").await;

    // The pump ran to a normal completion.
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);

    // The four non-terminal events each produced exactly one update, in order.
    assert_eq!(
        outcome.updates.len(),
        4,
        "one update per non-terminal event"
    );

    match &outcome.updates[0] {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(text) => assert_eq!(text.text, "Hello "),
            other => panic!("expected text content, got {other:?}"),
        },
        other => panic!("expected agent message chunk, got {other:?}"),
    }
    match &outcome.updates[1] {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(text) => assert_eq!(text.text, "world"),
            other => panic!("expected text content, got {other:?}"),
        },
        other => panic!("expected agent message chunk, got {other:?}"),
    }
    match &outcome.updates[2] {
        SessionUpdate::ToolCall(tool_call) => {
            assert_eq!(tool_call.tool_call_id.0.as_ref(), TOOL_CALL_UUID);
            assert_eq!(tool_call.title, "read_file");
            assert_eq!(tool_call.status, ToolCallStatus::InProgress);
        }
        other => panic!("expected tool call, got {other:?}"),
    }
    match &outcome.updates[3] {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.tool_call_id.0.as_ref(), TOOL_CALL_UUID);
            assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
        }
        other => panic!("expected tool call update, got {other:?}"),
    }

    // The pump subscribed *before* it sent the message (no lost events).
    assert_eq!(
        *call_log.lock().expect("lock"),
        vec!["subscribe", "send_message"],
    );

    // The prompt content blocks were mapped into the mag `UserInput` unchanged.
    assert_eq!(
        *recorded_input.lock().expect("lock"),
        Some(UserInput::text("please read")),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_pump_maps_run_error_to_refusal() {
    let events = vec![
        ServiceEvent::TextDelta {
            id: session_id(),
            text: "partial".to_owned(),
        },
        ServiceEvent::RunError {
            id: session_id(),
            message: "model refused".to_owned(),
            kind: mag_service::RunErrorKind::Other,
        },
    ];
    let service = Arc::new(ScriptedService::new(events));

    let outcome = drive_prompt(service, "go").await;

    // The one text delta streamed; `RunError` ended the turn with a refusal.
    assert_eq!(outcome.updates.len(), 1);
    assert_eq!(outcome.stop_reason, StopReason::Refusal);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_pump_treats_empty_stream_as_end_turn() {
    // A stream that ends without any terminal event is treated as a normal
    // completion (`docs/ACP.md` §3.4).
    let service = Arc::new(ScriptedService::new(Vec::new()));
    let call_log = Arc::clone(&service.call_log);

    let outcome = drive_prompt(service, "go").await;

    assert!(outcome.updates.is_empty(), "no events, no updates");
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    // Even with no events, subscribe still precedes send_message.
    assert_eq!(
        *call_log.lock().expect("lock"),
        vec!["subscribe", "send_message"],
    );
}
