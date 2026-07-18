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
use mag_service::{MagService, ServiceEvent};

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
/// *asynchronous event stream*. ACP requires the handler to run until the turn
/// terminates and then reply with a [`PromptResponse`] carrying the turn's
/// [`StopReason`]; mag drives the turn through
/// [`send_message`](MagService::send_message) and streams its progress through
/// [`subscribe`](MagService::subscribe). The handler therefore runs a *pump* that
/// translates each [`ServiceEvent`] into an outbound `session/update`
/// notification until a terminal event decides the stop reason.
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
/// [`InteractionRequested`](ServiceEvent::InteractionRequested) is a placeholder
/// in this milestone: it is left pending (the run stays paused on the service
/// side) and will be bridged to `session/request_permission` in M3.
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

        // Approvals are bridged to `session/request_permission` in M3; until then
        // the interaction is left pending and the pump keeps waiting.
        if let ServiceEvent::InteractionRequested { .. } = event {
            // TODO(M3): bridge_permission(&service, &connection, session_id, ..).
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
