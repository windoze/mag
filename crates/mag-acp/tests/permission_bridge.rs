//! Handler-level tests for the approval bridge (`docs/ACP.md` §5, M3-2).
//!
//! These tests drive the real `agent-client-protocol` client against
//! [`mag_acp::serve`] over an in-memory pipe, with a *scripted*
//! `Arc<dyn MagService>` injected on the service side. The scripted service's
//! `subscribe` stream emits some client-facing events, then an
//! [`InteractionRequested`](ServiceEvent::InteractionRequested), and then — to
//! faithfully model mag's asynchronous approval pause — **blocks** until
//! [`respond_interaction`](MagService::respond_interaction) is called, at which
//! point it releases the remaining events.
//!
//! A fake ACP client registers a `session/request_permission` request handler
//! that returns `Selected{allow}`, `Selected{reject}`, or `Cancelled`. The tests
//! assert that the pump:
//!
//! - is a genuine pause point: the post-interaction sentinel event has *not*
//!   streamed yet when the client is asked to decide (`docs/ACP.md` §5);
//! - feeds the client's outcome back into the service via `respond_interaction`
//!   with the family-correct [`InteractionResponseWire`] (approve / deny /
//!   cancel);
//! - resumes and finishes the turn once the decision is delivered.
//!
//! No network, real credentials, real Zed, or subprocesses are involved, and
//! every test completes well under a second.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{
    Agent, Channel, Client, ConnectionTo, Responder, on_receive_notification, on_receive_request,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use mag_service::{
    ApprovalDecisionWire, ApprovalRequirementWire, HistoryEntry, InteractionKindWire,
    InteractionOrigin, InteractionResponseWire, MagService, RequestId, RunId, RunOutput,
    ServiceError, ServiceEvent, SessionConfig, SessionId, SessionInfo, SourceInfo, ToolCallIdWire,
    UserInput,
};

// A fixed, valid session UUID the scripted service stamps on every event so the
// pump's `acp_session_id_to_mag` mapping succeeds.
const SESSION_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
// The framework tool-call id carried by the approval interaction.
const TOOL_CALL_UUID: &str = "11111111-2222-3333-4444-555555555555";
// The request identity the service emits and expects back in `respond_interaction`.
const REQUEST_UUID: &str = "22222222-3333-4444-5555-666666666666";
// The run id returned by the scripted `send_message`.
const RUN_UUID: &str = "00000000-0000-0000-0000-000000000001";

// Text of the event streamed *before* the interaction (must be visible to the
// client) and the sentinel streamed *after* the decision is delivered (must not
// be visible until then).
const BEFORE_TEXT: &str = "before";
const AFTER_TEXT: &str = "after";

fn session_id() -> SessionId {
    SessionId::parse_str(SESSION_UUID).expect("valid uuid")
}

fn request_id() -> RequestId {
    RequestId::parse_str(REQUEST_UUID).expect("valid uuid")
}

fn approval_interaction() -> ServiceEvent {
    ServiceEvent::InteractionRequested {
        id: session_id(),
        request_id: request_id(),
        kind: InteractionKindWire::Approval {
            call_id: ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
            requirement: ApprovalRequirementWire::RequireApproval {
                reason: Some("writes to disk".to_owned()),
            },
        },
        origin: InteractionOrigin::default(),
    }
}

/// A scripted service that models mag's asynchronous approval pause.
///
/// `subscribe` buffers the pre-interaction events and the interaction itself
/// immediately, then stashes the sender so that `respond_interaction` can release
/// the post-interaction events. This makes the post-interaction sentinel
/// unreachable until the pump has delivered the decision — exactly the invariant
/// the pause-point tests assert.
struct BridgeService {
    /// Sender for the post-interaction events, installed by `subscribe` and
    /// drained (then dropped, closing the stream) by `respond_interaction`.
    pending: Mutex<Option<(mpsc::UnboundedSender<ServiceEvent>, Vec<ServiceEvent>)>>,
    /// The `(session, request_id, response)` recorded when the driver is woken.
    recorded: Mutex<Option<(SessionId, RequestId, InteractionResponseWire)>>,
    /// Set true once `respond_interaction` runs, proving the driver resumed.
    resumed: Arc<AtomicBool>,
}

impl BridgeService {
    fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            recorded: Mutex::new(None),
            resumed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl MagService for BridgeService {
    async fn create_session(&self, _config: SessionConfig) -> Result<SessionId, ServiceError> {
        Ok(session_id())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn resume_session(&self, _id: SessionId) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn get_session_history(&self, _id: SessionId) -> Result<Vec<HistoryEntry>, ServiceError> {
        Ok(Vec::new())
    }

    async fn delete_session(&self, _id: SessionId) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn send_message(&self, _id: SessionId, _input: UserInput) -> Result<RunId, ServiceError> {
        Ok(RunId::parse_str(RUN_UUID).expect("valid uuid"))
    }

    async fn cancel(&self, _id: SessionId) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn pivot_message(&self, _id: SessionId, _input: UserInput) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "pivot_message".to_owned(),
        })
    }

    async fn respond_interaction(
        &self,
        id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        *self.recorded.lock().expect("lock") = Some((id, request_id, response));
        self.resumed.store(true, Ordering::SeqCst);
        // Release the post-interaction events, then drop the sender so the stream
        // ends after they drain — mirroring a driver that only progresses once the
        // approval is answered.
        if let Some((tx, post)) = self.pending.lock().expect("lock").take() {
            for event in post {
                tx.unbounded_send(event).expect("stream open");
            }
        }
        Ok(())
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let (tx, rx) = mpsc::unbounded();
        // Pre-interaction events + the interaction are available immediately.
        tx.unbounded_send(ServiceEvent::TextDelta {
            id: session_id(),
            text: BEFORE_TEXT.to_owned(),
        })
        .expect("stream open");
        tx.unbounded_send(approval_interaction())
            .expect("stream open");
        // The post-interaction sentinel + terminal are withheld until the driver
        // is woken by `respond_interaction`.
        let post = vec![
            ServiceEvent::TextDelta {
                id: session_id(),
                text: AFTER_TEXT.to_owned(),
            },
            ServiceEvent::RunFinished {
                id: session_id(),
                output: RunOutput {
                    text: format!("{BEFORE_TEXT}{AFTER_TEXT}"),
                    usage: None,
                },
            },
        ];
        *self.pending.lock().expect("lock") = Some((tx, post));
        rx.boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn get_config(&self) -> Result<mag_service::ConfigDto, ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "get_config".to_owned(),
        })
    }

    async fn update_config(&self, _config: mag_service::ConfigDto) -> Result<(), ServiceError> {
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

/// The decision the fake client returns for `session/request_permission`.
#[derive(Clone, Copy)]
enum ClientDecision {
    Allow,
    Reject,
    Cancel,
}

impl ClientDecision {
    fn outcome(self) -> RequestPermissionOutcome {
        match self {
            ClientDecision::Allow => RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(mag_acp::map::PERMISSION_OPTION_ALLOW.to_owned()),
            ),
            ClientDecision::Reject => RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(mag_acp::map::PERMISSION_OPTION_REJECT.to_owned()),
            ),
            ClientDecision::Cancel => RequestPermissionOutcome::Cancelled,
        }
    }
}

/// Whether any streamed update carries `needle` as agent-message text.
fn contains_text(updates: &[SessionUpdate], needle: &str) -> bool {
    updates.iter().any(|update| match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(text) => text.text.contains(needle),
            _ => false,
        },
        _ => false,
    })
}

/// The observable result of driving one approval round-trip.
struct BridgeOutcome {
    stop_reason: StopReason,
    updates: Vec<SessionUpdate>,
    recorded: Option<(SessionId, RequestId, InteractionResponseWire)>,
    /// True iff the post-interaction sentinel had *not* streamed when the client
    /// was asked to decide (the pause invariant held).
    paused_before_decision: bool,
    resumed: bool,
}

/// Drives `initialize` then a single `session/prompt` whose run pauses on an
/// approval, answering it with `decision`, and reports what the pump did.
async fn drive_bridge(decision: ClientDecision) -> BridgeOutcome {
    let service = Arc::new(BridgeService::new());
    let recorded = &service.recorded;
    let resumed = Arc::clone(&service.resumed);
    let service_dyn: Arc<dyn MagService> = Arc::clone(&service) as Arc<dyn MagService>;

    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service_dyn, agent_transport).await });

    let collected: Arc<Mutex<Vec<SessionUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let peek = Arc::clone(&collected);
    // Set true if the pause invariant is violated (sentinel already streamed when
    // the client is asked to decide). Starts as "held".
    let paused = Arc::new(AtomicBool::new(true));
    let paused_flag = Arc::clone(&paused);

    let client = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx: ConnectionTo<Agent>| {
                sink.lock().expect("lock").push(notification.update);
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |_request: RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        _cx: ConnectionTo<Agent>| {
                // Pause invariant: the driver must not have advanced past the
                // interaction, so the post-interaction sentinel cannot be here yet.
                if contains_text(&peek.lock().expect("lock"), AFTER_TEXT) {
                    paused_flag.store(false, Ordering::SeqCst);
                }
                responder.respond(RequestPermissionResponse::new(decision.outcome()))
            },
            on_receive_request!(),
        )
        .connect_with(client_transport, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let response = cx
                .send_request(PromptRequest::new(
                    mag_acp::map::mag_session_id_to_acp(session_id()),
                    vec![ContentBlock::from("go".to_owned())],
                ))
                .block_task()
                .await?;
            Ok(response.stop_reason)
        });

    let stop_reason = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("approval round-trip must not hang")
        .expect("approval round-trip must succeed");

    server.abort();

    BridgeOutcome {
        stop_reason,
        updates: collected.lock().expect("lock").clone(),
        recorded: recorded.lock().expect("lock").clone(),
        paused_before_decision: paused.load(Ordering::SeqCst),
        resumed: resumed.load(Ordering::SeqCst),
    }
}

/// Asserts the pause/resume sequencing common to every decision: the client was
/// asked to decide before the driver advanced, the driver was then resumed, and
/// both the pre- and post-interaction updates ultimately streamed.
fn assert_bridged(outcome: &BridgeOutcome) {
    assert!(
        outcome.paused_before_decision,
        "pump must pause on the approval before streaming post-interaction events",
    );
    assert!(
        outcome.resumed,
        "respond_interaction must be called to resume the driver",
    );
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert!(
        contains_text(&outcome.updates, BEFORE_TEXT),
        "the pre-interaction update must stream",
    );
    assert!(
        contains_text(&outcome.updates, AFTER_TEXT),
        "the post-interaction update must stream once resumed",
    );
}

/// Asserts the recorded response is an approval decision for the awaiting tool
/// call, and returns the decision + message for the caller to check.
fn recorded_approval(outcome: &BridgeOutcome) -> (ApprovalDecisionWire, Option<String>) {
    let (id, req_id, response) = outcome
        .recorded
        .clone()
        .expect("respond_interaction must have been called");
    assert_eq!(id, session_id(), "response is keyed to the paused session");
    assert_eq!(
        req_id,
        request_id(),
        "response echoes the emitted request id"
    );
    match response {
        InteractionResponseWire::Approval {
            call_id,
            decision,
            message,
            ..
        } => {
            assert_eq!(
                call_id,
                ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
                "response addresses the awaiting tool call",
            );
            (decision, message)
        }
        other => panic!("expected an Approval response, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_bridge_forwards_allow_decision() {
    let outcome = drive_bridge(ClientDecision::Allow).await;
    assert_bridged(&outcome);

    let (decision, message) = recorded_approval(&outcome);
    assert_eq!(decision, ApprovalDecisionWire::Approve);
    assert_eq!(message, None, "an approval carries no denial message");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_bridge_forwards_reject_decision() {
    let outcome = drive_bridge(ClientDecision::Reject).await;
    assert_bridged(&outcome);

    let (decision, message) = recorded_approval(&outcome);
    assert_eq!(decision, ApprovalDecisionWire::Deny);
    assert_eq!(message, None, "a plain deny carries no message");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_bridge_maps_cancelled_outcome_to_cancel() {
    let outcome = drive_bridge(ClientDecision::Cancel).await;
    assert_bridged(&outcome);

    let (decision, message) = recorded_approval(&outcome);
    assert_eq!(decision, ApprovalDecisionWire::Cancel);
    assert!(
        message.is_some(),
        "a cancellation carries a model-visible message",
    );
}
