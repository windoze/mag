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
use mag_config::{ConfigError, ConfigSnapshot, ResolvedExternalAgent, ResolvedProvider};
use mag_service::{
    InteractionResponseWire, MagService, RequestId, RunId, ServiceError, ServiceEvent,
    SessionConfig, SessionId, SessionInfo, SourceInfo, SourceKindWire, UserInput,
};
use mag_sources::SourceRegistry;
use mag_tools::ToolRegistry;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    EventBus,
    config::ConfigService,
    persistence::{Persistence, PersistenceError},
    session::SessionManager,
    turn_complete::{TurnCompleteHub, TurnCompleteListener},
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
        Self::assemble(
            None,
            Arc::new(ToolRegistry::new()),
            in_memory_store(),
            None,
            SourceRegistry::new(),
        )
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
        Self::assemble(
            Some(client),
            Arc::new(tools),
            in_memory_store(),
            None,
            SourceRegistry::new(),
        )
    }

    /// Creates an engine backed by a runtime [`ConfigService`]
    /// (`docs/CLI.md` §4.3–§4.5).
    ///
    /// This is the configuration injection point: with a config service
    /// present, the engine implements the full `MagService` configuration
    /// surface — [`get_config`](MagService::get_config) /
    /// [`update_config`](MagService::update_config) /
    /// [`reload_config`](MagService::reload_config) proxy the service
    /// (emitting [`ServiceEvent::ConfigChanged`] on success), and
    /// [`apply_config`](MagService::apply_config) rolls the current snapshot
    /// onto live sessions at each session's next turn boundary through the
    /// turn-complete mechanism (`docs/CLI.md` §4.4/§4.5); sessions without an
    /// in-progress run apply immediately. Engines built by the other
    /// constructors keep reporting those methods as
    /// [`ServiceError::Unsupported`].
    ///
    /// Persistence is in-memory; see [`with_persistence`](Engine::with_persistence)
    /// for a durable store. [`Engine::from_config`] assembles the client, tools,
    /// sources, and persistence from the snapshot itself and routes here.
    #[must_use]
    pub fn with_config_service(
        client: Arc<dyn LlmClient>,
        tools: ToolRegistry,
        config: Arc<ConfigService>,
    ) -> Self {
        Self::assemble(
            Some(client),
            Arc::new(tools),
            in_memory_store(),
            Some(config),
            SourceRegistry::new(),
        )
    }

    /// Registers a [`TurnCompleteListener`] invoked once after every run
    /// terminal on every live session (`docs/CLI.md` §4.5).
    ///
    /// This is the registration seam for turn-complete consumers beyond the
    /// built-in configuration apply — desktop notifications, usage accounting,
    /// session-title generation. Listeners run synchronously on the session's
    /// driver thread in registration order; a panicking listener is logged and
    /// skipped without affecting the driver or later listeners. Registration
    /// after sessions already exist is fine: the registry is shared, so later
    /// terminals on those sessions still reach the new listener.
    pub fn add_turn_complete_listener(&self, listener: Arc<dyn TurnCompleteListener>) {
        self.inner.turn_complete.add_listener(listener);
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
        Ok(Self::assemble(
            Some(client),
            Arc::new(tools),
            store,
            None,
            SourceRegistry::new(),
        ))
    }

    /// The AI source registry this engine was assembled with
    /// (`docs/CLI.md` §4.6).
    ///
    /// Engines built by [`from_config`](Engine::from_config) hold one
    /// [`LlmSource`](mag_sources::LlmSource) per `[providers.<name>]` entry and
    /// one reserved local-agent slot per `[external_agents.<name>]` entry
    /// (decision D3; session drivers consume those entries for ACP delegation). Engines
    /// built by the other constructors hold an empty registry.
    #[must_use]
    pub fn sources(&self) -> &SourceRegistry {
        &self.inner.sources
    }

    /// Assembles an engine over an already-opened persistence store.
    pub(crate) fn assemble(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        store: Arc<Persistence>,
        config: Option<Arc<ConfigService>>,
        sources: SourceRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(EngineInner::new(client, tools, store, config, sources)),
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

    async fn pivot_message(&self, id: SessionId, input: UserInput) -> Result<(), ServiceError> {
        {
            let sessions = self.inner.sessions.lock().await;
            if !sessions.contains_key(&id) {
                return Err(ServiceError::SessionNotFound { id });
            }
        }
        // Attachments follow the `send_message` envelope: only the text is
        // pivoted into the run (`docs/CLI.md` §3.2).
        self.inner.manager.pivot_message(id, input.text).await
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
        Ok(self
            .inner
            .config_apply
            .as_ref()
            .map(|state| source_infos(&state.service().current()))
            .unwrap_or_default())
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        let available = self
            .inner
            .config_apply
            .as_ref()
            .map(|state| local_agent_source_infos(&state.service().current()))
            .unwrap_or_default();
        let _ = self
            .inner
            .event_bus
            .emit(mag_service::Event::LocalAgentsProbed {
                available: available.clone(),
            });
        Ok(available)
    }

    // —— Runtime configuration ——
    //
    // Live when the engine was assembled with a `ConfigService`
    // (`Engine::with_config_service`, `docs/CLI.md` §4.3–§4.5);
    // otherwise the configuration surface reports `Unsupported`; source listing
    // remains available and simply returns an empty set without configuration.
    async fn get_config(&self) -> Result<mag_service::ConfigDto, ServiceError> {
        let Some(config_apply) = &self.inner.config_apply else {
            return Err(ServiceError::Unsupported {
                operation: "get_config".to_owned(),
            });
        };
        // Projecting the current snapshot back to a DTO keeps secrets in
        // their reference form (`docs/CLI.md` §4.1/§4.3).
        Ok(config_apply.service().current().project())
    }

    async fn update_config(&self, config: mag_service::ConfigDto) -> Result<(), ServiceError> {
        let Some(config_apply) = &self.inner.config_apply else {
            return Err(ServiceError::Unsupported {
                operation: "update_config".to_owned(),
            });
        };
        let snapshot = config_apply
            .service()
            .update(config)
            .map_err(config_error)?;
        let _ = self
            .inner
            .event_bus
            .emit(mag_service::Event::ConfigChanged {
                revision: snapshot.revision(),
            });
        Ok(())
    }

    async fn reload_config(&self) -> Result<(), ServiceError> {
        let Some(config_apply) = &self.inner.config_apply else {
            return Err(ServiceError::Unsupported {
                operation: "reload_config".to_owned(),
            });
        };
        let snapshot = config_apply.service().reload().map_err(config_error)?;
        let _ = self
            .inner
            .event_bus
            .emit(mag_service::Event::ConfigChanged {
                revision: snapshot.revision(),
            });
        Ok(())
    }

    async fn apply_config(&self) -> Result<(), ServiceError> {
        let Some(config_apply) = &self.inner.config_apply else {
            return Err(ServiceError::Unsupported {
                operation: "apply_config".to_owned(),
            });
        };
        // Bump the shared generation, then poke every live session actor:
        // idle sessions apply immediately when they service the command;
        // sessions with a run in flight apply at the run's turn boundary
        // (`docs/CLI.md` §4.4/§4.5).
        let generation = config_apply.bump();
        self.inner.manager.apply_config(generation);
        Ok(())
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
    /// Present when the engine was assembled with a [`ConfigService`]
    /// (`docs/CLI.md` §4.3): backs the `MagService` configuration surface.
    config_apply: Option<ConfigApplyState>,
    /// The AI source registry assembled from the configuration
    /// (`docs/CLI.md` §4.6); empty for engines built without `from_config`.
    sources: SourceRegistry,
    /// Engine-wide turn-complete hook (`docs/CLI.md` §4.5), shared with every
    /// session driver.
    turn_complete: TurnCompleteHub,
}

impl EngineInner {
    fn new(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        store: Arc<Persistence>,
        config: Option<Arc<ConfigService>>,
        sources: SourceRegistry,
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
        let config_apply = config.map(ConfigApplyState::new);
        let turn_complete = TurnCompleteHub::default();
        let manager = SessionManager::new(
            client,
            tools,
            event_bus.clone(),
            store.clone(),
            config_apply.clone(),
            turn_complete.clone(),
        );
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            event_bus,
            session_ids,
            store,
            manager,
            config_apply,
            sources,
            turn_complete,
        }
    }
}

/// Shared config-apply plumbing between the engine and its session actors
/// (`docs/CLI.md` §4.4, decision D2).
///
/// `apply_config` bumps `generation` (the *pending* marker); each session
/// actor tracks the generation it last applied and lands the current snapshot
/// on its agent as soon as it observes a newer generation while at rest —
/// immediately when idle (the actor services the `ApplyConfig` command), or
/// at the next run terminal when a run is in flight (the run-completion path
/// re-checks the generation, which is how the apply rides the turn-complete
/// boundary of §4.5).
#[derive(Clone, Debug)]
pub(crate) struct ConfigApplyState {
    service: Arc<ConfigService>,
    generation: Arc<AtomicU64>,
}

impl ConfigApplyState {
    /// Creates the shared state around `service`, with no apply pending.
    pub(crate) fn new(service: Arc<ConfigService>) -> Self {
        Self {
            service,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The configuration service backing this apply channel.
    pub(crate) fn service(&self) -> &Arc<ConfigService> {
        &self.service
    }

    /// Marks a new apply request pending, returning its generation.
    pub(crate) fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The generation of the newest apply request (0 = none ever requested).
    pub(crate) fn pending(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// Opens a private in-memory persistence store for the non-durable engine
/// constructors, panicking only if SQLite cannot open an in-memory database
/// (which does not happen in practice).
pub(crate) fn in_memory_store() -> Arc<Persistence> {
    Arc::new(Persistence::in_memory().expect("open in-memory persistence store"))
}

/// Maps a [`PersistenceError`] into a service-level [`ServiceError::Backend`].
fn persistence_backend(error: PersistenceError) -> ServiceError {
    ServiceError::Backend {
        message: error.to_string(),
    }
}

/// Maps a [`ConfigError`] into the service-level [`ServiceError::Config`].
///
/// The error's Display carries the dotted field path / line-column detail but
/// never a materialized secret value (secrets stay `{env=...}` references in
/// the file and DTO, `docs/CLI.md` §4.1).
fn config_error(error: ConfigError) -> ServiceError {
    ServiceError::Config {
        message: error.to_string(),
    }
}

/// Projects the current configuration snapshot into the service-level source
/// listing (`docs/CLI.md` §4.6 / §5 P7).
fn source_infos(snapshot: &ConfigSnapshot) -> Vec<SourceInfo> {
    let mut infos = Vec::new();
    infos.extend(
        snapshot
            .providers()
            .values()
            .map(|provider| provider_source_info(provider)),
    );
    infos.extend(local_agent_source_infos(snapshot));
    infos
}

/// Projects only local/external agents and performs a lightweight availability
/// check suitable for `probe_local_agents`.
fn local_agent_source_infos(snapshot: &ConfigSnapshot) -> Vec<SourceInfo> {
    snapshot
        .external_agents()
        .values()
        .map(|external| external_agent_source_info(external))
        .collect()
}

fn provider_source_info(provider: &ResolvedProvider) -> SourceInfo {
    SourceInfo {
        id: provider.name().to_owned(),
        name: provider.name().to_owned(),
        kind: SourceKindWire::LlmProvider,
        available: true,
        version: None,
        path: provider.base_url().map(str::to_owned),
        capabilities: vec![provider.wire().as_str().to_owned()],
    }
}

fn external_agent_source_info(external: &ResolvedExternalAgent) -> SourceInfo {
    let command = external.command();
    let path = command.first().cloned();
    let available = path.as_deref().is_some_and(command_available);
    if !available {
        tracing::warn!(
            source = external.name(),
            command = ?command,
            "external ACP source is not currently available"
        );
    }
    SourceInfo {
        id: external.name().to_owned(),
        name: external.name().to_owned(),
        kind: SourceKindWire::LocalAgent,
        available,
        version: None,
        path,
        capabilities: external.capabilities().to_vec(),
    }
}

fn command_available(binary: &str) -> bool {
    if binary.trim().is_empty() {
        return false;
    }
    let path = Path::new(binary);
    if path.is_absolute() || binary.contains(std::path::MAIN_SEPARATOR) {
        return executable_file(path);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| executable_file(&dir.join(binary)))
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
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

        assert_eq!(engine.list_sources().await, Ok(Vec::new()));
        assert_eq!(engine.probe_local_agents().await, Ok(Vec::new()));

        // Runtime configuration: unsupported until the `ConfigService` wiring
        // lands (M3-5/M3-6).
        assert_eq!(
            engine.get_config().await,
            Err(ServiceError::Unsupported {
                operation: "get_config".to_owned(),
            })
        );
        assert_eq!(
            engine
                .update_config(mag_service::ConfigDto::default())
                .await,
            Err(ServiceError::Unsupported {
                operation: "update_config".to_owned(),
            })
        );
        assert_eq!(
            engine.reload_config().await,
            Err(ServiceError::Unsupported {
                operation: "reload_config".to_owned(),
            })
        );
        assert_eq!(
            engine.apply_config().await,
            Err(ServiceError::Unsupported {
                operation: "apply_config".to_owned(),
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
                ..
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
                ..
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

#[cfg(test)]
mod pivot {
    //! Pivot queue bypass tests driven through the public [`MagService`]
    //! surface (`docs/CLI.md` §3.2, decision D1; M1-2).
    //!
    //! Everything runs offline through a scripted [`FakeLlmClient`]. A
    //! gate-held stub tool (and gated scripts) park the run at deterministic
    //! points, so the pivot command — issued from the test thread — provably
    //! reaches the session actor before the run moves on.

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
        model::{
            content::ContentBlock,
            message::{Message, Role},
            tool::Tool,
            usage::Usage,
        },
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use mag_service::{
        MagService, RoutingMode, RunErrorKind, ServiceError, ServiceEvent, SessionConfig,
        SessionId, UserInput,
    };
    use mag_tools::{ToolPlugin, ToolRegistry};
    use serde_json::{Value, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::test_support::{
        FakeLlmClient, StreamGate, gated_text_stream, stalling_text_stream, text_stream_with_usage,
        tool_use_stream,
    };

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
                "mag-pivot-{}-{nanos}-{unique}.sqlite",
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

    /// A stub tool that parks on `gate` before answering, holding the run in
    /// flight so a pivot can be queued deterministically from another thread.
    #[derive(Debug)]
    struct GatedTool {
        gate: Arc<StreamGate>,
    }

    #[async_trait]
    impl ToolPlugin for GatedTool {
        fn name(&self) -> &str {
            "hold"
        }

        fn declaration(&self) -> Tool {
            Tool {
                name: "hold".to_owned(),
                description: "stub tool that waits on a test gate".to_owned(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
            self.gate.wait().await;
            ToolResult::text("held")
        }
    }

    fn gated_registry(gate: Arc<StreamGate>) -> ToolRegistry {
        ToolRegistry::new().register(Arc::new(GatedTool { gate }))
    }

    fn config() -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: "fake-pivot".to_owned(),
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

    async fn create_session(engine: &Engine) -> SessionId {
        engine
            .create_session(config())
            .await
            .expect("create session")
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

    /// Index of the first event matching `predicate`, panicking when absent.
    fn position(events: &[ServiceEvent], predicate: impl Fn(&ServiceEvent) -> bool) -> usize {
        events
            .iter()
            .position(predicate)
            .expect("expected event is present")
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
    async fn pivot_mid_run_is_applied_at_the_step_boundary_and_enters_llm_context() {
        let gate = StreamGate::new();
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("hold", "call-1", json!({})),
            text_stream_with_usage(&["ack"], usage(5, 2)),
        ]);
        let client: Arc<dyn LlmClient> = fake.clone();
        let engine = Engine::with_llm_client_and_tools(client, gated_registry(gate.clone()));
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("start"))
            .await
            .expect("send message");
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, .. } if id == session
        ));
        // The stub tool is parked on the gate; the run is deterministically
        // in flight when the pivot command is issued.
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::ToolStarted { id, .. } if id == session
        ));

        // The actor announces `PivotQueued` before the call returns `Ok`.
        engine
            .pivot_message(session, UserInput::text("pivot please"))
            .await
            .expect("pivot queued while the run is in flight");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::PivotQueued { id: session }
        );

        // Releasing the tool lets the run reach its step boundary, where the
        // driver's post-poll drain lands the pivot through `interject()`.
        gate.open();
        let rest = collect_until_terminal(&mut events).await;

        let applied = position(
            &rest,
            |event| matches!(event, ServiceEvent::PivotApplied { id } if *id == session),
        );
        assert!(
            matches!(rest.last(), Some(ServiceEvent::RunFinished { id, .. }) if *id == session),
            "the pivoted run must finish: {rest:?}",
        );
        assert!(
            applied < rest.len() - 1,
            "PivotApplied precedes the terminal event: {rest:?}",
        );
        assert!(
            !rest
                .iter()
                .any(|event| matches!(event, ServiceEvent::PivotDropped { .. })),
            "an applied pivot is never dropped: {rest:?}",
        );

        // The accepted pivot enters the conversation as a user message, so
        // the follow-up LLM request carries it.
        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 2, "tool step plus pivoted final step");
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == Role::User
                    && text(message).contains("pivot please")),
            "second LLM request should include the pivot user message: {:?}",
            requests[1].messages,
        );
    }

    #[tokio::test]
    async fn pivot_without_an_in_progress_run_reports_not_pivotable() {
        let fake = FakeLlmClient::scripted(Vec::new());
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_llm_client(client);
        let session = create_session(&engine).await;

        // An idle session has no in-progress run: first-layer pivot semantics
        // report `NotPivotable` and never fall back to `send_message`.
        let error = engine
            .pivot_message(session, UserInput::text("hi"))
            .await
            .expect_err("an idle session cannot accept a pivot");
        assert!(
            matches!(error, ServiceError::NotPivotable { id, .. } if id == session),
            "expected NotPivotable, got {error:?}",
        );

        // An unknown session reports not-found, not NotPivotable.
        let missing = SessionId::new(Uuid::from_u128(0xdead));
        assert_eq!(
            engine.pivot_message(missing, UserInput::text("hi")).await,
            Err(ServiceError::SessionNotFound { id: missing })
        );

        // A clientless engine spawns no session actor, so it never has an
        // in-progress run either.
        let clientless = Engine::new();
        let idle = create_session(&clientless).await;
        let error = clientless
            .pivot_message(idle, UserInput::text("hi"))
            .await
            .expect_err("a clientless engine cannot accept a pivot");
        assert!(
            matches!(error, ServiceError::NotPivotable { id, .. } if id == idle),
            "expected NotPivotable, got {error:?}",
        );
    }

    #[tokio::test]
    async fn queued_pivot_is_dropped_when_the_run_is_cancelled() {
        let fake = FakeLlmClient::scripted_streams(vec![stalling_text_stream(&["working"])]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_llm_client(client);
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("start"))
            .await
            .expect("send message");
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, .. } if id == session
        ));
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "working".to_owned(),
            }
        );

        // The run is stalled mid-stream: a single text step never opens a
        // pivot boundary, so the pivot stays queued.
        engine
            .pivot_message(session, UserInput::text("too late"))
            .await
            .expect("pivot queued while the run is in flight");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::PivotQueued { id: session }
        );

        // A cancel preempts the run; the queued pivot is dropped with the
        // cancellation reason before the terminal event.
        engine.cancel(session).await.expect("cancel session");
        let rest = collect_until_terminal(&mut events).await;

        let dropped = position(
            &rest,
            |event| matches!(event, ServiceEvent::PivotDropped { id, reason } if *id == session && reason.contains("cancelled")),
        );
        let terminal = position(
            &rest,
            |event| matches!(event, ServiceEvent::RunError { id, kind, .. } if *id == session && *kind == RunErrorKind::Cancelled),
        );
        assert!(
            dropped < terminal,
            "PivotDropped precedes the terminal cancellation: {rest:?}",
        );
    }

    #[tokio::test]
    async fn queued_pivot_is_dropped_when_the_run_finishes_without_a_boundary() {
        let gate = StreamGate::new();
        let fake = FakeLlmClient::scripted_streams(vec![gated_text_stream(
            &["almost"],
            gate.clone(),
            usage(3, 1),
        )]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_llm_client(client);
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("start"))
            .await
            .expect("send message");
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, .. } if id == session
        ));
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::TextDelta {
                id: session,
                text: "almost".to_owned(),
            }
        );

        // The run is parked mid-response on the gate; its single text step
        // has no pivot boundary left, so the pivot can never land.
        engine
            .pivot_message(session, UserInput::text("redirect"))
            .await
            .expect("pivot queued while the run is in flight");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::PivotQueued { id: session }
        );

        // The run finishing preempts the queued pivot: it is dropped with the
        // normal-completion reason before the terminal event.
        gate.open();
        let rest = collect_until_terminal(&mut events).await;

        let dropped = position(
            &rest,
            |event| matches!(event, ServiceEvent::PivotDropped { id, reason } if *id == session && reason.contains("finished")),
        );
        let terminal = position(
            &rest,
            |event| matches!(event, ServiceEvent::RunFinished { id, output } if *id == session && output.text == "almost"),
        );
        assert!(
            dropped < terminal,
            "PivotDropped precedes the terminal finish: {rest:?}",
        );
    }

    #[tokio::test]
    async fn applied_pivot_is_persisted_with_the_committed_snapshot() {
        let db = TempDb::new();
        let gate = StreamGate::new();
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("hold", "call-1", json!({})),
            text_stream_with_usage(&["ack"], usage(5, 2)),
        ]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_persistence(client, gated_registry(gate.clone()), &db.path)
            .expect("open persistent engine");
        let session = create_session(&engine).await;
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("start"))
            .await
            .expect("send message");
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, .. } if id == session
        ));
        assert!(matches!(
            next_event(&mut events).await,
            ServiceEvent::ToolStarted { id, .. } if id == session
        ));
        engine
            .pivot_message(session, UserInput::text("pivot please"))
            .await
            .expect("pivot queued while the run is in flight");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::PivotQueued { id: session }
        );
        gate.open();
        let rest = collect_until_terminal(&mut events).await;
        assert!(
            rest.iter()
                .any(|event| matches!(event, ServiceEvent::PivotApplied { id } if *id == session)),
            "the pivot lands at the step boundary: {rest:?}",
        );
        assert!(
            matches!(rest.last(), Some(ServiceEvent::RunFinished { id, .. }) if *id == session),
            "the pivoted run must finish: {rest:?}",
        );

        // The committed snapshot — persisted before `RunFinished` fired — is
        // the same persistence path `send_message` turns use, so the pivot
        // message survives a restart.
        let snapshot = engine
            .inner
            .store
            .load_snapshot(session)
            .expect("load snapshot")
            .expect("a committed run must persist a snapshot");
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(
            json.contains("pivot please"),
            "persisted snapshot should carry the pivot message: {json}",
        );
        assert!(
            json.contains("start"),
            "persisted snapshot should carry the original message: {json}",
        );
    }
}

#[cfg(test)]
mod config_apply {
    //! M3-5 integration tests: turn-complete listener fan-out and the
    //! `apply_config` → turn-boundary reconfigure flow (`docs/CLI.md`
    //! §4.4/§4.5), all offline over the scripted [`FakeLlmClient`].

    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use agent_lib::{client::LlmClient, model::usage::Usage};
    use futures::StreamExt;
    use futures::stream::BoxStream;
    use mag_service::{
        MagService, RoutingMode, ServiceError, ServiceEvent, SessionConfig, SessionId, UserInput,
    };
    use tokio::time::{Duration, timeout};

    use crate::test_support::{
        FakeLlmClient, StreamGate, StreamScript, gated_text_stream, stalling_text_stream,
        text_stream_with_usage,
    };
    use crate::{ConfigService, TurnCompleteListener, TurnCompletion, TurnSummary};

    use super::Engine;

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// `TempConfigDir` helper in `config.rs` tests).
    struct TempConfigDir(PathBuf);

    impl TempConfigDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("mag-apply-{}-{nanos}-{unique}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Config whose `agents.default` only overrides the model.
    const CONFIG_MODEL_B: &str = r#"
[agents.default]
model = "model-b"
"#;

    /// Config whose `agents.default` overrides the model and narrows the tool
    /// surface to a subset of the built-ins.
    const CONFIG_MODEL_B_TOOLS: &str = r#"
[agents.default]
model = "model-b"
tools = ["read_file"]
"#;

    fn session_config(model: &str) -> SessionConfig {
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

    /// Builds an engine wired to a [`ConfigService`] serving `toml`, plus the
    /// scripted fake client.
    fn engine_with_config(
        toml: &str,
        scripts: Vec<StreamScript>,
    ) -> (TempConfigDir, Engine, Arc<FakeLlmClient>) {
        let dir = TempConfigDir::new();
        fs::write(dir.config_path(), toml).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted_streams(scripts);
        let client: Arc<dyn LlmClient> = fake.clone();
        let engine =
            Engine::with_config_service(client, mag_tools::ToolRegistry::with_builtins(), service);
        (dir, engine, fake)
    }

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(5), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    /// Waits until the nth `RunFinished` for `session` arrives, returning once
    /// the run's terminal event has been observed.
    async fn wait_run_finished(events: &mut BoxStream<'static, ServiceEvent>, session: SessionId) {
        timeout(Duration::from_secs(5), async {
            loop {
                if let ServiceEvent::RunFinished { id, .. } = next_event(events).await {
                    assert_eq!(id, session);
                    return;
                }
            }
        })
        .await
        .expect("run did not finish in time");
    }

    struct PanickingListener;

    impl TurnCompleteListener for PanickingListener {
        fn on_turn_complete(&self, _summary: &TurnSummary) {
            panic!("listener blew up");
        }
    }

    /// Listener recording every summary it receives, with a `Notify` so tests
    /// can await delivery (the callback runs on the session's driver thread).
    #[derive(Default)]
    struct RecordingListener {
        summaries: Mutex<Vec<TurnSummary>>,
        wake: tokio::sync::Notify,
    }

    impl TurnCompleteListener for RecordingListener {
        fn on_turn_complete(&self, summary: &TurnSummary) {
            self.summaries
                .lock()
                .expect("recording lock")
                .push(summary.clone());
            self.wake.notify_one();
        }
    }

    impl RecordingListener {
        /// Awaits at least `n` recorded summaries and returns a snapshot.
        async fn wait_for(&self, n: usize) -> Vec<TurnSummary> {
            timeout(Duration::from_secs(5), async {
                loop {
                    {
                        let recorded = self.summaries.lock().expect("recording lock");
                        if recorded.len() >= n {
                            return recorded.clone();
                        }
                    }
                    self.wake.notified().await;
                }
            })
            .await
            .expect("turn-complete listener timed out")
        }
    }

    /// M3-5 (b): a session with no in-progress run applies the current
    /// snapshot immediately — the very first run after `apply_config` already
    /// uses the new model and the narrowed tool surface.
    #[tokio::test]
    async fn apply_config_on_idle_session_applies_immediately() {
        let (_dir, engine, fake) = engine_with_config(
            CONFIG_MODEL_B_TOOLS,
            vec![StreamScript::Complete(text_stream_with_usage(
                &["ok"],
                usage(1, 1),
            ))],
        );
        let session = engine
            .create_session(session_config("fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine.apply_config().await.expect("apply config");
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        wait_run_finished(&mut events, session).await;

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "model-b");
        let tool_names: Vec<&str> = requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(tool_names, vec!["read_file"]);
    }

    /// M3-5 (a): `apply_config` arriving mid-run lands at the turn boundary —
    /// the in-flight run keeps its original configuration to the end (the
    /// facade only admits reconfiguration at rest), and the reconfigure takes
    /// effect for the next run. With the M3-6 session ↔ agent binding the
    /// first run already uses the bound entry's model from creation, so the
    /// boundary is demonstrated by updating the config mid-run.
    #[tokio::test]
    async fn apply_config_during_run_lands_at_the_turn_boundary() {
        let gate = StreamGate::new();
        let (_dir, engine, fake) = engine_with_config(
            CONFIG_MODEL_B,
            vec![
                gated_text_stream(&["first"], gate.clone(), usage(1, 1)),
                StreamScript::Complete(text_stream_with_usage(&["second"], usage(1, 1))),
            ],
        );
        let session = engine
            .create_session(session_config("fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("one"))
            .await
            .expect("first send");
        // Wait until the run is genuinely in flight (parked on the gate).
        loop {
            if matches!(
                next_event(&mut events).await,
                ServiceEvent::TextDelta { .. }
            ) {
                break;
            }
        }
        // Create-time binding: the in-flight run already carries the bound
        // entry's model (docs/CLI.md §4.4: new sessions use the current DO
        // graph).
        assert_eq!(fake.stream_requests().len(), 1);
        assert_eq!(fake.stream_requests()[0].model, "model-b");

        // Update the config mid-run and apply: the in-flight run is untouched.
        let mut dto = engine.get_config().await.expect("get config");
        dto.agents.get_mut("default").expect("default agent").model = Some("model-c".to_owned());
        engine.update_config(dto).await.expect("update config");
        engine.apply_config().await.expect("apply config mid-run");
        assert_eq!(fake.stream_requests().len(), 1);

        gate.open();
        wait_run_finished(&mut events, session).await;

        engine
            .send_message(session, UserInput::text("two"))
            .await
            .expect("second send");
        wait_run_finished(&mut events, session).await;

        // The reconfigure happened after the first run's terminal (it is only
        // admitted at rest): the second run's request carries the new model.
        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].model, "model-c");
    }

    /// M3-5 (c): a panicking listener is isolated — later listeners still
    /// receive the summary and the run's event flow is undisturbed.
    #[tokio::test]
    async fn panicking_listener_is_isolated_from_run_and_other_listeners() {
        let fake = FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_llm_client(client);
        engine.add_turn_complete_listener(Arc::new(PanickingListener));
        let recorder = Arc::new(RecordingListener::default());
        engine.add_turn_complete_listener(recorder.clone());

        let session = engine
            .create_session(session_config("fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        wait_run_finished(&mut events, session).await;

        let summaries = recorder.wait_for(1).await;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id(), session);
        assert_eq!(summaries[0].completion(), TurnCompletion::Committed);
    }

    /// The hook fires for every terminal kind, not just successful runs: a
    /// cancelled run reports `Cancelled`.
    #[tokio::test]
    async fn listener_observes_cancelled_completion() {
        let fake = FakeLlmClient::scripted_streams(vec![stalling_text_stream(&["hi"])]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_llm_client(client);
        let recorder = Arc::new(RecordingListener::default());
        engine.add_turn_complete_listener(recorder.clone());

        let session = engine
            .create_session(session_config("fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        // Wait for the run to be in flight, then cancel it.
        loop {
            match next_event(&mut events).await {
                ServiceEvent::RunStarted { .. } => break,
                _ => continue,
            }
        }
        engine.cancel(session).await.expect("cancel");
        loop {
            match next_event(&mut events).await {
                ServiceEvent::RunError {
                    kind: mag_service::RunErrorKind::Cancelled,
                    ..
                } => break,
                _ => continue,
            }
        }

        let summaries = recorder.wait_for(1).await;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].completion(), TurnCompletion::Cancelled);
    }

    /// The four `MagService` configuration methods proxy the injected
    /// `ConfigService`: reads project the current snapshot, writes bump the
    /// revision and emit `ConfigChanged`, and failures surface as
    /// `ServiceError::Config` without disturbing the snapshot.
    #[tokio::test]
    async fn config_methods_proxy_the_config_service() {
        let (_dir, engine, _fake) = engine_with_config(CONFIG_MODEL_B, Vec::new());
        let mut events = engine.subscribe(None);

        // get_config projects the current snapshot (DTO form).
        let dto = engine.get_config().await.expect("get config");
        assert_eq!(
            dto.agents
                .get("default")
                .and_then(|agent| agent.model.as_deref()),
            Some("model-b")
        );

        // update_config swaps the snapshot and announces the new revision.
        let mut updated = dto.clone();
        updated
            .agents
            .get_mut("default")
            .expect("default agent")
            .model = Some("model-c".to_owned());
        engine
            .update_config(updated.clone())
            .await
            .expect("update config");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::ConfigChanged { revision: 1 }
        );
        assert_eq!(
            engine.get_config().await.expect("get updated config"),
            updated
        );

        // reload_config picks up an external edit.
        fs::write(
            _dir.config_path(),
            CONFIG_MODEL_B.replace("model-b", "model-d"),
        )
        .expect("external edit");
        engine.reload_config().await.expect("reload config");
        assert_eq!(
            next_event(&mut events).await,
            ServiceEvent::ConfigChanged { revision: 2 }
        );
        let dto = engine.get_config().await.expect("get reloaded config");
        assert_eq!(
            dto.agents
                .get("default")
                .and_then(|agent| agent.model.as_deref()),
            Some("model-d")
        );

        // A failing update surfaces `ServiceError::Config` and changes
        // nothing.
        let invalid = mag_service::ConfigDto::parse_str("[agents.default]\nprovider = \"ghost\"\n")
            .expect("invalid-reference DTO still parses");
        let error = engine
            .update_config(invalid)
            .await
            .expect_err("invalid update must fail");
        assert!(matches!(error, ServiceError::Config { .. }), "got: {error}");
        assert_eq!(engine.get_config().await.expect("config unchanged"), dto);
    }

    /// `apply_config` with no configuration backend stays `Unsupported`
    /// (engines built by the other constructors).
    #[tokio::test]
    async fn apply_config_without_config_service_is_unsupported() {
        let engine = Engine::new();
        assert_eq!(
            engine.apply_config().await,
            Err(ServiceError::Unsupported {
                operation: "apply_config".to_owned(),
            })
        );
    }
}

#[cfg(test)]
mod session_binding {
    //! M3-6 integration tests: the session ↔ agent binding and the configured
    //! approval tiers, end-to-end over the scripted [`FakeLlmClient`]
    //! (`docs/CLI.md` §4.4; see the [`Engine::from_config`] rustdoc).

    use std::{
        fs,
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
        ApprovalDecisionWire, InteractionResponseWire, MagService, RoutingMode, ServiceEvent,
        SessionConfig, StepIdWire, ToolCallIdWire, UserInput,
    };
    use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
    use serde_json::{Value, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::test_support::{FakeLlmClient, text_stream_with_usage, tool_use_stream};
    use crate::{ConfigService, Engine};

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// `TempConfigDir` helper in `config.rs` tests).
    struct TempConfigDir(PathBuf);

    impl TempConfigDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("mag-bind-{}-{nanos}-{unique}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A canned tool plugin (same pattern as `tool_turn::StubTool`): fixed
    /// output, deterministic and offline.
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

    /// Builds an engine backed by a [`ConfigService`] serving `toml`, with the
    /// stub tool registry and the scripted fake client.
    fn engine_with_config(
        toml: &str,
        scripts: Vec<Vec<agent_lib::stream::StreamEvent>>,
    ) -> (TempConfigDir, Engine, Arc<FakeLlmClient>) {
        let dir = TempConfigDir::new();
        fs::write(dir.config_path(), toml).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(scripts);
        let client: Arc<dyn LlmClient> = fake.clone();
        let engine = Engine::with_config_service(client, registry(), service);
        (dir, engine, fake)
    }

    fn session_config(provider: &str, model: &str) -> SessionConfig {
        SessionConfig {
            provider: provider.to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage() -> Usage {
        Usage {
            input: 2,
            output: 1,
            total: Some(3),
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

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(5), futures::StreamExt::next(events))
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

    /// Create-time binding: a new session is assembled from the bound
    /// `agents.default` entry — its model overrides the wire
    /// `SessionConfig.model` and its tool list narrows the surface
    /// (`docs/CLI.md` §4.4: new sessions use the current DO graph).
    #[tokio::test]
    async fn create_session_binds_model_and_tools_from_the_default_entry() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
model = "model-b"
tools = ["read_file"]
"#,
            vec![text_stream_with_usage(&["ok"], usage())],
        );
        let session = engine
            .create_session(session_config("fake", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        collect_until_terminal(&mut events).await;

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "model-b");
        let tool_names: Vec<&str> = requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(tool_names, vec!["read_file"]);
    }

    /// An explicit `tools = []` on the bound entry builds the session with an
    /// empty tool surface — a real constraint, distinct from an absent
    /// `tools` key (unconstrained).
    #[tokio::test]
    async fn explicit_empty_tool_list_builds_a_tool_less_session() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
tools = []
"#,
            vec![text_stream_with_usage(&["ok"], usage())],
        );
        let session = engine
            .create_session(session_config("fake", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        collect_until_terminal(&mut events).await;

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].tools.is_empty(),
            "explicit empty list exposes no tools: {:?}",
            requests[0].tools
        );
    }

    /// A session created with `provider = "reviewer"` binds the
    /// `agents.reviewer` entry, and `apply_config` reconfigures it from that
    /// same bound entry.
    #[tokio::test]
    async fn named_entry_binds_and_apply_config_uses_the_bound_name() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
model = "model-d"

[agents.reviewer]
model = "model-r1"
"#,
            vec![
                text_stream_with_usage(&["one"], usage()),
                text_stream_with_usage(&["two"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("reviewer", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("one"))
            .await
            .expect("first send");
        collect_until_terminal(&mut events).await;
        assert_eq!(fake.stream_requests()[0].model, "model-r1");

        // Update the bound entry and apply: the next run uses the new model.
        let mut dto = engine.get_config().await.expect("get config");
        dto.agents
            .get_mut("reviewer")
            .expect("reviewer agent")
            .model = Some("model-r2".to_owned());
        engine.update_config(dto).await.expect("update config");
        engine.apply_config().await.expect("apply config");

        engine
            .send_message(session, UserInput::text("two"))
            .await
            .expect("second send");
        collect_until_terminal(&mut events).await;
        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].model, "model-r2");
    }

    /// `[approval].default_policy = "ask"` gates even a permission-free
    /// (normally auto-allowed) tool behind an `InteractionRequested`.
    #[tokio::test]
    async fn approval_default_ask_pauses_a_permission_free_tool() {
        let (_dir, engine, _fake) = engine_with_config(
            r#"
[approval]
default_policy = "ask"
"#,
            vec![
                tool_use_stream("read_file", "call-1", json!({ "path": "x" })),
                text_stream_with_usage(&["done"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("fake", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("read"))
            .await
            .expect("send message");

        // Wait for the pause, then approve it.
        let request_id = loop {
            match next_event(&mut events).await {
                ServiceEvent::InteractionRequested { request_id, .. } => break request_id,
                ServiceEvent::RunStarted { .. } => continue,
                other => panic!("expected the run to pause for approval, got {other:?}"),
            }
        };
        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve");

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { trace, .. } if trace.name == "read_file"
            )),
            "the approved tool runs: {rest:?}"
        );
        assert!(
            matches!(
                rest.last().expect("terminal event"),
                ServiceEvent::RunFinished { output, .. } if output.text == "done"
            ),
            "the turn finishes: {rest:?}"
        );
    }

    /// A `[tools.<name>] approval = "allow"` entry overrides an `ask` default
    /// for that tool: no pause, the tool runs directly.
    #[tokio::test]
    async fn per_tool_allow_overrides_an_ask_default() {
        let (_dir, engine, _fake) = engine_with_config(
            r#"
[approval]
default_policy = "ask"

[tools.read_file]
approval = "allow"
"#,
            vec![
                tool_use_stream("read_file", "call-1", json!({ "path": "x" })),
                text_stream_with_usage(&["done"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("fake", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("read"))
            .await
            .expect("send message");

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            !rest
                .iter()
                .any(|event| matches!(event, ServiceEvent::InteractionRequested { .. })),
            "an allowed tool never pauses: {rest:?}"
        );
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { trace, .. } if trace.name == "read_file"
            )),
            "the tool runs without a pause: {rest:?}"
        );
    }

    /// A `[tools.<name>] enabled = false` entry removes the tool from the
    /// bound entry's enabled list, so an agent entry naming it cannot expose
    /// it (the binding filters disabled tools at resolve time).
    #[tokio::test]
    async fn disabled_tool_is_absent_from_the_session_surface() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
tools = ["read_file", "shell"]

[tools.shell]
enabled = false
"#,
            vec![text_stream_with_usage(&["ok"], usage())],
        );
        let session = engine
            .create_session(session_config("fake", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("hi"))
            .await
            .expect("send message");
        collect_until_terminal(&mut events).await;

        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 1);
        let tool_names: Vec<&str> = requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(tool_names, vec!["read_file"]);
    }
}

#[cfg(test)]
mod delegation {
    //! M4-1 integration tests: local LLM subagent delegation assembled from
    //! the configuration's `agents.<name>` entries and the `Delegation*` wire
    //! event mapping, end-to-end over the scripted [`FakeLlmClient`]
    //! (`docs/CLI.md` §5 P7, decision D3/D5).

    use std::{
        fs,
        path::{Path, PathBuf},
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
        ApprovalDecisionWire, ApprovalRequirementWire, InteractionKindWire,
        InteractionResponseWire, MagService, RequestId, RoutingMode, ServiceEvent, SessionConfig,
        SourceKindWire, StepIdWire, ToolCallIdWire, UserInput,
    };
    use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
    use serde_json::{Value, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::test_support::{FakeLlmClient, text_stream_with_usage, tool_use_stream};
    use crate::{ConfigService, Engine};

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// `TempConfigDir` helper in `config.rs` tests).
    struct TempConfigDir(PathBuf);

    impl TempConfigDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("mag-deleg-{}-{nanos}-{unique}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A canned tool plugin (same pattern as `session_binding::StubTool`).
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

    /// Builds an engine backed by a [`ConfigService`] serving `toml`, with the
    /// stub tool registry and the scripted fake client.
    fn engine_with_config(
        toml: &str,
        scripts: Vec<Vec<agent_lib::stream::StreamEvent>>,
    ) -> (TempConfigDir, Engine, Arc<FakeLlmClient>) {
        let dir = TempConfigDir::new();
        fs::write(dir.config_path(), toml).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(scripts);
        let client: Arc<dyn LlmClient> = fake.clone();
        let engine = Engine::with_config_service(client, registry(), service);
        (dir, engine, fake)
    }

    fn session_config(provider: &str, model: &str) -> SessionConfig {
        SessionConfig {
            provider: provider.to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn usage() -> Usage {
        Usage {
            input: 2,
            output: 1,
            total: Some(3),
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

    async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
        timeout(Duration::from_secs(5), futures::StreamExt::next(events))
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

    /// Waits for the next approval interaction and verifies its attribution and
    /// reason mention the expected delegate start tool.
    async fn next_approval_request(
        events: &mut BoxStream<'static, ServiceEvent>,
        expected_delegate: Option<&str>,
        reason_fragment: &str,
    ) -> RequestId {
        loop {
            match next_event(events).await {
                ServiceEvent::InteractionRequested {
                    request_id,
                    kind,
                    origin,
                    ..
                } => {
                    assert_eq!(
                        origin.delegate.as_deref(),
                        expected_delegate,
                        "approval origin matches the expected producer"
                    );
                    match kind {
                        InteractionKindWire::Approval { requirement, .. } => match requirement {
                            ApprovalRequirementWire::RequireApproval { reason } => {
                                let reason = reason.unwrap_or_default();
                                assert!(
                                    reason.contains(reason_fragment),
                                    "approval reason `{reason}` mentions `{reason_fragment}`"
                                );
                            }
                            other => panic!("approval should require a decision, got {other:?}"),
                        },
                        other => panic!("expected an approval interaction, got {other:?}"),
                    }
                    return request_id;
                }
                ServiceEvent::RunStarted { .. } | ServiceEvent::DelegationStarted { .. } => {}
                other => panic!("unexpected event before approval: {other:?}"),
            }
        }
    }

    /// Config with a bound `default` entry and one `researcher` delegate
    /// carrying a pinned model, a system prompt, and a narrowed tool list.
    const CONFIG_WITH_RESEARCHER: &str = r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
system_prompt = "Research thoroughly."
role = "Researches topics and reports findings."
tools = ["read_file"]

[tools.ask_researcher]
approval = "allow"
"#;

    #[cfg(unix)]
    fn fake_acp_script(dir: &TempConfigDir) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.0.join("fake-acp.sh");
        fs::write(
            &path,
r#"#!/bin/sh
set -eu
if [ "$#" -ge 1 ]; then MAG_FAKE_ACP_LOG="$1"; fi
: "${MAG_FAKE_ACP_LOG:?}"
if [ "$#" -ge 2 ]; then mode="$2"; else mode="${MAG_FAKE_ACP_MODE:-success}"; fi
if [ "$#" -ge 3 ]; then session="$3"; else session="${MAG_FAKE_ACP_SESSION:-mag-fake-acp-session}"; fi
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$MAG_FAKE_ACP_LOG"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
      ;;
    *'"method":"session/new"'*)
      if [ "$mode" = "crash_new" ]; then exit 7; fi
      printf '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"%s"}}\n' "$session"
      ;;
    *'"method":"session/prompt"'*)
      if [ "$mode" = "crash_prompt" ]; then exit 9; fi
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"external summary"}}}}\n' "$session"
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
      ;;
    *'"method":"session/cancel"'*)
      printf '%s\n' 'SESSION_CANCELLED' >> "$MAG_FAKE_ACP_LOG"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("write fake ACP script");
        let mut permissions = fs::metadata(&path)
            .expect("fake ACP script metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod fake ACP script");
        path
    }

    fn toml_string(value: &Path) -> String {
        value
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    fn external_acp_config(script: &Path, log: &Path, mode: &str) -> String {
        external_acp_config_with_start_policy(script, log, mode, Some("allow"))
    }

    fn external_acp_config_with_start_policy(
        script: &Path,
        log: &Path,
        mode: &str,
        start_approval: Option<&str>,
    ) -> String {
        let start_approval = start_approval
            .map(|approval| format!("\n[tools.ask_peer]\napproval = \"{approval}\"\n"))
            .unwrap_or_default();
        format!(
            r#"
[agents.default]
model = "model-d"

[external_agents.peer]
kind = "acp"
command = ["{}", "{}", "{}", "mag-fake-acp-session"]
capabilities = ["streaming", "graceful_shutdown"]

[external_agents.peer.env]
MAG_FAKE_ACP_LOG = "{}"
MAG_FAKE_ACP_MODE = "{}"
MAG_FAKE_ACP_SESSION = "mag-fake-acp-session"
{}"#,
            toml_string(script),
            toml_string(log),
            mode,
            toml_string(log),
            mode,
            start_approval,
        )
    }

    /// M4-1 main path: the supervisor calls `ask_researcher` and the run emits
    /// `DelegationStarted` → `DelegationFinished` in order with a complete
    /// trace; the delegate runs on the shared client with its own model and
    /// system prompt.
    #[tokio::test]
    async fn ask_researcher_emits_the_delegation_lifecycle_in_order() {
        let (_dir, engine, fake) = engine_with_config(
            CONFIG_WITH_RESEARCHER,
            vec![
                tool_use_stream("ask_researcher", "del-1", json!({ "task": "find facts" })),
                text_stream_with_usage(&["research summary"], usage()),
                text_stream_with_usage(&["final answer"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("research this"))
            .await
            .expect("send message");

        let collected = collect_until_terminal(&mut events).await;

        // Lifecycle order: DelegationStarted precedes DelegationFinished
        // precedes the terminal RunFinished; no ToolStarted/ToolFinished pair
        // brackets the `ask_researcher` call.
        let started = collected
            .iter()
            .position(|event| matches!(event, ServiceEvent::DelegationStarted { .. }))
            .expect("a DelegationStarted event");
        let finished = collected
            .iter()
            .position(|event| matches!(event, ServiceEvent::DelegationFinished { .. }))
            .expect("a DelegationFinished event");
        assert!(
            started < finished,
            "started precedes finished: {collected:?}"
        );
        assert!(
            !collected.iter().any(|event| matches!(
                event,
                ServiceEvent::ToolStarted { trace, .. } if trace.name == "ask_researcher"
            )),
            "a delegation call emits no tool lifecycle events: {collected:?}"
        );
        assert!(
            matches!(collected.last(), Some(ServiceEvent::RunFinished { .. })),
            "the run finishes: {collected:?}"
        );

        // Trace contents: the delegate name is populated on both events.
        for event in &collected[started..=finished] {
            match event {
                ServiceEvent::DelegationStarted { id, trace }
                | ServiceEvent::DelegationFinished { id, trace } => {
                    assert_eq!(*id, session);
                    assert_eq!(trace.delegate, "researcher");
                }
                _ => {}
            }
        }
        assert!(
            !collected
                .iter()
                .any(|event| matches!(event, ServiceEvent::DelegationFailed { .. })),
            "a successful delegation never fails: {collected:?}"
        );

        // The supervisor's tool surface advertises the delegation tool.
        let requests = fake.stream_requests();
        assert_eq!(requests.len(), 2, "supervisor turn + continuation");
        let tool_names: Vec<&str> = requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(
            tool_names.contains(&"ask_researcher"),
            "ask_researcher on the tool surface: {tool_names:?}"
        );

        // The delegate runs on the shared client (non-streaming endpoint) with
        // its pinned model and configured system prompt.
        let child_requests = fake.chat_requests();
        assert_eq!(child_requests.len(), 1, "one delegate drive");
        assert_eq!(child_requests[0].model, "model-r");
        assert_eq!(
            child_requests[0].system.as_deref(),
            Some("Research thoroughly.")
        );
    }

    /// M4-1 origin attribution (M2 wiring, verified end-to-end): a tool the
    /// delegate gates pauses through the root session's `IpcApproval`, so the
    /// root receives an `InteractionRequested` carrying the delegate's origin;
    /// approving it lets the delegation finish.
    #[tokio::test]
    async fn delegate_tool_approval_pops_to_root_with_origin() {
        let (_dir, engine, _fake) = engine_with_config(
            r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
tools = ["shell"]

[tools.ask_researcher]
approval = "allow"
"#,
            vec![
                tool_use_stream("ask_researcher", "del-1", json!({ "task": "inspect" })),
                tool_use_stream("shell", "child-1", json!({ "cmd": "ls" })),
                text_stream_with_usage(&["shell unavailable; reporting from memory"], usage()),
                text_stream_with_usage(&["final answer"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("inspect"))
            .await
            .expect("send message");

        // agent-lib drives the child synchronously inside the delegation
        // tool's fulfill and emits both live delegation events only once the
        // child settles, so the child's pause reaches the root *before* any
        // `DelegationStarted` (recorded in the M4-1 completion notes). Consume
        // events until the interaction surfaces.
        let request_id = loop {
            match next_event(&mut events).await {
                ServiceEvent::InteractionRequested {
                    request_id,
                    kind,
                    origin,
                    ..
                } => {
                    assert!(
                        matches!(kind, InteractionKindWire::Approval { .. }),
                        "the child's pause is an approval: {kind:?}"
                    );
                    assert_eq!(
                        origin.delegate.as_deref(),
                        Some("researcher"),
                        "origin names the delegate"
                    );
                    assert_eq!(origin.depth, 1, "origin carries the delegation depth");
                    assert!(!origin.is_root());
                    break request_id;
                }
                ServiceEvent::RunStarted { .. } | ServiceEvent::DelegationStarted { .. } => {}
                other => panic!("unexpected event before the interaction: {other:?}"),
            }
        };

        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve the child's tool");

        // The approved child finishes its turn (the declaration-only child
        // surface answers the approved `shell` call with the facade's
        // declaration-only `UnknownTool` result, which the child model absorbs
        // and summarises), so the delegation completes and the run finishes.
        let rest = collect_until_terminal(&mut events).await;
        let started = rest
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ServiceEvent::DelegationStarted { trace, .. } if trace.delegate == "researcher"
                )
            })
            .expect("a DelegationStarted event");
        let finished = rest
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "researcher"
                )
            })
            .expect("a DelegationFinished event");
        assert!(started < finished, "started precedes finished: {rest:?}");
        assert!(
            matches!(
                rest.last(),
                Some(ServiceEvent::RunFinished { output, .. }) if output.text == "final answer"
            ),
            "the run finishes: {rest:?}"
        );
    }

    /// M4-3: starting a local delegation is an approval point by default. A
    /// denied `ask_researcher` call never drives the child agent; the denied tool
    /// result is instead fed back to the supervisor, which can continue.
    #[tokio::test]
    async fn delegate_start_denial_does_not_drive_the_local_delegate() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
"#,
            vec![
                tool_use_stream("ask_researcher", "del-1", json!({ "task": "inspect" })),
                text_stream_with_usage(&["delegation denied"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("inspect"))
            .await
            .expect("send message");

        let request_id = next_approval_request(&mut events, None, "ask_researcher").await;
        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Deny))
            .await
            .expect("deny the delegation start");

        let rest = collect_until_terminal(&mut events).await;
        assert!(
            !rest.iter().any(|event| matches!(
                event,
                ServiceEvent::DelegationStarted { .. }
                    | ServiceEvent::DelegationFinished { .. }
                    | ServiceEvent::DelegationFailed { .. }
            )),
            "a denied local start is a rejected tool call, not a driven delegation: {rest:?}"
        );
        assert!(
            matches!(
                rest.last(),
                Some(ServiceEvent::RunFinished { output, .. }) if output.text == "delegation denied"
            ),
            "the supervisor continues after the rejected tool result: {rest:?}"
        );
        assert!(
            fake.chat_requests().is_empty(),
            "the denied delegate must not consume a child LLM request"
        );
    }

    /// M4-3: approving the default start approval lets the local delegate run and
    /// preserves the existing `DelegationStarted` → `DelegationFinished` mapping.
    #[tokio::test]
    async fn delegate_start_approval_allows_the_local_delegate_to_run() {
        let (_dir, engine, fake) = engine_with_config(
            r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
"#,
            vec![
                tool_use_stream("ask_researcher", "del-1", json!({ "task": "inspect" })),
                text_stream_with_usage(&["research summary"], usage()),
                text_stream_with_usage(&["final answer"], usage()),
            ],
        );
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));
        engine
            .send_message(session, UserInput::text("inspect"))
            .await
            .expect("send message");

        let request_id = next_approval_request(&mut events, None, "ask_researcher").await;
        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve the delegation start");

        let rest = collect_until_terminal(&mut events).await;
        let started = rest
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ServiceEvent::DelegationStarted { trace, .. } if trace.delegate == "researcher"
                )
            })
            .expect("a DelegationStarted event");
        let finished = rest
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "researcher"
                )
            })
            .expect("a DelegationFinished event");
        assert!(started < finished, "started precedes finished: {rest:?}");
        assert!(
            matches!(
                rest.last(),
                Some(ServiceEvent::RunFinished { output, .. }) if output.text == "final answer"
            ),
            "the approved delegation finishes the supervisor run: {rest:?}"
        );
        assert_eq!(fake.chat_requests().len(), 1, "the child delegate ran once");
    }

    /// M4-3 restore regression: after a restart, the restored facade must
    /// re-register the local delegate and its approval policy instead of falling
    /// back to agent-lib's snapshot default (`auto_allow`).
    #[tokio::test]
    async fn resume_re_registers_local_delegate_and_start_approval_policy() {
        let dir = TempConfigDir::new();
        let db = dir.0.join("sessions.sqlite");
        fs::write(
            dir.config_path(),
            r#"
[agents.default]
model = "model-d"

[agents.researcher]
model = "model-r"
"#,
        )
        .expect("write config");

        let service1 =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake1 = FakeLlmClient::scripted(vec![text_stream_with_usage(&["ready"], usage())]);
        let client1: Arc<dyn LlmClient> = fake1;
        let engine1 = Engine::assemble(
            Some(client1),
            Arc::new(registry()),
            Arc::new(crate::persistence::Persistence::open(&db).expect("open store")),
            Some(service1),
            mag_sources::SourceRegistry::new(),
        );
        let session = engine1
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events1 = engine1.subscribe(Some(session));
        engine1
            .send_message(session, UserInput::text("warm up"))
            .await
            .expect("send warmup");
        collect_until_terminal(&mut events1).await;
        drop(events1);
        drop(engine1);

        let service2 =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("reload config"));
        let fake2 = FakeLlmClient::scripted(vec![
            tool_use_stream(
                "ask_researcher",
                "del-1",
                json!({ "task": "resume inspect" }),
            ),
            text_stream_with_usage(&["research summary"], usage()),
            text_stream_with_usage(&["resumed final"], usage()),
        ]);
        let client2: Arc<dyn LlmClient> = fake2.clone();
        let engine2 = Engine::assemble(
            Some(client2),
            Arc::new(registry()),
            Arc::new(crate::persistence::Persistence::open(&db).expect("reopen store")),
            Some(service2),
            mag_sources::SourceRegistry::new(),
        );
        engine2
            .resume_session(session)
            .await
            .expect("resume session");
        let mut events2 = engine2.subscribe(Some(session));
        engine2
            .send_message(session, UserInput::text("after resume"))
            .await
            .expect("send after resume");

        let request_id = next_approval_request(&mut events2, None, "ask_researcher").await;
        let first_request = fake2
            .stream_requests()
            .into_iter()
            .next()
            .expect("the resumed supervisor made an LLM request");
        let tool_names: Vec<&str> = first_request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(
            tool_names.contains(&"ask_researcher"),
            "the restored tool surface still advertises ask_researcher: {tool_names:?}"
        );
        engine2
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve restored delegation");
        let rest = collect_until_terminal(&mut events2).await;
        assert!(
            rest.iter().any(|event| matches!(
                event,
                ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "researcher"
            )),
            "the restored delegate runs after approval: {rest:?}"
        );
        assert_eq!(
            fake2.chat_requests().len(),
            1,
            "the restored child ran once"
        );
    }

    /// M4-3 external path: without an explicit `[tools.ask_peer] allow`, a
    /// managed ACP delegate start first asks the root session. Approval then
    /// allows the fake ACP process to run.
    #[cfg(unix)]
    #[tokio::test]
    async fn external_acp_delegate_start_approval_runs_only_after_approval() {
        let dir = TempConfigDir::new();
        let script = fake_acp_script(&dir);
        let log = dir.0.join("fake-acp-approval.log");
        let config = external_acp_config_with_start_policy(&script, &log, "success", None);
        fs::write(dir.config_path(), config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("ask_peer", "del-1", json!({ "task": "inspect" })),
            text_stream_with_usage(&["final answer"], usage()),
        ]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_config_service(client, registry(), service);
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("delegate externally"))
            .await
            .expect("send message");
        let request_id = next_approval_request(&mut events, Some("peer"), "ask_peer").await;
        let log_before_approval = fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !log_before_approval.contains(r#""method":"session/prompt""#),
            "the ACP process must not receive the prompt before approval: {log_before_approval}"
        );

        engine
            .respond_interaction(session, request_id, approval(ApprovalDecisionWire::Approve))
            .await
            .expect("approve external start");
        let collected = collect_until_terminal(&mut events).await;
        let started = collected
            .iter()
            .position(|event| {
                matches!(event, ServiceEvent::DelegationStarted { trace, .. } if trace.delegate == "peer")
            })
            .expect("external DelegationStarted");
        let finished = collected
            .iter()
            .position(|event| {
                matches!(event, ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "peer")
            })
            .expect("external DelegationFinished");
        assert!(
            started < finished,
            "started precedes finished: {collected:?}"
        );
        assert!(
            matches!(collected.last(), Some(ServiceEvent::RunFinished { output, .. }) if output.text == "final answer"),
            "the supervisor continues after the approved external summary: {collected:?}"
        );
        let log_after_approval = fs::read_to_string(&log).expect("fake ACP log after approval");
        assert!(
            log_after_approval.contains(r#""method":"session/prompt""#),
            "the approved ACP process receives the prompt: {log_after_approval}"
        );
    }

    /// M4-2 main path: a configured `external_agents.peer` ACP process is
    /// advertised as `ask_peer`, driven through a local fake ACP subprocess, and
    /// explicitly cleaned up when the mag session is deleted.
    #[cfg(unix)]
    #[tokio::test]
    async fn ask_external_acp_delegate_emits_lifecycle_and_cleans_up_on_delete() {
        let dir = TempConfigDir::new();
        let script = fake_acp_script(&dir);
        let log = dir.0.join("fake-acp.log");
        let config = external_acp_config(&script, &log, "success");
        fs::write(dir.config_path(), config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("ask_peer", "del-1", json!({ "task": "inspect" })),
            text_stream_with_usage(&["final answer"], usage()),
        ]);
        let client: Arc<dyn LlmClient> = fake.clone();
        let engine = Engine::with_config_service(client, registry(), service);
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("delegate externally"))
            .await
            .expect("send message");
        let collected = collect_until_terminal(&mut events).await;

        let started = collected
            .iter()
            .position(|event| {
                matches!(event, ServiceEvent::DelegationStarted { trace, .. } if trace.delegate == "peer")
            })
            .unwrap_or_else(|| panic!("external DelegationStarted in {collected:?}"));
        let finished = collected
            .iter()
            .position(|event| {
                matches!(event, ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "peer")
            })
            .unwrap_or_else(|| {
                let log_text = fs::read_to_string(&log).unwrap_or_else(|error| error.to_string());
                panic!("external DelegationFinished in {collected:?}; fake log: {log_text}")
            });
        assert!(
            started < finished,
            "started precedes finished: {collected:?}"
        );
        assert!(
            matches!(collected.last(), Some(ServiceEvent::RunFinished { output, .. }) if output.text == "final answer"),
            "the supervisor continues after the external summary: {collected:?}"
        );

        let stream_requests = fake.stream_requests();
        let tool_names: Vec<&str> = stream_requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(
            tool_names.contains(&"ask_peer"),
            "external delegate appears on the tool surface: {tool_names:?}"
        );
        let log_before_delete = fs::read_to_string(&log).expect("fake ACP log");
        assert!(
            log_before_delete.contains(r#""method":"initialize""#)
                && log_before_delete.contains(r#""method":"session/new""#)
                && log_before_delete.contains(r#""method":"session/prompt""#),
            "fake ACP process was driven: {log_before_delete}"
        );

        engine
            .delete_session(session)
            .await
            .expect("delete session");
        let log_after_delete = fs::read_to_string(&log).expect("fake ACP log after cleanup");
        assert!(
            log_after_delete.contains(r#""method":"session/cancel""#)
                || log_after_delete.contains("SESSION_CANCELLED"),
            "session deletion sweeps the completed external session: {log_after_delete}"
        );
    }

    /// A crashing ACP subprocess does not block engine/session startup. The
    /// failed delegation is surfaced as `DelegationFailed`, and the supervisor
    /// can still continue with a normal final answer.
    #[cfg(unix)]
    #[tokio::test]
    async fn crashing_external_acp_delegate_maps_to_delegation_failed() {
        let dir = TempConfigDir::new();
        let script = fake_acp_script(&dir);
        let log = dir.0.join("fake-acp-crash.log");
        let config = external_acp_config(&script, &log, "crash_prompt");
        fs::write(dir.config_path(), config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(vec![
            tool_use_stream("ask_peer", "del-1", json!({ "task": "inspect" })),
            text_stream_with_usage(&["fallback answer"], usage()),
        ]);
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_config_service(client, registry(), service);
        let session = engine
            .create_session(session_config("default", "fake-chat"))
            .await
            .expect("create session");
        let mut events = engine.subscribe(Some(session));

        engine
            .send_message(session, UserInput::text("delegate externally"))
            .await
            .expect("send message");
        let collected = collect_until_terminal(&mut events).await;

        assert!(
            collected.iter().any(|event| {
                matches!(event, ServiceEvent::DelegationFailed { trace, .. } if trace.delegate == "peer")
            }),
            "the crashed process maps to DelegationFailed: {collected:?}"
        );
        assert!(
            !collected.iter().any(|event| {
                matches!(event, ServiceEvent::DelegationFinished { trace, .. } if trace.delegate == "peer")
            }),
            "a crashed external delegation never finishes: {collected:?}"
        );
        assert!(
            matches!(collected.last(), Some(ServiceEvent::RunFinished { output, .. }) if output.text == "fallback answer"),
            "the supervisor continues after a failed delegation: {collected:?}"
        );
    }

    /// Source listing/probing reflects configured ACP sources, including a
    /// lightweight executable check and configured capability labels.
    #[cfg(unix)]
    #[tokio::test]
    async fn list_sources_reports_external_acp_availability_and_capabilities() {
        let dir = TempConfigDir::new();
        let script = fake_acp_script(&dir);
        let log = dir.0.join("fake-acp-source.log");
        let missing = dir.0.join("missing-acp");
        let config = format!(
            r#"
[external_agents.peer]
kind = "acp"
command = ["{}"]
capabilities = ["streaming", "graceful_shutdown"]

[external_agents.peer.env]
MAG_FAKE_ACP_LOG = "{}"
MAG_FAKE_ACP_MODE = "success"

[external_agents.ghost]
kind = "acp"
command = ["{}"]
capabilities = ["streaming"]
"#,
            toml_string(&script),
            toml_string(&log),
            toml_string(&missing),
        );
        fs::write(dir.config_path(), config).expect("write config");
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
        let fake = FakeLlmClient::scripted(Vec::new());
        let client: Arc<dyn LlmClient> = fake;
        let engine = Engine::with_config_service(client, registry(), service);
        let mut events = engine.subscribe(None);

        let listed = engine.list_sources().await.expect("list sources");
        let peer = listed.iter().find(|source| source.id == "peer").unwrap();
        assert_eq!(peer.kind, SourceKindWire::LocalAgent);
        assert!(peer.available, "executable fake ACP source is available");
        assert_eq!(
            peer.path.as_deref(),
            Some(script.to_string_lossy().as_ref())
        );
        assert_eq!(peer.capabilities, vec!["streaming", "graceful_shutdown"]);
        let ghost = listed.iter().find(|source| source.id == "ghost").unwrap();
        assert!(!ghost.available, "missing binary is unavailable");

        let probed = engine
            .probe_local_agents()
            .await
            .expect("probe local agents");
        assert_eq!(probed.len(), 2);
        let event = next_event(&mut events).await;
        assert!(
            matches!(event, ServiceEvent::LocalAgentsProbed { ref available } if available == &probed),
            "probe emits the global LocalAgentsProbed event: {event:?}"
        );
    }
}
