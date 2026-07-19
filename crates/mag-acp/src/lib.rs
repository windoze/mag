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
//! It assembles the ACP `Agent` builder, registers the `initialize`,
//! `session/new`, `session/load`, `session/prompt` request handlers, the
//! `session/cancel` notification handler (`docs/ACP.md` §3.5), and a
//! placeholder `authenticate`, and runs the connection over a caller-supplied
//! transport. The
//! `session/prompt` handler spawns the *pump* that bridges ACP's
//! request/response prompt turn to mag's asynchronous event stream
//! (`docs/ACP.md` §3.4); on an approval the pump pauses and translates mag's
//! `InteractionRequested` / `respond_interaction` round-trip to ACP
//! `session/request_permission` (`docs/ACP.md` §5), and a shared cancellation
//! tracker lets `session/cancel` wake a pump parked on that bridge so the turn
//! always ends with `StopReason::Cancelled`. The single point that wires a
//! concrete `mag-core::Engine` into `serve` is the top-level `mag` binary,
//! which keeps this library's dependency boundary intact.

use std::sync::Arc;

use agent_client_protocol::{Agent, ConnectTo, on_receive_notification, on_receive_request};
use mag_service::MagService;

mod handlers;
pub mod map;

/// Serves the ACP agent interface for `service` over `transport`.
///
/// This assembles the `agent-client-protocol` [`Agent`] builder, registers the
/// request handlers (currently `initialize` and `session/new`, plus a
/// placeholder `authenticate`; `docs/ACP.md` §3.1/§3.2), and drives the
/// connection run loop until `transport` closes. The `service` handle is threaded
/// to the handlers that need it as more ACP methods are implemented in later
/// milestones.
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
    // `service` is captured by the `session/new`, `session/prompt`, and
    // `session/cancel` handler closures (and later handlers as more ACP methods
    // are implemented). Each async closure moves its own `Arc` clone in and
    // re-`Arc::clone`s it per invocation so the handler can be called for every
    // inbound request. The shared `CancelTracker` lets a `session/cancel`
    // notification wake a prompt pump parked on an approval bridge.
    let cancels = handlers::CancelTracker::default();
    let service_for_new = Arc::clone(&service);
    let service_for_load = Arc::clone(&service);
    let service_for_prompt = Arc::clone(&service);
    let service_for_cancel = service;
    let cancels_for_prompt = cancels.clone();
    let cancels_for_cancel = cancels;
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
                handlers::session_new(Arc::clone(&service_for_new), request, responder, connection)
                    .await
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request, responder, connection| {
                handlers::session_load(
                    Arc::clone(&service_for_load),
                    request,
                    responder,
                    connection,
                )
                .await
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request, responder, connection| {
                handlers::session_prompt(
                    Arc::clone(&service_for_prompt),
                    cancels_for_prompt.clone(),
                    request,
                    responder,
                    connection,
                )
                .await
            },
            on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification, connection| {
                handlers::session_cancel(
                    Arc::clone(&service_for_cancel),
                    cancels_for_cancel.clone(),
                    notification,
                    connection,
                )
                .await
            },
            on_receive_notification!(),
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
