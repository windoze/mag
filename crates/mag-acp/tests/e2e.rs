//! Offline protocol-level end-to-end tests for `mag-acp` (`docs/ACP.md` §9).
//!
//! These tests drive the real `agent-client-protocol` client against
//! [`mag_acp::serve`] over an in-memory pipe, with a fake `Arc<dyn MagService>`
//! injected on the service side. No network, real credentials, real Zed, or
//! subprocesses are involved, and every test completes well under a second.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{AgentCapabilities, InitializeRequest};
use agent_client_protocol::{Channel, Client};
use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use mag_service::{
    InteractionResponseWire, MagService, RequestId, RunId, ServiceError, ServiceEvent,
    SessionConfig, SessionId, SessionInfo, SourceInfo, UserInput,
};

// A fixed, valid UUID string so the fake can mint ids without pulling in the
// `uuid` crate (keeps mag-acp's dependency boundary intact, mirroring the M1-1
// unit tests).
const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// A minimal fake service: every method returns a benign default and the event
/// stream is empty. It is enough to exercise handlers (like `initialize`) that
/// do not touch the service, and is the seed for richer scripted fakes in later
/// milestones.
#[derive(Default)]
struct FakeService;

#[async_trait]
impl MagService for FakeService {
    async fn create_session(&self, _config: SessionConfig) -> Result<SessionId, ServiceError> {
        Ok(SessionId::parse_str(NIL_UUID).expect("valid uuid"))
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
        Ok(RunId::parse_str(NIL_UUID).expect("valid uuid"))
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
        stream::empty().boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(Vec::new())
    }
}

/// Drives an `initialize` request from the real ACP client, through an in-memory
/// pipe, into [`mag_acp::serve`], and returns the negotiated agent capabilities.
///
/// The pipe is the acp crate's own [`Channel::duplex`] in-memory transport, which
/// is the shared fixture reused by later milestones' end-to-end tests.
async fn initialize_over_pipe(service: Arc<dyn MagService>) -> AgentCapabilities {
    let (agent_transport, client_transport) = Channel::duplex();

    let server = tokio::spawn(async move { mag_acp::serve(service, agent_transport).await });

    let client = Client
        .builder()
        .connect_with(client_transport, async move |cx| {
            let response = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            Ok(response.agent_capabilities)
        });

    let capabilities = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("initialize round-trip must not hang")
        .expect("initialize round-trip must succeed");

    server.abort();
    capabilities
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_round_trips_over_in_memory_pipe() {
    let service: Arc<dyn MagService> = Arc::new(FakeService);

    let negotiated = initialize_over_pipe(service).await;

    // The capabilities the client observes must equal what the pure function
    // declares — proving the handshake carried them faithfully end to end.
    assert_eq!(negotiated, mag_acp::map::agent_capabilities());
    // And they must stay conservative (M1-1 contract).
    assert!(!negotiated.load_session);
    assert!(!negotiated.prompt_capabilities.image);
}
