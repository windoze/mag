#![warn(missing_docs)]

//! `mag-acp`: the ACP (Agent Client Protocol) agent-side interface for mag.
//!
//! This crate is a thin, IO-free-at-the-core protocol translator that exposes the
//! frozen [`mag_service::MagService`] contract to ACP clients (e.g. Zed). It only
//! depends on `mag-service` and the `agent-client-protocol` crate, and it faces
//! the service exclusively through `Arc<dyn MagService>`.
//!
//! The design of record is [`docs/ACP.md`](../docs/ACP.md); the phase plan lives
//! in [`PLAN.md`](../PLAN.md) and the task list in [`TODO.md`](../TODO.md).
//!
//! This milestone (M1-4) adds the `session/new` request handler on top of the
//! M1-2 [`serve`] entry point: it assembles the ACP `Agent` builder, registers
//! the `initialize`, `session/new`, and placeholder `authenticate` request
//! handlers, and runs the connection over a caller-supplied transport. The
//! single point that wires a concrete `mag-core::Engine` into `serve` is the
//! top-level `mag` binary, which keeps this library's dependency boundary intact.

use std::sync::Arc;

use agent_client_protocol::{Agent, ConnectTo, on_receive_request};
use mag_service::MagService;

mod handlers;
pub mod map;

/// Serves the ACP agent interface for `service` over `transport`.
///
/// This assembles the `agent-client-protocol` [`Agent`] builder, registers the
/// request handlers (currently `initialize` and a placeholder `authenticate`;
/// `docs/ACP.md` §3.1), and drives the connection run loop until `transport`
/// closes. The `service` handle is threaded to the handlers that need it as more
/// ACP methods are implemented in later milestones.
///
/// The `transport` is any `agent-client-protocol` transport that connects to an
/// [`Agent`]: [`Stdio`](agent_client_protocol::Stdio) in production (see the
/// `mag` binary) or an in-memory [`Channel`](agent_client_protocol::Channel) in
/// offline tests (`docs/ACP.md` §9).
///
/// # Errors
///
/// Returns an [`agent_client_protocol::Error`] if the connection fails to run to
/// completion (for example, a transport-level failure).
pub async fn serve<T>(
    service: Arc<dyn MagService>,
    transport: T,
) -> Result<(), agent_client_protocol::Error>
where
    T: ConnectTo<Agent> + 'static,
{
    // `service` is captured by the `session/new` handler closure (and later
    // handlers as more ACP methods are implemented). The async closure moves the
    // handle in and re-`Arc::clone`s it per invocation so the handler can be
    // called for every inbound request.
    Agent
        .builder()
        .name("mag-acp")
        .on_receive_request(
            async move |request, responder, connection| {
                handlers::initialize(request, responder, connection).await
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request, responder, connection| {
                handlers::session_new(Arc::clone(&service), request, responder, connection).await
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request, responder, connection| {
                handlers::authenticate(request, responder, connection).await
            },
            on_receive_request!(),
        )
        .connect_to(transport)
        .await
}
