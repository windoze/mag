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
    NewSessionRequest, NewSessionResponse,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use mag_service::MagService;

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
