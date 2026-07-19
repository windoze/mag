//! Transport-neutral engine entry point implementing [`MagService`].

use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use agent_lib::client::LlmClient;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use mag_service::{
    InteractionResponseWire, MagService, RequestId, RunId, ServiceError, ServiceEvent,
    SessionConfig, SessionId, SessionInfo, SourceInfo, UserInput,
};
use mag_tools::ToolRegistry;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    EventBus,
    persistence::{Persistence, PersistenceError},
    session::SessionManager,
};

pub(crate) mod approval;

/// Transport-neutral service engine implementing [`MagService`].
///
/// The engine stores session configuration in memory, emits neutral
/// [`ServiceEvent`]s through an [`EventBus`] observed via
/// [`subscribe`](MagService::subscribe), and drives chat turns through agent-lib
/// on per-session driver actors (see the `session` module). It is the single
/// implementation of the [`MagService`] facade (`docs/DESIGN.md` §3.0/§3.1) and
/// can be injected as `Arc<dyn MagService>`.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    /// Creates an empty engine with an in-memory session store and event bus.
    ///
    /// Without an LLM client the engine can manage session metadata but cannot
    /// start runs; [`send_message`](MagService::send_message) reports a
    /// [`ServiceError::Backend`] until [`with_llm_client`](Engine::with_llm_client)
    /// is used instead.
    ///
    /// Persistence is backed by a private in-memory SQLite database, so session
    /// metadata and snapshots live only as long as the engine (`docs/DESIGN.md`
    /// §3.6). Use [`with_persistence`](Engine::with_persistence) for a durable
    /// store that survives a restart.
    #[must_use]
    pub fn new() -> Self {
        Self::assemble(None, Arc::new(ToolRegistry::new()), in_memory_store())
    }

    /// Creates an engine that drives chat turns through `client`, exposing the
    /// built-in minimal tool set (`docs/DESIGN.md` §3.2/§7).
    ///
    /// Persistence is in-memory; see [`with_persistence`](Engine::with_persistence)
    /// for a durable store.
    #[must_use]
    pub fn with_llm_client(client: Arc<dyn LlmClient>) -> Self {
        Self::with_llm_client_and_tools(client, ToolRegistry::with_builtins())
    }

    /// Creates an engine that drives chat turns through `client` with an explicit
    /// tool registry.
    ///
    /// This is the injection point used to supply a custom or test tool surface;
    /// [`with_llm_client`](Engine::with_llm_client) is the production default that
    /// installs the built-in tool set. Persistence is in-memory; see
    /// [`with_persistence`](Engine::with_persistence) for a durable store.
    #[must_use]
    pub fn with_llm_client_and_tools(client: Arc<dyn LlmClient>, tools: ToolRegistry) -> Self {
        Self::assemble(Some(client), Arc::new(tools), in_memory_store())
    }

    /// Creates an engine whose sessions and committed snapshots are persisted to a
    /// SQLite database at `path`, so a session survives a process restart
    /// (`docs/DESIGN.md` §3.6).
    ///
    /// On construction the engine seeds its session-id counter past every id
    /// already stored, so freshly created sessions never collide with persisted
    /// ones. A session persisted by an earlier process is made live again through
    /// [`resume_session`](MagService::resume_session), which reloads the latest
    /// snapshot and rebuilds the session's agent.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the database cannot be opened or its
    /// schema cannot be applied.
    pub fn with_persistence(
        client: Arc<dyn LlmClient>,
        tools: ToolRegistry,
        path: impl AsRef<Path>,
    ) -> Result<Self, PersistenceError> {
        let store = Arc::new(Persistence::open(path)?);
        Ok(Self::assemble(Some(client), Arc::new(tools), store))
    }

    /// Assembles an engine over an already-opened persistence store.
    fn assemble(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        store: Arc<Persistence>,
    ) -> Self {
        Self {
            inner: Arc::new(EngineInner::new(client, tools, store)),
        }
    }
}

#[async_trait]
impl MagService for Engine {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError> {
        let id = self.inner.session_ids.next_id();

        // Persist the session config before it becomes live so a restart can find
        // and resume it (`docs/DESIGN.md` §3.6).
        self.inner
            .store
            .save_session(id, &config)
            .map_err(persistence_backend)?;

        {
            let mut sessions = self.inner.sessions.lock().await;
            sessions.insert(id, config.clone());
        }

        self.inner.manager.create_session(id, config.clone());

        let _ = self
            .inner
            .event_bus
            .emit(mag_service::Event::SessionCreated { id, config });

        Ok(id)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        // The durable store is the source of truth for known sessions, so the
        // listing includes sessions persisted by an earlier process that have not
        // been resumed yet (`docs/DESIGN.md` §3.6).
        self.inner
            .store
            .list_sessions()
            .map_err(persistence_backend)
    }

    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
        // Load the persisted config and latest committed snapshot before making
        // the session live again (`docs/DESIGN.md` §3.6). An unknown session id
        // has no stored config, so it is reported as not found.
        let config = self
            .inner
            .store
            .load_session(id)
            .map_err(persistence_backend)?
            .ok_or(ServiceError::SessionNotFound { id })?;
        let snapshot = self
            .inner
            .store
            .load_snapshot(id)
            .map_err(persistence_backend)?;

        {
            let mut sessions = self.inner.sessions.lock().await;
            if sessions.contains_key(&id) {
                // Already live: resuming an active session is a no-op.
                return Ok(());
            }
            sessions.insert(id, config.clone());
        }

        // Rebuild the session's agent from the snapshot (re-injecting client,
        // tools, and the `IpcApproval` handler). A missing snapshot resumes the
        // session with empty history — the same shape a freshly created session
        // has before its first run.
        if let Err(error) = self.inner.manager.resume_session(id, config, snapshot) {
            self.inner.sessions.lock().await.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError> {
        {
            let mut sessions = self.inner.sessions.lock().await;
            if sessions.remove(&id).is_none() {
                return Err(ServiceError::SessionNotFound { id });
            }
        }
        self.inner.manager.delete_session(id);
        self.inner
            .store
            .delete_session(id)
            .map_err(persistence_backend)?;
        Ok(())
    }

    async fn send_message(&self, id: SessionId, input: UserInput) -> Result<RunId, ServiceError> {
        {
            let sessions = self.inner.sessions.lock().await;
            if !sessions.contains_key(&id) {
                return Err(ServiceError::SessionNotFound { id });
            }
        }

        self.inner.manager.send_message(id, input.text).await
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        {
            let sessions = self.inner.sessions.lock().await;
            if !sessions.contains_key(&id) {
                return Err(ServiceError::SessionNotFound { id });
            }
        }
        self.inner.manager.cancel(id);
        Ok(())
    }

    async fn respond_interaction(
        &self,
        id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        {
            let sessions = self.inner.sessions.lock().await;
            if !sessions.contains_key(&id) {
                return Err(ServiceError::SessionNotFound { id });
            }
        }
        self.inner
            .manager
            .respond_interaction(id, request_id, response)
            .await
    }

    fn subscribe(&self, id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let stream = self.inner.event_bus.subscribe().map(ServiceEvent::from);
        match id {
            None => stream.boxed(),
            Some(target) => stream
                .filter(move |event| {
                    let keep = event.session_id().is_none_or(|scope| scope == target);
                    futures::future::ready(keep)
                })
                .boxed(),
        }
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "list_sources".to_owned(),
        })
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "probe_local_agents".to_owned(),
        })
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

struct EngineInner {
    sessions: Mutex<BTreeMap<SessionId, SessionConfig>>,
    event_bus: EventBus,
    session_ids: SessionIdSource,
    store: Arc<Persistence>,
    manager: SessionManager,
}

impl EngineInner {
    fn new(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        store: Arc<Persistence>,
    ) -> Self {
        let event_bus = EventBus::new();
        // Seed the id counter past every persisted session so a restarted engine
        // never re-mints an id that already exists in the store (`docs/DESIGN.md`
        // §3.6).
        let session_ids = match store.max_session_id_value() {
            Ok(Some(max)) => {
                let next = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
                SessionIdSource::starting_at(next)
            }
            _ => SessionIdSource::new(),
        };
        let manager = SessionManager::new(client, tools, event_bus.clone(), store.clone());
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            event_bus,
            session_ids,
            store,
            manager,
        }
    }
}

/// Opens a private in-memory persistence store for the non-durable engine
/// constructors, panicking only if SQLite cannot open an in-memory database
/// (which does not happen in practice).
fn in_memory_store() -> Arc<Persistence> {
    Arc::new(Persistence::in_memory().expect("open in-memory persistence store"))
}

/// Maps a [`PersistenceError`] into a service-level [`ServiceError::Backend`].
fn persistence_backend(error: PersistenceError) -> ServiceError {
    ServiceError::Backend {
        message: error.to_string(),
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Engine").finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct SessionIdSource {
    counter: AtomicU64,
}

impl SessionIdSource {
    fn new() -> Self {
        Self {
            counter: AtomicU64::new(1),
        }
    }

    /// Creates a source whose first minted id encodes `next` (clamped to at least
    /// 1). Used to continue past a restored store's highest session id.
    fn starting_at(next: u64) -> Self {
        Self {
            counter: AtomicU64::new(next.max(1)),
        }
    }

    fn next_id(&self) -> SessionId {
        let value = self
            .counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .expect("session id counter exhausted");
        SessionId::new(Uuid::from_u128(u128::from(value)))
    }
}

#[cfg(test)]
mod skeleton {
    use futures::StreamExt;
    use futures::stream::BoxStream;
    use mag_service::{
        InteractionResponseWire, MagService, RequestId, RoutingMode, ServiceError, ServiceEvent,
        SessionConfig, SessionId,
    };
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use super::Engine;

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(1), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    #[tokio::test]
    async fn create_session_emits_session_created() {
        let engine = Engine::new();
        let mut events = engine.subscribe(None);
        let config = config("model-a");

        let id = engine
            .create_session(config.clone())
            .await
            .expect("create session");
        let event = next_event(&mut events).await;

        assert_eq!(
            event,
            ServiceEvent::SessionCreated {
                id,
                config: config.clone(),
            }
        );

        let sessions = engine.list_sessions().await.expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].config, config);
    }

    #[tokio::test]
    async fn list_sessions_reflects_created_sessions() {
        let engine = Engine::new();

        let first = engine
            .create_session(config("model-a"))
            .await
            .expect("create first session");
        let second = engine
            .create_session(config("model-b"))
            .await
            .expect("create second session");

        let listed = engine.list_sessions().await.expect("list sessions");

        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, first);
        assert_eq!(listed[0].config, config("model-a"));
        assert_eq!(listed[1].id, second);
        assert_eq!(listed[1].config, config("model-b"));
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_the_same_event() {
        let engine = Engine::new();
        let mut first_subscriber = engine.subscribe(None);
        let mut second_subscriber = engine.subscribe(None);
        let config = config("model-a");

        let id = engine
            .create_session(config.clone())
            .await
            .expect("create session");
        let expected = ServiceEvent::SessionCreated { id, config };

        assert_eq!(next_event(&mut first_subscriber).await, expected);
        assert_eq!(next_event(&mut second_subscriber).await, expected);
    }

    #[tokio::test]
    async fn unimplemented_methods_return_unsupported() {
        let engine = Engine::new();

        assert_eq!(
            engine.list_sources().await,
            Err(ServiceError::Unsupported {
                operation: "list_sources".to_owned(),
            })
        );
        assert_eq!(
            engine.probe_local_agents().await,
            Err(ServiceError::Unsupported {
                operation: "probe_local_agents".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn resume_unknown_session_reports_session_not_found() {
        // Nothing is persisted for a never-created id, so resuming it is a
        // not-found rather than the retired `Unsupported`.
        let engine = Engine::new();
        let missing = SessionId::new(Uuid::from_u128(7777));

        assert_eq!(
            engine.resume_session(missing).await,
            Err(ServiceError::SessionNotFound { id: missing })
        );
    }

    #[tokio::test]
    async fn resume_active_session_is_a_noop() {
        // A clientless engine still tracks the session in memory, so resuming an
        // already-live session succeeds without spawning a duplicate actor.
        let engine = Engine::new();
        let id = engine
            .create_session(config("model-a"))
            .await
            .expect("create session");

        engine
            .resume_session(id)
            .await
            .expect("resume active session");
    }

    #[tokio::test]
    async fn respond_interaction_without_pending_reports_interaction_not_found() {
        // A clientless engine spawns no session actor, so there is never a
        // pending interaction to resolve; `respond_interaction` reports the
        // request id as unknown rather than the retired `Unsupported`.
        let engine = Engine::new();
        let id = engine
            .create_session(config("model-a"))
            .await
            .expect("create session");
        let request_id = RequestId::new(Uuid::from_u128(9));

        assert_eq!(
            engine
                .respond_interaction(
                    id,
                    request_id,
                    InteractionResponseWire::Answer {
                        text: "ok".to_owned(),
                    },
                )
                .await,
            Err(ServiceError::InteractionNotFound { request_id })
        );
    }

    #[tokio::test]
    async fn cancel_and_delete_unknown_session_report_session_not_found() {
        let engine = Engine::new();
        let missing = SessionId::new(Uuid::from_u128(4242));

        assert_eq!(
            engine.cancel(missing).await,
            Err(ServiceError::SessionNotFound { id: missing })
        );
        assert_eq!(
            engine.delete_session(missing).await,
            Err(ServiceError::SessionNotFound { id: missing })
        );
    }

    #[tokio::test]
    async fn cancel_and_delete_known_session_succeed() {
        let engine = Engine::new();
        let id = engine
            .create_session(config("model-a"))
            .await
            .expect("create session");

        // Without a run in flight, cancel is an accepted no-op.
        engine.cancel(id).await.expect("cancel known session");

        engine.delete_session(id).await.expect("delete session");
        assert!(
            engine
                .list_sessions()
                .await
                .expect("list sessions")
                .is_empty()
        );

        // Deleting again reports the session as gone.
        assert_eq!(
            engine.delete_session(id).await,
            Err(ServiceError::SessionNotFound { id })
        );
    }

    #[tokio::test]
    async fn send_message_to_unknown_session_reports_session_not_found() {
        let engine = Engine::new();
        let missing = SessionId::new(Uuid::from_u128(999));

        let error = engine
            .send_message(missing, mag_service::UserInput::text("hi"))
            .await
            .expect_err("unknown session must fail");
        assert_eq!(error, ServiceError::SessionNotFound { id: missing });
    }
}

#[cfg(test)]
mod chat {
    use std::sync::Arc;

    use agent_lib::{
        client::LlmClient,
        model::{
            content::ContentBlock,
            message::{Message, Role},
            usage::Usage,
        },
    };
    use futures::StreamExt;
    use futures::stream::BoxStream;
    use mag_service::{
        MagService, RoutingMode, ServiceEvent, SessionConfig, SessionId, UsageInfo, UserInput,
    };
    use tokio::time::{Duration, timeout};

    use crate::test_support::{FakeLlmClient, text_stream_with_usage};

    use super::Engine;

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input,
            output,
            total: Some(input + output),
            ..Usage::default()
        }
    }

    fn engine_with_fake(fake: Arc<FakeLlmClient>) -> Engine {
        let client: Arc<dyn LlmClient> = fake;
        Engine::with_llm_client(client)
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(1), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    async fn create_session(engine: &Engine) -> SessionId {
        engine
            .create_session(config("fake-chat"))
            .await
            .expect("create session")
    }

    fn text(message: &Message) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn send_message_streams_ordered_run_events() {
        let fake =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&["hel", "lo"], usage(7, 2))]);
        let engine = engine_with_fake(fake.clone());
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        let run_id = engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        assert_ne!(run_id.into_uuid(), uuid::Uuid::nil());

        let ServiceEvent::RunStarted {
            id,
            run_id: started,
        } = next_event(&mut events).await
        else {
            panic!("expected run_started");
        };
        assert_eq!(id, session);
        assert_eq!(started, run_id);

        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "hel".to_owned(),
            }
        );
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "lo".to_owned(),
            }
        );

        let ServiceEvent::RunFinished { id, output } = next_event(&mut events).await else {
            panic!("expected run_finished");
        };
        assert_eq!(id, session);
        assert_eq!(output.text, "hello");
        assert_eq!(
            output.usage,
            Some(UsageInfo {
                input_tokens: 7,
                output_tokens: 2,
                total_tokens: 9,
            })
        );

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "fake-chat");
        assert!(requests[0].stream);
        assert_eq!(requests[0].messages.len(), 1);
        assert_eq!(requests[0].messages[0].role, Role::User);
        assert_eq!(text(&requests[0].messages[0]), "hi");
    }

    #[tokio::test]
    async fn arc_dyn_service_streams_ordered_run_events() {
        let fake = FakeLlmClient::scripted(vec![text_stream_with_usage(&["hi", "!"], usage(4, 1))]);
        let service: Arc<dyn MagService> = Arc::new(engine_with_fake(fake));

        let session = service
            .create_session(config("fake-chat"))
            .await
            .expect("create session");
        let mut events = service.subscribe(Some(session));

        let run_id = service
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");

        let ServiceEvent::RunStarted {
            id,
            run_id: started,
        } = next_event(&mut events).await
        else {
            panic!("expected run_started");
        };
        assert_eq!(id, session);
        assert_eq!(started, run_id);

        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "hi".to_owned(),
            }
        );
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "!".to_owned(),
            }
        );

        let ServiceEvent::RunFinished { id, output } = next_event(&mut events).await else {
            panic!("expected run_finished");
        };
        assert_eq!(id, session);
        assert_eq!(output.text, "hi!");
    }

    #[tokio::test]
    async fn exhausted_budget_surfaces_structured_run_error() {
        // A session budget of 1 token is blown by the first scripted response
        // (usage 9): agent-lib refuses the charge and the run ends with
        // `FacadeError::BudgetExhausted`, which mag must classify structurally.
        let fake = FakeLlmClient::scripted(vec![text_stream_with_usage(&["hi"], usage(7, 2))]);
        let engine = engine_with_fake(fake);
        let mut budgeted = config("fake-chat");
        budgeted.budget = Some(mag_service::SessionBudget {
            max_tokens: Some(1),
            ..mag_service::SessionBudget::default()
        });
        let session = engine
            .create_session(budgeted)
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");

        let terminal = loop {
            match next_event(&mut events).await {
                ServiceEvent::RunFinished { .. } => {
                    panic!("a budget-exhausted run must not finish successfully")
                }
                error @ ServiceEvent::RunError { .. } => break error,
                _ => continue,
            }
        };
        assert!(
            matches!(
                terminal,
                ServiceEvent::RunError {
                    kind: mag_service::RunErrorKind::BudgetExhausted,
                    ..
                }
            ),
            "expected a BudgetExhausted run error, got {terminal:?}",
        );
    }

    #[tokio::test]
    async fn exhausted_loop_limit_surfaces_structured_run_error() {
        // Ten scripted tool-use rounds blow the facade's per-turn loop guard
        // (max_steps 8 / max_tool_rounds 4): the run ends with
        // `FacadeError::LoopLimitExceeded`, classified structurally so the ACP
        // bridge can report `MaxTurnRequests`.
        let scripts = (0..10)
            .map(|i| {
                crate::test_support::tool_use_stream(
                    "read_file",
                    &format!("call-{i}"),
                    serde_json::json!({ "path": "README.md" }),
                )
            })
            .collect();
        let fake = FakeLlmClient::scripted(scripts);
        let client: Arc<dyn LlmClient> = fake;
        let engine =
            Engine::with_llm_client_and_tools(client, mag_tools::ToolRegistry::with_builtins());
        let session = engine
            .create_session(config("fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("loop"))
            .await
            .expect("send message");

        let terminal = loop {
            match next_event(&mut events).await {
                ServiceEvent::RunFinished { .. } => {
                    panic!("a loop-limited run must not finish successfully")
                }
                error @ ServiceEvent::RunError { .. } => break error,
                _ => continue,
            }
        };
        assert!(
            matches!(
                terminal,
                ServiceEvent::RunError {
                    kind: mag_service::RunErrorKind::LoopLimitExceeded,
                    ..
                }
            ),
            "expected a LoopLimitExceeded run error, got {terminal:?}",
        );
    }

    #[tokio::test]
    async fn subscribe_filters_events_by_session() {
        let fake = FakeLlmClient::scripted(vec![
            text_stream_with_usage(&["ignored"], usage(1, 1)),
            text_stream_with_usage(&["ok"], usage(1, 1)),
        ]);
        let engine = engine_with_fake(fake);

        let observed = create_session(&engine).await;
        let other = create_session(&engine).await;
        let mut events = engine.subscribe(Some(observed));

        // A run on `other` must not leak into a subscription filtered to
        // `observed`.
        engine
            .send_message(other, UserInput::text("ignore"))
            .await
            .expect("send to other session");
        engine
            .send_message(observed, UserInput::text("hi"))
            .await
            .expect("send to observed session");

        let ServiceEvent::RunStarted { id, .. } = next_event(&mut events).await else {
            panic!("expected run_started for observed session");
        };
        assert_eq!(id, observed);
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta { id, .. } if id == observed
        ));
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunFinished { id, .. } if id == observed
        ));
    }

    #[tokio::test]
    async fn send_message_accumulates_history_in_one_session() {
        let fake = FakeLlmClient::scripted(vec![
            text_stream_with_usage(&["first"], usage(3, 1)),
            text_stream_with_usage(&["second"], usage(5, 2)),
        ]);
        let engine = engine_with_fake(fake.clone());
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("first send");
        for _ in 0..3 {
            let _ = next_event(&mut events).await;
        }

        engine
            .send_message(session, UserInput::text("again"))
            .await
            .expect("second send");
        for _ in 0..3 {
            let _ = next_event(&mut events).await;
        }

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 2);
        let second_messages = &requests[1].messages;
        assert!(
            second_messages
                .iter()
                .any(|message| message.role == Role::User && text(message) == "hi"),
            "second request should include first user message: {second_messages:?}",
        );
        assert!(
            second_messages
                .iter()
                .any(|message| message.role == Role::Assistant && text(message) == "first"),
            "second request should include first assistant reply: {second_messages:?}",
        );
        let last = second_messages.last().expect("second request has messages");
        assert_eq!(last.role, Role::User);
        assert_eq!(text(last), "again");
    }
}

#[cfg(test)]
mod session {
    use std::sync::Arc;

    use agent_lib::{client::LlmClient, model::usage::Usage};
    use futures::stream::BoxStream;
    use mag_service::{MagService, RoutingMode, ServiceEvent, SessionConfig, SessionId, UserInput};
    use tokio::time::{Duration, timeout};

    use crate::test_support::{
        FakeLlmClient, StreamScript, stalling_text_stream, text_stream_with_usage,
    };

    use super::Engine;

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input,
            output,
            total: Some(input + output),
            ..Usage::default()
        }
    }

    fn complete(chunks: &[&str], usage: Usage) -> StreamScript {
        StreamScript::Complete(text_stream_with_usage(chunks, usage))
    }

    fn engine_with_fake(fake: Arc<FakeLlmClient>) -> Engine {
        let client: Arc<dyn LlmClient> = fake;
        Engine::with_llm_client(client)
    }

    async fn create_session(engine: &Engine, model: &str) -> SessionId {
        engine
            .create_session(config(model))
            .await
            .expect("create session")
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(2), futures::StreamExt::next(events))
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    /// Reads events until the run reaches a terminal `RunFinished`/`RunError`.
    async fn collect_run(events: &mut BoxStream<'static, ServiceEvent>) -> Vec<ServiceEvent> {
        let mut collected = Vec::new();
        loop {
            let event = next_event(events).await;
            let terminal = matches!(
                event,
                ServiceEvent::RunFinished { .. } | ServiceEvent::RunError { .. }
            );
            collected.push(event);
            if terminal {
                break;
            }
        }
        collected
    }

    /// Asserts a clean `RunStarted → TextDelta+ → RunFinished` lifecycle whose
    /// every event is scoped to `session`.
    fn assert_run_ok(events: &[ServiceEvent], session: SessionId, run_id: mag_service::RunId) {
        for event in events {
            assert_eq!(
                event.session_id(),
                Some(session),
                "event leaked across sessions: {event:?}",
            );
        }

        let ServiceEvent::RunStarted {
            id,
            run_id: started,
        } = &events[0]
        else {
            panic!("expected run_started, got {:?}", events[0]);
        };
        assert_eq!(*id, session);
        assert_eq!(*started, run_id);

        assert!(
            events
                .iter()
                .any(|event| matches!(event, ServiceEvent::TextDelta { .. })),
            "run produced no text delta: {events:?}",
        );

        let last = events.last().expect("run has at least one event");
        assert!(
            matches!(last, ServiceEvent::RunFinished { id, .. } if *id == session),
            "expected run_finished, got {last:?}",
        );
    }

    #[tokio::test]
    async fn two_sessions_route_events_by_session_id() {
        let fake = FakeLlmClient::scripted(vec![
            text_stream_with_usage(&["ok"], usage(1, 1)),
            text_stream_with_usage(&["ok"], usage(1, 1)),
        ]);
        let engine = engine_with_fake(fake);

        let a = create_session(&engine, "fake-a").await;
        let b = create_session(&engine, "fake-b").await;
        let mut events_a = engine.subscribe(Some(a));
        let mut events_b = engine.subscribe(Some(b));

        let run_a = engine
            .send_message(a, UserInput::text("hi"))
            .await
            .expect("send to session a");
        let run_b = engine
            .send_message(b, UserInput::text("hi"))
            .await
            .expect("send to session b");

        // Each subscriber only ever observes its own session's run, proving the
        // two per-session actors never cross-talk on the shared event bus.
        let a_events = collect_run(&mut events_a).await;
        let b_events = collect_run(&mut events_b).await;

        assert_run_ok(&a_events, a, run_a);
        assert_run_ok(&b_events, b, run_b);
    }

    #[tokio::test]
    async fn cancel_mid_run_terminates_and_session_stays_usable() {
        let fake = FakeLlmClient::scripted_streams(vec![
            // Session S's first run stalls after one delta so a cancel can land.
            stalling_text_stream(&["wait"]),
            // A concurrent run on session O completes while S is stalled.
            complete(&["other"], usage(1, 1)),
            // Session S's post-cancel run completes, proving reuse.
            complete(&["resumed"], usage(2, 1)),
        ]);
        let engine = engine_with_fake(fake);

        let s = create_session(&engine, "fake-s").await;
        let o = create_session(&engine, "fake-o").await;
        let mut events_s = engine.subscribe(Some(s));
        let mut events_o = engine.subscribe(Some(o));

        // Start the long run and wait until it is actually streaming.
        let run_s = engine
            .send_message(s, UserInput::text("go"))
            .await
            .expect("send to session s");
        assert!(matches!(
            next_event(&mut events_s).await,
            ServiceEvent::RunStarted { id, run_id } if id == s && run_id == run_s
        ));
        assert_eq!(
            next_event(&mut events_s).await,
            ServiceEvent::TextDelta {
                id: s,
                text: "wait".to_owned(),
            }
        );

        // Another session runs to completion while S is stalled: an in-flight run
        // in one session does not affect another.
        engine
            .send_message(o, UserInput::text("hi"))
            .await
            .expect("send to session o");
        let o_events = collect_run(&mut events_o).await;
        assert!(matches!(
            o_events.last().expect("session o produced events"),
            ServiceEvent::RunFinished { id, .. } if *id == o
        ));

        // Cancel the stalled run: it terminates cleanly with a cancellation error
        // and never emits a RunFinished.
        engine.cancel(s).await.expect("cancel session s");
        let cancelled = next_event(&mut events_s).await;
        assert!(
            matches!(
                &cancelled,
                ServiceEvent::RunError { id, message, kind } if *id == s && message == "run cancelled" && *kind == mag_service::RunErrorKind::Cancelled
            ),
            "expected cancellation error, got {cancelled:?}",
        );

        // The session is still usable: a fresh message runs to completion.
        let run_s2 = engine
            .send_message(s, UserInput::text("again"))
            .await
            .expect("resend to session s");
        let resumed = collect_run(&mut events_s).await;
        assert_run_ok(&resumed, s, run_s2);
        assert!(
            resumed.iter().any(|event| matches!(
                event,
                ServiceEvent::TextDelta { text, .. } if text == "resumed"
            )),
            "resumed run should stream its scripted follow-up text: {resumed:?}",
        );
    }
}

#[cfg(test)]
mod tool_turn {
    //! End-to-end, offline tool + approval turns driven through the public
    //! [`MagService`] surface (`docs/DESIGN.md` §3.2/§3.3, C3-3).
    //!
    //! A scripted [`FakeLlmClient`] returns a tool call, and stub plugins provide
    //! a gated `shell` and an auto-allowed `read_file`, so these tests exercise
    //! the full pause/resume path (`InteractionRequested` -> `respond_interaction`
    //! -> `ToolStarted` / `ToolFinished`) without a network or real filesystem.

    use std::sync::Arc;

    use agent_lib::{
        client::LlmClient,
        facade::{ToolContext, ToolResult},
        model::{tool::Tool, usage::Usage},
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use mag_service::{
        ApprovalDecisionWire, InteractionKindWire, InteractionResponseWire, MagService,
        RoutingMode, ServiceEvent, SessionConfig, SessionId, StepIdWire, ToolCallIdWire, UserInput,
    };
    use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
    use serde_json::{Value, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::test_support::{FakeLlmClient, text_stream_with_usage, tool_use_stream};

    use super::Engine;

    /// A canned tool plugin: it ignores its arguments and returns fixed text, so
    /// a turn's tool events are deterministic and offline.
    #[derive(Debug)]
    struct StubTool {
        name: &'static str,
        output: &'static str,
        permission: Option<PermissionSpec>,
    }

    #[async_trait]
    impl ToolPlugin for StubTool {
        fn name(&self) -> &str {
            self.name
        }

        fn declaration(&self) -> Tool {
            Tool {
                name: self.name.to_owned(),
                description: format!("stub {} tool", self.name),
                input_schema: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
            ToolResult::text(self.output)
        }

        fn permission(&self) -> Option<PermissionSpec> {
            self.permission
        }
    }

    /// A registry with a gated `shell` and an auto-allowed `read_file`.
    fn registry() -> ToolRegistry {
        ToolRegistry::new()
            .register(Arc::new(StubTool {
                name: "shell",
                output: "shell output",
                permission: Some(PermissionSpec::new(ToolCategory::Shell, ToolRisk::Medium)),
            }))
            .register(Arc::new(StubTool {
                name: "read_file",
                output: "file contents",
                permission: None,
            }))
    }

    fn config() -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: "fake-tool".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input,
            output,
            total: Some(input + output),
            ..Usage::default()
        }
    }

    fn engine(fake: Arc<FakeLlmClient>) -> Engine {
        let client: Arc<dyn LlmClient> = fake;
        Engine::with_llm_client_and_tools(client, registry())
    }

    /// The interface only supplies the decision; `step_id`/`call_id` are
    /// reconstructed from the stored interaction, so nil placeholders are fine.
    fn approval(decision: ApprovalDecisionWire) -> InteractionResponseWire {
        InteractionResponseWire::Approval {
            step_id: StepIdWire::new(Uuid::nil()),
            call_id: ToolCallIdWire::new(Uuid::nil()),
            decision,
            message: None,
        }
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(2), futures::StreamExt::next(events))
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    /// Reads events until (and including) the run's terminal event.
    async fn collect_until_terminal(
        events: &mut BoxStream<'static, ServiceEvent>,
    ) -> Vec<ServiceEvent> {
        let mut collected = Vec::new();
        loop {
            let event = next_event(events).await;
            let terminal = matches!(
                event,
                ServiceEvent::RunFinished { .. } | ServiceEvent::RunError { .. }
            );
            collected.push(event);
            if terminal {
                return collected;
            }
        }
    }

    async fn create_session(engine: &Engine) -> SessionId {
        engine
            .create_session(config())
            .await
            .expect("create session")
    }

    #[tokio::test]
    async fn gated_tool_pauses_then_runs_after_approve() {
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("shell", "call-1", json!({ "command": "echo hi" })),
            text_stream_with_usage(&["done"], usage(3, 1)),
        ]);
        let engine = engine(fake);
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        let run_id = engine
            .send_message(session, UserInput::text("run shell"))
            .await
            .expect("send message");

        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, run_id: started } if id == session && started == run_id
        ));

        // The gated shell tool pauses the run: `IpcApproval` emits an approval
        // `InteractionRequested`, and no tool event has fired yet.
        let request_id = match next_event(&mut events).await {
            ServiceEvent::InteractionRequested {
                id,
                request_id,
                kind,
            } => {
                assert_eq!(id, session);
                assert!(
                    matches!(kind, InteractionKindWire::Approval { .. }),
                    "expected an approval interaction, got {kind:?}",
                );
                request_id
            }
            other => panic!("expected interaction_requested, got {other:?}"),
        };

        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve");

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { id, trace } if *id == session && trace.name == "shell"
            )),
            "an approved gated tool must emit ToolStarted: {rest:?}",
        );
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolFinished { id, trace } if *id == session && trace.name == "shell"
            )),
            "an approved gated tool must emit ToolFinished: {rest:?}",
        );
        assert!(
            matches!(
                rest.last().expect("terminal event"),
                ServiceEvent::RunFinished { id, output } if *id == session && output.text == "done"
            ),
            "the approved turn must finish with the model's follow-up text: {rest:?}",
        );
    }

    #[tokio::test]
    async fn gated_tool_denied_skips_tool_but_run_finishes() {
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("shell", "call-1", json!({ "command": "rm -rf /" })),
            text_stream_with_usage(&["understood"], usage(2, 1)),
        ]);
        let engine = engine(fake);
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("run shell"))
            .await
            .expect("send message");

        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { .. }
        ));
        let request_id = match next_event(&mut events).await {
            ServiceEvent::InteractionRequested { request_id, .. } => request_id,
            other => panic!("expected interaction_requested, got {other:?}"),
        };

        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Deny))
            .await
            .expect("deny");

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            !rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { .. } | ServiceEvent::ToolFinished { .. }
            )),
            "a denied tool must never execute (no tool events): {rest:?}",
        );
        assert!(
            matches!(
                rest.last().expect("terminal event"),
                ServiceEvent::RunFinished { id, .. } if *id == session
            ),
            "a denied tool is fed back to the model and the run still finishes: {rest:?}",
        );
    }

    #[tokio::test]
    async fn auto_allowed_tool_runs_without_interaction() {
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("read_file", "call-1", json!({ "path": "README.md" })),
            text_stream_with_usage(&["summary"], usage(4, 2)),
        ]);
        let engine = engine(fake);
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("read it"))
            .await
            .expect("send message");

        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { .. }
        ));

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            !rest
                .iter()
                .any(|event| matches!(event, ServiceEvent::InteractionRequested { .. })),
            "an auto-allowed tool must not pause for approval: {rest:?}",
        );
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { trace, .. } if trace.name == "read_file"
            )),
            "the auto-allowed tool must emit ToolStarted: {rest:?}",
        );
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolFinished { trace, .. } if trace.name == "read_file"
            )),
            "the auto-allowed tool must emit ToolFinished: {rest:?}",
        );
        assert!(
            matches!(
                rest.last().expect("terminal event"),
                ServiceEvent::RunFinished { id, .. } if *id == session
            ),
            "the auto-allowed turn must finish: {rest:?}",
        );
    }
}

#[cfg(test)]
mod persist {
    //! Cross-"restart" persistence and restore tests driven through the public
    //! [`MagService`] surface (`docs/DESIGN.md` §3.6, C4-1).
    //!
    //! Each test opens a temporary file-backed SQLite database, drives a session
    //! on one engine, drops that engine (the "restart"), then opens a second
    //! engine over the same database and resumes the session. Everything runs
    //! offline through a scripted [`FakeLlmClient`]; no network, real credentials,
    //! or real filesystem tools are involved.

    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use agent_lib::{
        client::LlmClient,
        facade::{ToolContext, ToolResult},
        model::{tool::Tool, usage::Usage},
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use mag_service::{
        ApprovalDecisionWire, InteractionKindWire, InteractionResponseWire, MagService,
        RoutingMode, ServiceEvent, SessionConfig, SessionId, StepIdWire, ToolCallIdWire, UserInput,
    };
    use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
    use serde_json::{Value, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::test_support::{FakeLlmClient, text_stream_with_usage, tool_use_stream};

    use super::Engine;

    /// A unique temporary database path that deletes its files on drop.
    struct TempDb {
        path: PathBuf,
    }

    impl TempDb {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "mag-persist-{}-{nanos}-{unique}.sqlite",
                std::process::id()
            ));
            Self { path }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_extension("sqlite-wal"));
            let _ = std::fs::remove_file(self.path.with_extension("sqlite-shm"));
        }
    }

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input,
            output,
            total: Some(input + output),
            ..Usage::default()
        }
    }

    /// The interface only supplies the decision; `step_id`/`call_id` are
    /// reconstructed from the stored interaction, so nil placeholders are fine.
    fn approval(decision: ApprovalDecisionWire) -> InteractionResponseWire {
        InteractionResponseWire::Approval {
            step_id: StepIdWire::new(Uuid::nil()),
            call_id: ToolCallIdWire::new(Uuid::nil()),
            decision,
            message: None,
        }
    }

    /// A canned gated `shell` tool: it pauses for approval and returns fixed text.
    #[derive(Debug)]
    struct GatedShell;

    #[async_trait]
    impl ToolPlugin for GatedShell {
        fn name(&self) -> &str {
            "shell"
        }

        fn declaration(&self) -> Tool {
            Tool {
                name: "shell".to_owned(),
                description: "stub shell tool".to_owned(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
            ToolResult::text("shell output")
        }

        fn permission(&self) -> Option<PermissionSpec> {
            Some(PermissionSpec::new(ToolCategory::Shell, ToolRisk::Medium))
        }
    }

    fn gated_registry() -> ToolRegistry {
        ToolRegistry::new().register(Arc::new(GatedShell))
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(2), futures::StreamExt::next(events))
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    /// Reads events until (and including) the run's terminal event.
    async fn drain_run(events: &mut BoxStream<'static, ServiceEvent>) -> Vec<ServiceEvent> {
        let mut collected = Vec::new();
        loop {
            let event = next_event(events).await;
            let terminal = matches!(
                event,
                ServiceEvent::RunFinished { .. } | ServiceEvent::RunError { .. }
            );
            collected.push(event);
            if terminal {
                return collected;
            }
        }
    }

    /// Sends `message` and drains the resulting run, asserting it finished.
    async fn run_message(
        engine: &Engine,
        session: SessionId,
        events: &mut BoxStream<'static, ServiceEvent>,
        message: &str,
    ) {
        engine
            .send_message(session, UserInput::text(message))
            .await
            .expect("send message");
        let run = drain_run(events).await;
        assert!(
            matches!(
                run.last().expect("terminal event"),
                ServiceEvent::RunFinished { id, .. } if *id == session
            ),
            "turn `{message}` should finish cleanly: {run:?}",
        );
    }

    #[tokio::test]
    async fn committed_run_persists_a_snapshot_to_the_store() {
        let db = TempDb::new();
        let client: Arc<dyn LlmClient> =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&["hi"], usage(2, 1))]);
        let engine = Engine::with_persistence(client, ToolRegistry::new(), &db.path)
            .expect("open persistent engine");

        let session = engine
            .create_session(config("fake-persist"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        // Before any run there is no snapshot; a committed run writes one.
        assert!(
            engine
                .inner
                .store
                .load_snapshot(session)
                .expect("load snapshot")
                .is_none(),
            "a never-run session must have no snapshot",
        );

        run_message(&engine, session, &mut events, "hello").await;

        assert!(
            engine
                .inner
                .store
                .load_snapshot(session)
                .expect("load snapshot")
                .is_some(),
            "a committed run must persist a snapshot",
        );
    }

    #[tokio::test]
    async fn resume_after_restart_continues_conversation_with_prior_context() {
        let db = TempDb::new();

        // First process: two committed turns accumulate history and snapshots.
        let client1: Arc<dyn LlmClient> = FakeLlmClient::scripted(vec![
            text_stream_with_usage(&["first"], usage(3, 1)),
            text_stream_with_usage(&["second"], usage(5, 2)),
        ]);
        let engine1 = Engine::with_persistence(client1, ToolRegistry::new(), &db.path)
            .expect("open first engine");
        let session = engine1
            .create_session(config("fake-resume"))
            .await
            .expect("create session");
        let mut events1 = engine1.subscribe(Some(session));
        run_message(&engine1, session, &mut events1, "hi").await;
        run_message(&engine1, session, &mut events1, "again").await;

        // "Restart": drop the first engine (joins its session threads); the latest
        // committed snapshot is already durable.
        drop(events1);
        drop(engine1);

        // Second process: a fresh engine over the same database resumes the
        // session and runs a third turn.
        let client2 =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&["third"], usage(7, 1))]);
        let client2_dyn: Arc<dyn LlmClient> = client2.clone();
        let engine2 = Engine::with_persistence(client2_dyn, ToolRegistry::new(), &db.path)
            .expect("open second engine");

        // The session is visible from the durable store before it is resumed.
        let listed = engine2.list_sessions().await.expect("list sessions");
        assert!(
            listed.iter().any(|info| info.id == session),
            "the persisted session must be listed after restart: {listed:?}",
        );

        engine2
            .resume_session(session)
            .await
            .expect("resume session");
        let mut events2 = engine2.subscribe(Some(session));
        run_message(&engine2, session, &mut events2, "third").await;

        // The third turn's request carries the first two turns as history, proving
        // the restored agent continued the snapshotted conversation.
        let requests = client2.stream_requests();
        assert_eq!(
            requests.len(),
            1,
            "the resumed engine drove exactly one turn"
        );
        let flat = format!("{:?}", requests[0].messages);
        for fragment in ["hi", "first", "again", "second", "third"] {
            assert!(
                flat.contains(fragment),
                "restored context is missing `{fragment}`: {flat}",
            );
        }

        // A freshly created session on the restarted engine gets a new id past the
        // resumed one, so ids never collide across a restart.
        let fresh = engine2
            .create_session(config("fake-fresh"))
            .await
            .expect("create fresh session");
        assert_ne!(
            fresh, session,
            "a new session must not reuse a persisted id"
        );
        assert!(
            fresh.into_uuid().as_u128() > session.into_uuid().as_u128(),
            "a new session id must be seeded past the persisted maximum",
        );
    }

    #[tokio::test]
    async fn snapshot_written_by_a_run_contains_no_credentials() {
        // The snapshot the actor persists after a committed run must stay
        // data-only: no credential ever reaches the store (`docs/DESIGN.md` §9.5).
        let db = TempDb::new();
        let client: Arc<dyn LlmClient> =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(2, 1))]);
        let engine = Engine::with_persistence(client, ToolRegistry::new(), &db.path)
            .expect("open persistent engine");
        let session = engine
            .create_session(config("fake-clean"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        run_message(&engine, session, &mut events, "hello").await;

        let snapshot = engine
            .inner
            .store
            .load_snapshot(session)
            .expect("load snapshot")
            .expect("snapshot written");
        let json = serde_json::to_string(&snapshot)
            .expect("serialize snapshot")
            .to_lowercase();
        for forbidden in ["api_key", "apikey", "secret", "credential", "password"] {
            assert!(
                !json.contains(forbidden),
                "persisted snapshot contains a credential-like key `{forbidden}`",
            );
        }
    }

    #[tokio::test]
    async fn resumed_approval_session_still_pauses_through_ipc_approval() {
        let db = TempDb::new();

        // First process: one committed plain turn on an agent that also carries a
        // gated `shell` tool, so the snapshot preserves the tool declaration.
        let client1: Arc<dyn LlmClient> =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&["ready"], usage(2, 1))]);
        let engine1 = Engine::with_persistence(client1, gated_registry(), &db.path)
            .expect("open first engine");
        let session = engine1
            .create_session(config("fake-approve"))
            .await
            .expect("create session");
        let mut events1 = engine1.subscribe(Some(session));
        run_message(&engine1, session, &mut events1, "warm up").await;
        drop(events1);
        drop(engine1);

        // Second process: resume, then trigger the gated tool. The restored agent
        // must re-inject `IpcApproval`, so the tool call pauses cross-process
        // (an `InteractionRequested` that only `respond_interaction` can release)
        // rather than falling back to synchronous `FacadeApproval`.
        let client2: Arc<dyn LlmClient> = FakeLlmClient::scripted(vec![
            tool_use_stream("shell", "call-1", json!({ "command": "echo hi" })),
            text_stream_with_usage(&["done"], usage(3, 1)),
        ]);
        let engine2 = Engine::with_persistence(client2, gated_registry(), &db.path)
            .expect("open second engine");
        engine2
            .resume_session(session)
            .await
            .expect("resume session");
        let mut events2 = engine2.subscribe(Some(session));

        engine2
            .send_message(session, UserInput::text("run shell"))
            .await
            .expect("send message");

        assert!(matches!(
            next_event(&mut events2).await,
            ServiceEvent::RunStarted { id, .. } if id == session
        ));

        let request_id = match next_event(&mut events2).await {
            ServiceEvent::InteractionRequested {
                id,
                request_id,
                kind,
            } => {
                assert_eq!(id, session);
                assert!(
                    matches!(kind, InteractionKindWire::Approval { .. }),
                    "a restored gated tool must pause with an approval interaction, got {kind:?}",
                );
                request_id
            }
            other => panic!(
                "restored session must pause through IpcApproval, got {other:?} \
                 (a synchronous FacadeApproval fallback would emit no InteractionRequested)"
            ),
        };

        engine2
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve");

        let rest = drain_run(&mut events2).await;
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { id, trace } if *id == session && trace.name == "shell"
            )),
            "an approved restored tool must execute: {rest:?}",
        );
        assert!(
            matches!(
                rest.last().expect("terminal event"),
                ServiceEvent::RunFinished { id, output } if *id == session && output.text == "done"
            ),
            "the resumed approved turn must finish with the follow-up text: {rest:?}",
        );
    }
}
