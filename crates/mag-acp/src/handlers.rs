//! ACP request handlers (`docs/ACP.md` §3).
//!
//! Each handler is a small `async fn` matching the `agent-client-protocol`
//! typed-handler shape (`req`, [`Responder`], [`ConnectionTo<Client>`]). They are
//! registered on the `Agent` builder by [`crate::serve`]. Keeping the bodies here
//! (rather than inline in `serve`) keeps the wiring readable as more methods are
//! added in later milestones.

use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
    StopReason,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use futures::StreamExt;
use mag_service::{InteractionKindWire, MagService, RequestId, ServiceEvent, SessionId};

use crate::map;

/// Handles the ACP `initialize` request (`docs/ACP.md` §3.1).
///
/// Echoes back the client-proposed `protocol_version` (version negotiation,
/// `docs/ACP.md` §7) and advertises mag's conservative
/// [`agent_capabilities`](map::agent_capabilities). No authentication is
/// required, so no `auth_methods` are offered.
pub(crate) async fn initialize(
    request: InitializeRequest,
    responder: Responder<InitializeResponse>,
    _connection: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    responder.respond(
        InitializeResponse::new(request.protocol_version)
            .agent_capabilities(map::agent_capabilities()),
    )
}

/// Handles the ACP `session/new` request (`docs/ACP.md` §3.2).
///
/// Maps the request into a mag [`SessionConfig`](mag_service::SessionConfig) via
/// [`map::new_session_request_to_config`] (carrying the absolute `cwd` through as
/// the session working root), calls
/// [`create_session`](mag_service::MagService::create_session), maps the returned
/// mag [`SessionId`](mag_service::SessionId) into an ACP `SessionId`
/// ([`map::mag_session_id_to_acp`]), and replies with a
/// [`NewSessionResponse`].
///
/// A [`ServiceError`](mag_service::ServiceError) from the service is surfaced to
/// the client as an internal JSON-RPC error.
pub(crate) async fn session_new(
    service: Arc<dyn MagService>,
    request: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
    _connection: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let config = map::new_session_request_to_config(&request);
    match service.create_session(config).await {
        Ok(session_id) => {
            let acp_session_id = map::mag_session_id_to_acp(session_id);
            responder.respond(NewSessionResponse::new(acp_session_id))
        }
        Err(error) => {
            responder.respond_with_error(agent_client_protocol::Error::into_internal_error(error))
        }
    }
}

/// Handles the ACP `session/prompt` request (`docs/ACP.md` §3.4).
///
/// This is the core bridge between ACP's *request/response* prompt turn and mag's
/// *asynchronous event stream*. ACP requires the turn to run until it terminates
/// and then reply with a [`PromptResponse`] carrying the turn's
/// [`StopReason`]; mag drives the turn through
/// [`send_message`](MagService::send_message) and streams its progress through
/// [`subscribe`](MagService::subscribe). The turn therefore runs a *pump*
/// ([`run_prompt_pump`]) that translates each [`ServiceEvent`] into an outbound
/// `session/update` notification until a terminal event decides the stop reason.
///
/// The handler validates the session id, then **spawns** the pump off the
/// connection's event loop (see [`run_prompt_pump`] for why) and returns; the
/// pump answers the prompt through the moved-in `responder` when the turn ends.
///
/// The pump follows `docs/ACP.md` §3.4 precisely:
///
/// 1. map the ACP `SessionId` back to a mag [`SessionId`](mag_service::SessionId)
///    ([`map::acp_session_id_to_mag`]) and the prompt content blocks into a mag
///    [`UserInput`](mag_service::UserInput) ([`map::content_blocks_to_user_input`]);
/// 2. **subscribe before sending** — `subscribe(Some(sid))` is established *before*
///    `send_message`, so no event produced by the run can be lost to a race;
/// 3. pump: each event is mapped with [`map::service_event_to_session_update`] and,
///    when it carries client-facing content, forwarded as a
///    [`SessionNotification`] (`session/update`);
///    [`RunFinished`](ServiceEvent::RunFinished) /
///    [`RunError`](ServiceEvent::RunError) end the turn with the stop reason from
///    [`map::run_terminal_to_stop_reason`]; a stream that ends without a terminal
///    event falls back to [`StopReason::EndTurn`];
/// 4. reply with the resolved [`PromptResponse`].
///
/// Because `subscribe(Some(sid))` filters by session, concurrent prompts for
/// different sessions each pump their own stream without interference.
///
/// [`InteractionRequested`](ServiceEvent::InteractionRequested) is bridged to
/// `session/request_permission` by [`bridge_permission`]: the pump pauses,
/// asks the client to decide, and feeds the decision back into the service with
/// [`respond_interaction`](MagService::respond_interaction) before resuming
/// (`docs/ACP.md` §5).
///
/// An invalid session id, or a [`ServiceError`](mag_service::ServiceError) from
/// [`send_message`](MagService::send_message), is surfaced to the client as an
/// internal JSON-RPC error.
pub(crate) async fn session_prompt(
    service: Arc<dyn MagService>,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = match map::acp_session_id_to_mag(&request.session_id) {
        Ok(session_id) => session_id,
        Err(error) => {
            return responder
                .respond_with_error(agent_client_protocol::Error::into_internal_error(error));
        }
    };

    // The pump is spawned *off* the connection's event loop. Handler callbacks run
    // on that single-task loop and block it until they return, so a pump that ran
    // inline could never receive the inbound `session/request_permission` response
    // it awaits mid-turn — the loop that must deliver that response would be the
    // very loop the pump is blocking (deadlock; see `agent-client-protocol`
    // `SentRequest::block_task`). Spawning frees the loop to serve inbound
    // messages while the pump runs and answers the prompt via `responder` when the
    // turn ends (a deferred response).
    let pump_connection = connection.clone();
    connection.spawn(async move {
        run_prompt_pump(service, request, session_id, responder, pump_connection).await
    })
}

/// Runs the `session/prompt` pump for one turn (spawned by [`session_prompt`]).
///
/// This translates mag's asynchronous [`ServiceEvent`] stream into ACP
/// `session/update` notifications and, on
/// [`InteractionRequested`](ServiceEvent::InteractionRequested), pauses to bridge
/// the approval ([`bridge_permission`]) before resuming. It answers the prompt
/// through `responder` with the resolved [`StopReason`] once the turn terminates.
async fn run_prompt_pump(
    service: Arc<dyn MagService>,
    request: PromptRequest,
    session_id: SessionId,
    responder: Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let input = map::content_blocks_to_user_input(&request.prompt);

    // Subscribe *before* sending so the run cannot emit an event before the pump
    // is listening (`docs/ACP.md` §3.4).
    let mut events = service.subscribe(Some(session_id));
    if let Err(error) = service.send_message(session_id, input).await {
        return responder
            .respond_with_error(agent_client_protocol::Error::into_internal_error(error));
    }

    let stop_reason = loop {
        let Some(event) = events.next().await else {
            // The stream ended without an explicit terminal event; treat the turn
            // as a normal completion.
            break StopReason::EndTurn;
        };

        // Terminal events decide the stop reason and end the pump.
        if let Some(stop_reason) = map::run_terminal_to_stop_reason(&event) {
            break stop_reason;
        }

        // Approvals are an asynchronous pause point: the pump bridges the
        // interaction to `session/request_permission`, awaits the client's
        // decision, and feeds it back into the service before resuming
        // (`docs/ACP.md` §5). The service-side driver stays paused until then, so
        // no further event can arrive while the bridge is in flight.
        if let ServiceEvent::InteractionRequested {
            request_id, kind, ..
        } = event
        {
            if let Err(error) = bridge_permission(
                &service,
                &connection,
                session_id,
                &request.session_id,
                request_id,
                kind,
            )
            .await
            {
                return responder.respond_with_error(error);
            }
            continue;
        }

        // Every other event that carries client-facing content is streamed to the
        // client as a `session/update` notification (`docs/ACP.md` §4).
        if let Some(update) = map::service_event_to_session_update(&event) {
            connection
                .send_notification(SessionNotification::new(request.session_id.clone(), update))?;
        }
    };

    responder.respond(PromptResponse::new(stop_reason))
}

/// Bridges one mag approval round-trip to ACP `session/request_permission`
/// (`docs/ACP.md` §5).
///
/// This is the *asynchronous pause point* of the prompt pump. When the service
/// emits an [`InteractionRequested`](ServiceEvent::InteractionRequested), the
/// pump calls this to:
///
/// 1. map the mag [`InteractionKindWire`] into a
///    [`RequestPermissionRequest`](agent_client_protocol::schema::v1::RequestPermissionRequest)
///    ([`map::interaction_to_permission_request`]);
/// 2. send it to the client and **await** the user's decision
///    (`send_request(..).block_task()`) — the pump does not advance and the
///    service-side driver stays paused until the outcome arrives;
/// 3. translate the [`RequestPermissionOutcome`](agent_client_protocol::schema::v1::RequestPermissionOutcome)
///    back into a mag [`InteractionResponseWire`]
///    ([`map::outcome_to_interaction_response`]);
/// 4. deliver it with [`respond_interaction`](MagService::respond_interaction),
///    which wakes the paused driver.
///
/// Tool [`Approval`](InteractionKindWire::Approval) and privileged-action
/// [`Permission`](InteractionKindWire::Permission) both flow through this single
/// channel (`docs/ACP.md` §5/§6): mag-acp never bypasses the gate.
///
/// # Errors
///
/// Returns an [`agent_client_protocol::Error`] if the outbound
/// `session/request_permission` request fails, or if
/// [`respond_interaction`](MagService::respond_interaction) returns a
/// [`ServiceError`](mag_service::ServiceError) (surfaced as an internal
/// JSON-RPC error).
async fn bridge_permission(
    service: &Arc<dyn MagService>,
    connection: &ConnectionTo<Client>,
    session_id: SessionId,
    acp_session_id: &agent_client_protocol::schema::v1::SessionId,
    request_id: RequestId,
    kind: InteractionKindWire,
) -> Result<(), agent_client_protocol::Error> {
    let request = map::interaction_to_permission_request(acp_session_id, &kind);
    // Await the client's decision — the anti-advance invariant of `docs/ACP.md`
    // §5. The service-side driver stays paused until `respond_interaction` below.
    let response = connection.send_request(request).block_task().await?;
    let mag_response = map::outcome_to_interaction_response(&kind, response.outcome);
    service
        .respond_interaction(session_id, request_id, mag_response)
        .await
        .map_err(agent_client_protocol::Error::into_internal_error)
}

/// Handles the ACP `authenticate` request (`docs/ACP.md` §3.1).
///
/// mag is a local single-user tool whose credentials are managed by its own
/// `CredentialStore` rather than through ACP, so `initialize` advertises no
/// `auth_methods` and a well-behaved client never calls `authenticate`. This
/// placeholder returns an empty success response for robustness against clients
/// that probe it anyway.
pub(crate) async fn authenticate(
    _request: AuthenticateRequest,
    responder: Responder<AuthenticateResponse>,
    _connection: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    responder.respond(AuthenticateResponse::new())
}
