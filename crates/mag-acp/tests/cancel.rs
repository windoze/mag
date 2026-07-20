//! Handler-level tests for `session/cancel` (`docs/ACP.md` §3.5, M4-1).
//!
//! These tests drive the real `agent-client-protocol` client against
//! [`mag_acp::serve`] over an in-memory pipe, with a *scripted*
//! `Arc<dyn MagService>` injected on the service side:
//!
//! - **Plain cancel**: the service streams one text delta, then ends the run
//!   with a `RunError{kind: Cancelled}` when its `cancel` is called. The client
//!   sends `session/cancel` mid-turn and the prompt must resolve with
//!   `StopReason::Cancelled` — the ACP spec mandates this stop reason after
//!   `session/cancel`.
//! - **Cancel during a pending permission**: the service emits an
//!   [`InteractionRequested`](ServiceEvent::InteractionRequested) and then
//!   stays silent, while the fake client never answers the
//!   `session/request_permission`. The cancellation must still end the turn:
//!   the bridge stops waiting, answers the parked interaction conservatively
//!   (and tolerates the service having already resolved it itself —
//!   [`ServiceError::InteractionNotFound`]), and the pump drains the terminal
//!   cancelled `RunError`.
//!
//! No network, real credentials, real Zed, or subprocesses are involved, and
//! every test completes well under a second.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, PromptRequest, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, StopReason,
};
use agent_client_protocol::{
    Agent, Channel, Client, ConnectionTo, Responder, on_receive_notification, on_receive_request,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use mag_service::{
    ApprovalDecisionWire, ApprovalRequirementWire, InteractionKindWire, InteractionOrigin,
    InteractionResponseWire, MagService, RequestId, RunErrorKind, RunId, ServiceError,
    ServiceEvent, SessionConfig, SessionId, SessionInfo, SourceInfo, ToolCallIdWire, UserInput,
};
use tokio::sync::Notify;

// A fixed, valid session UUID the scripted service stamps on every event so the
// pump's `acp_session_id_to_mag` mapping succeeds.
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

fn request_id() -> RequestId {
    RequestId::parse_str(REQUEST_UUID).expect("valid uuid")
}

fn cancelled_run_error() -> ServiceEvent {
    ServiceEvent::RunError {
        id: session_id(),
        message: "run cancelled".to_owned(),
        kind: RunErrorKind::Cancelled,
    }
}

/// What the scripted service records about the cancellation round.
#[derive(Debug, Default)]
struct Recorded {
    /// The session passed to `cancel`, if it was called.
    cancelled: Option<SessionId>,
    /// The response delivered to `respond_interaction`, if it was called.
    response: Option<InteractionResponseWire>,
}

/// A scripted service whose run produces `initial` events and then stays
/// silent until `cancel` is called, at which point it ends the run with a
/// cancelled `RunError` — mirroring mag-core's post-M4-0 cancellation
/// contract. `respond_interaction` answers with `respond_result` so tests can
/// model the service having already resolved the interaction itself.
struct CancelService {
    /// Initial events emitted by `subscribe` before the run goes quiet.
    initial: Vec<ServiceEvent>,
    /// Sender installed by `subscribe`; `cancel` uses it to end the run.
    tx: Mutex<Option<mpsc::UnboundedSender<ServiceEvent>>>,
    /// Recorded observations.
    recorded: Mutex<Recorded>,
    /// Result returned by `respond_interaction`.
    respond_result: Result<(), ServiceError>,
}

impl CancelService {
    fn new(initial: Vec<ServiceEvent>, respond_result: Result<(), ServiceError>) -> Self {
        Self {
            initial,
            tx: Mutex::new(None),
            recorded: Mutex::new(Recorded::default()),
            respond_result,
        }
    }
}

#[async_trait]
impl MagService for CancelService {
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

    async fn send_message(&self, _id: SessionId, _input: UserInput) -> Result<RunId, ServiceError> {
        Ok(RunId::parse_str(RUN_UUID).expect("valid uuid"))
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        self.recorded.lock().expect("lock").cancelled = Some(id);
        // End the run with the structured cancellation the pump maps to
        // `StopReason::Cancelled`, then close the stream.
        if let Some(tx) = self.tx.lock().expect("lock").take() {
            tx.unbounded_send(cancelled_run_error())
                .expect("stream open");
        }
        Ok(())
    }

    async fn pivot_message(&self, _id: SessionId, _input: UserInput) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "pivot_message".to_owned(),
        })
    }

    async fn respond_interaction(
        &self,
        _id: SessionId,
        _request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        self.recorded.lock().expect("lock").response = Some(response);
        self.respond_result.clone()
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let (tx, rx) = mpsc::unbounded();
        for event in &self.initial {
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

/// The observable result of driving one prompt that gets cancelled mid-turn.
struct CancelOutcome {
    stop_reason: StopReason,
    recorded: Recorded,
}

/// Drives `initialize` then a single `session/prompt` against `service`,
/// sends `session/cancel` once `trigger` fires, and reports the outcome.
///
/// `trigger` is signalled when the client may send the cancel: the first
/// streamed `session/update` (plain cancel path) or the arrival of the
/// `session/request_permission` (pending-permission path), whichever the
/// scripted service produces first.
async fn drive_cancel(service: Arc<CancelService>, trigger: Arc<Notify>) -> CancelOutcome {
    let service_dyn: Arc<dyn MagService> = service.clone();

    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service_dyn, agent_transport).await });

    let signal_updates = Arc::clone(&trigger);
    let signal_permission = Arc::clone(&trigger);
    let client = Client
        .builder()
        .on_receive_notification(
            async move |_notification: SessionNotification, _cx: ConnectionTo<Agent>| {
                // The first streamed update proves the run is in flight.
                signal_updates.notify_one();
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |_request: RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        cx: ConnectionTo<Agent>| {
                // A client that never decides: signal the test, then hold the
                // request outstanding *off* the event loop (a handler that
                // pends inline would block the loop and stall the test). The
                // turn must still end on `session/cancel`.
                signal_permission.notify_one();
                cx.spawn(async move {
                    let _responder = responder;
                    std::future::pending::<()>().await;
                    #[allow(unreachable_code)]
                    Ok(())
                })?;
                Ok(())
            },
            on_receive_request!(),
        )
        .connect_with(client_transport, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let acp_sid = mag_acp::map::mag_session_id_to_acp(session_id());
            let trigger_in_closure = Arc::clone(&trigger);
            let prompt = {
                let cx = cx.clone();
                let acp_sid = acp_sid.clone();
                tokio::spawn(async move {
                    cx.send_request(PromptRequest::new(
                        acp_sid,
                        vec![ContentBlock::from("go".to_owned())],
                    ))
                    .block_task()
                    .await
                })
            };
            // Cancel only once the run (or the permission request) is in flight.
            trigger_in_closure.notified().await;
            cx.send_notification(CancelNotification::new(acp_sid))?;
            prompt
                .await
                .expect("prompt task")
                .map(|response| response.stop_reason)
        });

    let stop_reason = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("cancelled turn must not hang")
        .expect("cancelled turn must succeed");

    server.abort();

    CancelOutcome {
        stop_reason,
        recorded: std::mem::take(&mut *service.recorded.lock().expect("lock")),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_ends_prompt_turn_with_cancelled_stop_reason() {
    let service = Arc::new(CancelService::new(
        vec![ServiceEvent::TextDelta {
            id: session_id(),
            text: "working".to_owned(),
        }],
        Ok(()),
    ));
    let outcome = drive_cancel(service, Arc::new(Notify::new())).await;

    assert_eq!(
        outcome.stop_reason,
        StopReason::Cancelled,
        "a turn cancelled via session/cancel must end with StopReason::Cancelled",
    );
    assert_eq!(
        outcome.recorded.cancelled,
        Some(session_id()),
        "the notification must reach MagService::cancel for the right session",
    );
    assert!(
        outcome.recorded.response.is_none(),
        "no interaction was pending, so none should have been answered",
    );
}

/// A scripted service that plays a different script per `subscribe` call, so
/// one session can run two consecutive turns (a cancelled run followed by an
/// approval-gated run).
struct TwoRunService {
    /// Initial events per `subscribe` call, popped in order.
    scripts: Mutex<std::collections::VecDeque<Vec<ServiceEvent>>>,
    /// Sender of the *current* run, installed by `subscribe`.
    tx: Mutex<Option<mpsc::UnboundedSender<ServiceEvent>>>,
    /// Recorded observations.
    recorded: Mutex<Recorded>,
}

#[async_trait]
impl MagService for TwoRunService {
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

    async fn send_message(&self, _id: SessionId, _input: UserInput) -> Result<RunId, ServiceError> {
        Ok(RunId::parse_str(RUN_UUID).expect("valid uuid"))
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        self.recorded.lock().expect("lock").cancelled = Some(id);
        if let Some(tx) = self.tx.lock().expect("lock").take() {
            tx.unbounded_send(cancelled_run_error())
                .expect("stream open");
        }
        Ok(())
    }

    async fn pivot_message(&self, _id: SessionId, _input: UserInput) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "pivot_message".to_owned(),
        })
    }

    async fn respond_interaction(
        &self,
        _id: SessionId,
        _request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        let approved = matches!(
            &response,
            InteractionResponseWire::Approval {
                decision: ApprovalDecisionWire::Approve,
                ..
            }
        );
        self.recorded.lock().expect("lock").response = Some(response);
        // Only an approval completes the run; a cancel/deny answer leaves the
        // terminal event to `cancel` (mirroring a run cancelled mid-approval).
        if approved && let Some(tx) = self.tx.lock().expect("lock").take() {
            tx.unbounded_send(ServiceEvent::RunFinished {
                id: session_id(),
                output: mag_service::RunOutput {
                    text: "done".to_owned(),
                    usage: None,
                },
            })
            .expect("stream open");
        }
        Ok(())
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let (tx, rx) = mpsc::unbounded();
        if let Some(script) = self.scripts.lock().expect("lock").pop_front() {
            for event in script {
                tx.unbounded_send(event).expect("stream open");
            }
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

/// Regression test for the stale-cancel bug found in M4-R review: a `cancel`
/// that ends turn 1 must not leak into turn 2 of the same session. Turn 2
/// pauses on an approval, the client approves, and the turn must complete
/// normally (`EndTurn`) instead of being instantly cancelled by the leftover
/// tracker flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_does_not_leak_into_the_next_turn() {
    use agent_client_protocol::schema::v1::{RequestPermissionOutcome, SelectedPermissionOutcome};

    // Both turns pause on an approval. The cancel must land while turn 1's
    // approval is pending — that is what creates the tracker channel whose
    // stale flag would leak into turn 2 without the pump-start reset.
    let approval = || ServiceEvent::InteractionRequested {
        id: session_id(),
        request_id: request_id(),
        kind: InteractionKindWire::Approval {
            call_id: ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
            requirement: ApprovalRequirementWire::RequireApproval { reason: None },
        },
        origin: InteractionOrigin::default(),
    };
    let service = Arc::new(TwoRunService {
        scripts: Mutex::new(vec![vec![approval()], vec![approval()]].into()),
        tx: Mutex::new(None),
        recorded: Mutex::new(Recorded::default()),
    });
    let service_dyn: Arc<dyn MagService> = service.clone();

    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service_dyn, agent_transport).await });

    let trigger = Arc::new(Notify::new());
    let permission_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let signal_permission = Arc::clone(&trigger);
    let call_count = Arc::clone(&permission_calls);
    let client = Client
        .builder()
        .on_receive_request(
            async move |_request: RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        cx: ConnectionTo<Agent>| {
                let call = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call == 0 {
                    // Turn 1: never answer — the client stays undecided while
                    // the test cancels the turn (held off the event loop).
                    signal_permission.notify_one();
                    cx.spawn(async move {
                        let _responder = responder;
                        std::future::pending::<()>().await;
                        #[allow(unreachable_code)]
                        Ok(())
                    })?;
                    Ok(())
                } else {
                    // Turn 2: approve after a short delay. Without the
                    // pump-start reset the stale cancel flag resolves the
                    // bridge *instantly*, long before this answer arrives.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            mag_acp::map::PERMISSION_OPTION_ALLOW.to_owned(),
                        )),
                    ))
                }
            },
            on_receive_request!(),
        )
        .connect_with(client_transport, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let acp_sid = mag_acp::map::mag_session_id_to_acp(session_id());

            // Turn 1: cancel mid-run.
            let prompt1 = cx.send_request(PromptRequest::new(
                acp_sid.clone(),
                vec![ContentBlock::from("first".to_owned())],
            ));
            let (first, _) = tokio::join!(async { prompt1.block_task().await }, async {
                trigger.notified().await;
                cx.send_notification(CancelNotification::new(acp_sid.clone()))
            });
            let first = first?;

            // Turn 2: pauses on an approval the client approves.
            let second = cx
                .send_request(PromptRequest::new(
                    acp_sid,
                    vec![ContentBlock::from("second".to_owned())],
                ))
                .block_task()
                .await?;
            Ok((first.stop_reason, second.stop_reason))
        });

    let (first, second) = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("two-turn cancel regression must not hang")
        .expect("two-turn cancel regression must succeed");

    server.abort();

    assert_eq!(first, StopReason::Cancelled, "turn 1 was cancelled mid-run");
    assert_eq!(
        second,
        StopReason::EndTurn,
        "turn 2 must not inherit turn 1's cancellation: its approval was approved",
    );
    let response = service
        .recorded
        .lock()
        .expect("lock")
        .response
        .clone()
        .expect("turn 2's approval must have been answered");
    match response {
        InteractionResponseWire::Approval { decision, .. } => {
            assert_eq!(decision, ApprovalDecisionWire::Approve);
        }
        other => panic!("expected an Approval response, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_pending_permission_wraps_up_both_sides() {
    let interaction = ServiceEvent::InteractionRequested {
        id: session_id(),
        request_id: request_id(),
        kind: InteractionKindWire::Approval {
            call_id: ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
            requirement: ApprovalRequirementWire::RequireApproval {
                reason: Some("writes to disk".to_owned()),
            },
        },
        origin: InteractionOrigin::default(),
    };
    // Model the post-M4-0 race: the service already resolved the parked
    // interaction itself on cancellation, so the bridge's late answer finds
    // nothing — and must be tolerated, not surfaced as an error.
    let service = Arc::new(CancelService::new(
        vec![interaction],
        Err(ServiceError::InteractionNotFound {
            request_id: request_id(),
        }),
    ));
    let outcome = drive_cancel(service, Arc::new(Notify::new())).await;

    assert_eq!(
        outcome.stop_reason,
        StopReason::Cancelled,
        "a turn cancelled while a permission request is pending must still end \
         with StopReason::Cancelled (no hang, no spurious error)",
    );
    assert_eq!(outcome.recorded.cancelled, Some(session_id()));

    // The bridge woke from its wait and answered the parked interaction
    // conservatively with a cancel decision before draining the terminal event.
    let response = outcome
        .recorded
        .response
        .expect("the bridge must answer the parked interaction on cancel");
    match response {
        InteractionResponseWire::Approval {
            call_id, decision, ..
        } => {
            assert_eq!(
                call_id,
                ToolCallIdWire::parse_str(TOOL_CALL_UUID).expect("valid uuid"),
            );
            assert_eq!(
                decision,
                ApprovalDecisionWire::Cancel,
                "a cancelled pending permission resolves conservatively with Cancel",
            );
        }
        other => panic!("expected an Approval response, got {other:?}"),
    }
}
