//! ACP request handlers (`docs/ACP.md` §3).
//!
//! Each handler is a small `async fn` matching the `agent-client-protocol`
//! typed-handler shape (`req`, [`Responder`], [`ConnectionTo<Client>`]). They are
//! registered on the `Agent` builder by [`crate::serve`]. Keeping the bodies here
//! (rather than inline in `serve`) keeps the wiring readable as more methods are
//! added in later milestones.

use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, InitializeRequest, InitializeResponse,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};

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
