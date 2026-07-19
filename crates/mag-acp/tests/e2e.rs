//! Offline protocol-level end-to-end tests for `mag-acp` (`docs/ACP.md` §9).
//!
//! These tests drive the real `agent-client-protocol` client against
//! [`mag_acp::serve`] over an in-memory pipe, with a fake `Arc<dyn MagService>`
//! injected on the service side. No network, real credentials, real Zed, or
//! subprocesses are involved, and every test completes well under a second.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{AgentCapabilities, InitializeRequest, NewSessionRequest};
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

// A distinct, non-nil UUID the fake returns from `create_session`, so the
// `session/new` round-trip test can prove the ACP `SessionId` maps back to the
// exact mag `SessionId` the service produced.
const SESSION_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// A minimal fake service: every method returns a benign default and the event
/// stream is empty. `create_session` additionally records the received
/// [`SessionConfig`] (so handler tests can assert the mapping, e.g. that the ACP
/// `cwd` is carried through) and returns a fixed [`SESSION_UUID`] session id. It
/// is enough to exercise the M1 handlers and is the seed for richer scripted
/// fakes in later milestones.
#[derive(Default)]
struct FakeService {
    recorded_config: Arc<Mutex<Option<SessionConfig>>>,
    recorded_resume: Arc<Mutex<Option<SessionId>>>,
}

#[async_trait]
impl MagService for FakeService {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError> {
        *self.recorded_config.lock().expect("lock not poisoned") = Some(config);
        Ok(SessionId::parse_str(SESSION_UUID).expect("valid uuid"))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        Ok(Vec::new())
    }

    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
        *self.recorded_resume.lock().expect("lock not poisoned") = Some(id);
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
    let service: Arc<dyn MagService> = Arc::new(FakeService::default());

    let negotiated = initialize_over_pipe(service).await;

    // The capabilities the client observes must equal what the pure function
    // declares — proving the handshake carried them faithfully end to end.
    assert_eq!(negotiated, mag_acp::map::agent_capabilities());
    // `load_session` is advertised (M4-2: mag-core restore is ready); the
    // multimodal prompt bits stay off.
    assert!(negotiated.load_session);
    assert!(!negotiated.prompt_capabilities.image);
}

/// Drives `initialize` then `session/new` from the real ACP client, through the
/// in-memory pipe, into [`mag_acp::serve`], and proves the mapping round-trips:
/// the fake service records a [`SessionConfig`] whose `cwd` equals the request's
/// absolute `cwd` (M1-3/M1-4 contract — the client cwd is carried through, never
/// dropped), and the ACP `SessionId` returned to the client maps back to the exact
/// mag `SessionId` the service produced (`docs/ACP.md` §3.2/§4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_new_round_trips_over_in_memory_pipe() {
    let recorded: Arc<Mutex<Option<SessionConfig>>> = Arc::new(Mutex::new(None));
    let service: Arc<dyn MagService> = Arc::new(FakeService {
        recorded_config: Arc::clone(&recorded),
        ..FakeService::default()
    });

    let cwd = PathBuf::from("/abs/session/root");
    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service, agent_transport).await });

    let cwd_for_client = cwd.clone();
    let client = Client
        .builder()
        .connect_with(client_transport, async move |cx| {
            // Handshake first, then create the session (`docs/ACP.md` §3.2).
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let response = cx
                .send_request(NewSessionRequest::new(cwd_for_client.clone()))
                .block_task()
                .await?;
            Ok(response.session_id)
        });

    let acp_session_id = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("session/new round-trip must not hang")
        .expect("session/new round-trip must succeed");

    server.abort();

    // The service saw a config whose cwd is the client's absolute cwd.
    let config = recorded
        .lock()
        .expect("lock not poisoned")
        .clone()
        .expect("create_session must have been called");
    assert_eq!(config.cwd, Some(cwd));

    // The ACP session id resolves back to the exact mag session id the fake
    // produced, proving the id mapping is faithful end to end.
    let mag_id = mag_acp::map::acp_session_id_to_mag(&acp_session_id)
        .expect("returned ACP session id must map back to a mag session id");
    assert_eq!(
        mag_id,
        SessionId::parse_str(SESSION_UUID).expect("valid uuid")
    );
}

/// Drives `initialize` → `session/new` → `session/load` from the real ACP
/// client over the in-memory pipe (`docs/ACP.md` §3.3), proving the advertised
/// `load_session` capability is backed by a working handler: the fake service
/// records a `resume_session` call for the exact mag `SessionId` the
/// `session/new` round-trip produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_load_round_trips_over_in_memory_pipe() {
    use agent_client_protocol::schema::v1::LoadSessionRequest;

    let recorded_resume: Arc<Mutex<Option<SessionId>>> = Arc::new(Mutex::new(None));
    let service: Arc<dyn MagService> = Arc::new(FakeService {
        recorded_resume: Arc::clone(&recorded_resume),
        ..FakeService::default()
    });

    let cwd = PathBuf::from("/abs/session/root");
    let (agent_transport, client_transport) = Channel::duplex();
    let server = tokio::spawn(async move { mag_acp::serve(service, agent_transport).await });

    let client = Client
        .builder()
        .connect_with(client_transport, async move |cx| {
            // Handshake: `load_session` must be advertised for a client to
            // call `session/load` at all (`docs/ACP.md` §3.3/§7).
            let initialize = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = cx
                .send_request(NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            cx.send_request(LoadSessionRequest::new(created.session_id, cwd))
                .block_task()
                .await?;
            Ok(initialize.agent_capabilities.load_session)
        });

    let load_session_advertised = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("session/load round-trip must not hang")
        .expect("session/load round-trip must succeed");

    server.abort();

    assert!(
        load_session_advertised,
        "load_session must be advertised for clients to call session/load",
    );
    assert_eq!(
        *recorded_resume.lock().expect("lock not poisoned"),
        Some(SessionId::parse_str(SESSION_UUID).expect("valid uuid")),
        "session/load must reach MagService::resume_session for the created session",
    );
}
