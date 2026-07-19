//! Protocol-level full-round end-to-end tests for `mag-acp` (`docs/ACP.md` §9,
//! M5-1).
//!
//! A real `agent-client-protocol` client drives [`mag_acp::serve`] over the
//! in-memory `Channel::duplex` pipe through complete rounds —
//! `initialize → session/new → session/prompt` (streaming + one permission
//! round-trip) `→ session/cancel` — with a scripted `Arc<dyn MagService>` on
//! the service side. The tests assert the client-observed `session/update`
//! sequence, the `RequestPermissionRequest` contents, and the prompt's stop
//! reason for each round shape:
//!
//! - full round: streaming + permission approve → `EndTurn`;
//! - plain conversation (no tools, no permission) → `EndTurn`;
//! - mid-prompt cancel → `Cancelled`.
//!
//! Everything is offline: no network, real credentials, real Zed, or
//! subprocesses, and every test completes well under a second. The real-Zed
//! integration skeleton at the bottom is `#[ignore]`d by default.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{
    Agent, Channel, Client, ConnectionTo, Responder, on_receive_notification, on_receive_request,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use mag_service::{
    ApprovalDecisionWire, ApprovalRequirementWire, InteractionKindWire, InteractionResponseWire,
    MagService, RequestId, RunErrorKind, RunId, RunOutput, ServiceError, ServiceEvent,
    SessionConfig, SessionId, SessionInfo, SourceInfo, ToolCallIdWire, UserInput,
};
use tokio::sync::Notify;

// A fixed, valid session UUID the scripted service stamps on every event.
const SESSION_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
// The framework tool-call id carried by the approval interaction.
const TOOL_CALL_UUID: &str = "11111111-2222-3333-4444-555555555555";
// The request identity the service emits and expects back in `respond_interaction`.
const REQUEST_UUID: &str = "22222222-3333-4444-5555-666666666666";
// The run id returned by the scripted `send_message`.
const RUN_UUID: &str = "00000000-0000-0000-0000-000000000001";

fn session_id() -> SessionId {
    SessionId::parse_str(SESSION_UUID).expect("valid uuid")
}

fn text_delta(text: &str) -> ServiceEvent {
    ServiceEvent::TextDelta {
        id: session_id(),
        text: text.to_owned(),
    }
}

fn approval_interaction() -> ServiceEvent {
    ServiceEvent::InteractionRequested {
        id: session_id(),
        request_id: RequestId::parse_str(REQUEST_UUID).expect("valid uuid"),
        kind: InteractionKindWire::Approval {
            call_id: ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
            requirement: ApprovalRequirementWire::RequireApproval {
                reason: Some("writes to disk".to_owned()),
            },
        },
    }
}

/// One scripted run: `initial` events stream on `subscribe`; the run then goes
/// quiet until either `respond_interaction` releases `on_respond` or `cancel`
/// releases `on_cancel` (each closing the stream afterwards).
struct RoundScript {
    initial: Vec<ServiceEvent>,
    on_respond: Vec<ServiceEvent>,
    on_cancel: Vec<ServiceEvent>,
}

/// What the scripted service observed.
#[derive(Debug, Default)]
struct Observed {
    created: Option<SessionConfig>,
    response: Option<InteractionResponseWire>,
    cancelled: Option<SessionId>,
}

/// A scripted service playing one [`RoundScript`].
struct RoundService {
    script: RoundScript,
    tx: Mutex<Option<mpsc::UnboundedSender<ServiceEvent>>>,
    observed: Mutex<Observed>,
}

impl RoundService {
    fn new(script: RoundScript) -> Self {
        Self {
            script,
            tx: Mutex::new(None),
            observed: Mutex::new(Observed::default()),
        }
    }

    /// Sends `events` on the live stream, then drops the sender to close it.
    fn release(&self, events: &[ServiceEvent]) {
        if let Some(tx) = self.tx.lock().expect("lock").take() {
            for event in events {
                tx.unbounded_send(event.clone()).expect("stream open");
            }
        }
    }
}

#[async_trait]
impl MagService for RoundService {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError> {
        self.observed.lock().expect("lock").created = Some(config);
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

    async fn send_message(&self, _id: SessionId, _input: UserInput) -> Result<RunId, ServiceError> {
        Ok(RunId::parse_str(RUN_UUID).expect("valid uuid"))
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        self.observed.lock().expect("lock").cancelled = Some(id);
        self.release(&self.script.on_cancel.clone());
        Ok(())
    }

    async fn respond_interaction(
        &self,
        _id: SessionId,
        _request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        self.observed.lock().expect("lock").response = Some(response);
        self.release(&self.script.on_respond.clone());
        Ok(())
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let (tx, rx) = mpsc::unbounded();
        for event in &self.script.initial {
            tx.unbounded_send(event.clone()).expect("stream open");
        }
        *self.tx.lock().expect("lock") = Some(tx);
        rx.boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }
}

/// What one full-round drive observed on the client side.
struct RoundOutcome {
    stop_reason: StopReason,
    updates: Vec<SessionUpdate>,
    observed: Observed,
}

/// Drives `initialize → session/new → session/prompt` over the in-memory pipe.
///
/// When `cancel_after_start` is set, a `session/cancel` is sent once the first
/// `session/update` (or the permission request) arrives. The client's
/// permission handler asserts the request's shape and approves.
async fn drive_round(script: RoundScript, cancel_after_start: bool) -> RoundOutcome {
    let service = Arc::new(RoundService::new(script));
    let service_dyn: Arc<dyn MagService> = service.clone();

    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service_dyn, agent_transport).await });

    let updates: Arc<Mutex<Vec<SessionUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&updates);
    let in_flight = Arc::new(Notify::new());
    let signal_update = Arc::clone(&in_flight);
    // One permission handler shape for every round: it asserts the request's
    // contents and approves. Rounds without an approval simply never trigger it.
    let client = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx: ConnectionTo<Agent>| {
                sink.lock().expect("lock").push(notification.update);
                signal_update.notify_one();
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        _cx: ConnectionTo<Agent>| {
                // The request must address the awaiting tool call and offer the
                // stable once-scoped allow/reject options (`docs/ACP.md` §5).
                assert_eq!(
                    request.tool_call.tool_call_id.0.as_ref(),
                    TOOL_CALL_UUID,
                    "permission request addresses the paused tool call",
                );
                let kinds: Vec<PermissionOptionKind> =
                    request.options.iter().map(|option| option.kind).collect();
                assert!(
                    kinds.contains(&PermissionOptionKind::AllowOnce)
                        && kinds.contains(&PermissionOptionKind::RejectOnce),
                    "options offer once-scoped allow and reject, got {kinds:?}",
                );
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        mag_acp::map::PERMISSION_OPTION_ALLOW.to_owned(),
                    )),
                ))
            },
            on_receive_request!(),
        );

    let collected = Arc::clone(&updates);
    let client = client.connect_with(client_transport, async move |cx| {
        let initialize = cx
            .send_request(InitializeRequest::new(ProtocolVersion::V1))
            .block_task()
            .await?;
        assert!(initialize.agent_capabilities.load_session);
        let created = cx
            .send_request(NewSessionRequest::new("/abs/session/root"))
            .block_task()
            .await?;
        let prompt = {
            let cx = cx.clone();
            let session_id = created.session_id.clone();
            tokio::spawn(async move {
                cx.send_request(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::from("go".to_owned())],
                ))
                .block_task()
                .await
            })
        };
        if cancel_after_start {
            in_flight.notified().await;
            cx.send_notification(CancelNotification::new(created.session_id))?;
        }
        let response = prompt.await.expect("prompt task")?;
        Ok(response.stop_reason)
    });

    let stop_reason = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("full round must not hang")
        .expect("full round must succeed");

    server.abort();

    RoundOutcome {
        stop_reason,
        updates: collected.lock().expect("lock").clone(),
        observed: std::mem::take(&mut *service.observed.lock().expect("lock")),
    }
}

/// Extracts the concatenated agent-message text from streamed updates.
fn agent_text(updates: &[SessionUpdate]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_streams_permission_and_completes() {
    let outcome = drive_round(
        RoundScript {
            initial: vec![text_delta("draft "), approval_interaction()],
            on_respond: vec![
                text_delta("final"),
                ServiceEvent::RunFinished {
                    id: session_id(),
                    output: RunOutput {
                        text: "draft final".to_owned(),
                        usage: None,
                    },
                },
            ],
            on_cancel: vec![],
        },
        false,
    )
    .await;

    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(
        agent_text(&outcome.updates),
        "draft final",
        "the client observed both streamed chunks around the permission pause",
    );
    assert!(
        outcome.observed.created.is_some(),
        "session/new reached create_session",
    );
    let response = outcome
        .observed
        .response
        .expect("the approval outcome was fed back");
    match response {
        InteractionResponseWire::Approval { decision, .. } => {
            assert_eq!(decision, ApprovalDecisionWire::Approve);
        }
        other => panic!("expected an Approval response, got {other:?}"),
    }
    assert!(
        outcome.observed.cancelled.is_none(),
        "a completed round is never cancelled",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_conversation_ends_with_end_turn() {
    let outcome = drive_round(
        RoundScript {
            initial: vec![
                text_delta("hel"),
                text_delta("lo"),
                ServiceEvent::RunFinished {
                    id: session_id(),
                    output: RunOutput {
                        text: "hello".to_owned(),
                        usage: None,
                    },
                },
            ],
            on_respond: vec![],
            on_cancel: vec![],
        },
        false,
    )
    .await;

    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(agent_text(&outcome.updates), "hello");
    assert!(outcome.observed.response.is_none());
    assert!(outcome.observed.cancelled.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_prompt_cancel_ends_with_cancelled() {
    let outcome = drive_round(
        RoundScript {
            initial: vec![text_delta("working")],
            on_respond: vec![],
            on_cancel: vec![ServiceEvent::RunError {
                id: session_id(),
                message: "run cancelled".to_owned(),
                kind: RunErrorKind::Cancelled,
            }],
        },
        true,
    )
    .await;

    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert_eq!(agent_text(&outcome.updates), "working");
    assert_eq!(
        outcome.observed.cancelled,
        Some(session_id()),
        "session/cancel reached MagService::cancel",
    );
}

/// Real-Zed integration skeleton (`docs/ACP.md` §9): spawns `mag --acp` as a
/// subprocess and performs the `initialize` handshake over its stdio.
///
/// This test is `#[ignore]`d by default: it requires a `mag` binary on `PATH`
/// (e.g. `cargo install --path crates/mag` or adding `target/debug` to `PATH`)
/// and is meant for manual, on-host verification with a real ACP client such
/// as Zed. Run it explicitly with:
///
/// ```text
/// cargo test -p mag-acp --test e2e_acp zed_integration -- --ignored
/// ```
///
/// It is expected to **skip cleanly** (pass) when no `mag` binary is present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a `mag` binary on PATH and a real ACP client setup"]
async fn zed_integration_manual_handshake() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut child = match tokio::process::Command::new("mag")
        .arg("--acp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            eprintln!("skip: no `mag` binary on PATH");
            return;
        }
    };

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));

    stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
        )
        .await
        .expect("write initialize");
    stdin.write_all(b"\n").await.expect("write newline");

    let mut lines = stdout.lines();
    let response = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("mag must answer initialize")
        .expect("stdout readable")
        .expect("a response line");
    assert!(
        response.contains("agentCapabilities"),
        "initialize response carries agent capabilities: {response}",
    );

    child.kill().await.expect("kill mag subprocess");
}
