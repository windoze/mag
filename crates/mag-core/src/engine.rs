//! Transport-neutral engine entry point and session skeleton.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use agent_lib::client::LlmClient;
use mag_service::{Command, Event, SessionConfig, SessionId};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{EventBus, EventStream, driver::SessionDriver};

/// Transport-neutral command engine.
///
/// The engine stores session records in memory, emits lifecycle events through
/// an [`EventBus`], and drives pure chat turns through agent-lib.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    /// Creates an empty engine with an in-memory session manager and event bus.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EngineInner::default()),
        }
    }

    /// Creates an engine that drives chat turns through `client`.
    #[must_use]
    pub fn with_llm_client(client: Arc<dyn LlmClient>) -> Self {
        Self {
            inner: Arc::new(EngineInner::with_llm_client(client)),
        }
    }

    /// Subscribes to future engine events.
    #[must_use]
    pub fn subscribe(&self) -> EventStream {
        self.inner.event_bus.subscribe()
    }

    /// Handles one command and returns any immediate command output.
    ///
    /// Lifecycle events are emitted through [`subscribe`](Engine::subscribe).
    /// Session-scoped commands that are not implemented in the current
    /// milestone emit a clear `RunError` instead of silently succeeding.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnsupportedCommand`] for unsupported commands that
    /// are not tied to a session and therefore cannot produce a `RunError`
    /// event.
    pub async fn handle_command(&self, command: Command) -> Result<CommandOutput, EngineError> {
        match command {
            Command::CreateSession { config } => {
                let session = {
                    let mut sessions = self.inner.sessions.lock().await;
                    let id = self.inner.session_ids.next_id();
                    sessions.create_session(id, config)
                };

                let _ = self.inner.event_bus.emit(Event::SessionCreated {
                    id: session.id,
                    config: session.config.clone(),
                });

                Ok(CommandOutput::SessionCreated(session))
            }
            Command::ListSessions => {
                let sessions = self.inner.sessions.lock().await.list_sessions();
                Ok(CommandOutput::Sessions(sessions))
            }
            Command::ResumeSession { id } => {
                self.emit_unimplemented(id, "resume_session");
                Ok(CommandOutput::None)
            }
            Command::DeleteSession { id } => {
                self.emit_unimplemented(id, "delete_session");
                Ok(CommandOutput::None)
            }
            Command::SendMessage {
                session_id, text, ..
            } => {
                self.send_message(session_id, text).await;
                Ok(CommandOutput::None)
            }
            Command::CancelRun { session_id } => {
                self.emit_unimplemented(session_id, "cancel_run");
                Ok(CommandOutput::None)
            }
            Command::RespondInteraction { session_id, .. } => {
                self.emit_unimplemented(session_id, "respond_interaction");
                Ok(CommandOutput::None)
            }
            Command::ListSources => Err(EngineError::UnsupportedCommand {
                command: "list_sources",
            }),
            Command::ProbeLocalAgents => Err(EngineError::UnsupportedCommand {
                command: "probe_local_agents",
            }),
            _ => Err(EngineError::UnsupportedCommand { command: "unknown" }),
        }
    }

    fn emit_unimplemented(&self, id: SessionId, command: &'static str) {
        let _ = self.inner.event_bus.emit(Event::RunError {
            id,
            message: format!("command `{command}` is not implemented yet"),
        });
    }

    async fn send_message(&self, session_id: SessionId, text: String) {
        let Some(client) = self.inner.llm_client.clone() else {
            self.emit_run_error(session_id, "no LLM client configured");
            return;
        };
        let Some((driver, config)) = self.session_entry(session_id).await else {
            self.emit_run_error(session_id, "session not found");
            return;
        };

        let mut guard = driver.lock().await;
        if guard.is_none() {
            match SessionDriver::new(&config, client) {
                Ok(driver) => *guard = Some(driver),
                Err(error) => {
                    self.emit_run_error(session_id, error.to_string());
                    return;
                }
            }
        }
        let driver = guard.as_mut().expect("session driver is built");

        let result = driver
            .send_message(session_id, text, self.inner.event_bus.clone())
            .await;

        if let Err(error) = result {
            self.emit_run_error(session_id, error.to_string());
        }
    }

    async fn session_entry(
        &self,
        session_id: SessionId,
    ) -> Option<(Arc<Mutex<Option<SessionDriver>>>, SessionConfig)> {
        self.inner.sessions.lock().await.session_entry(session_id)
    }

    fn emit_run_error(&self, id: SessionId, message: impl Into<String>) {
        let _ = self.inner.event_bus.emit(Event::RunError {
            id,
            message: message.into(),
        });
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

struct EngineInner {
    sessions: Mutex<SessionManager>,
    event_bus: EventBus,
    session_ids: SessionIdSource,
    llm_client: Option<Arc<dyn LlmClient>>,
}

impl Default for EngineInner {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(SessionManager::default()),
            event_bus: EventBus::new(),
            session_ids: SessionIdSource::new(),
            llm_client: None,
        }
    }
}

impl EngineInner {
    fn with_llm_client(client: Arc<dyn LlmClient>) -> Self {
        Self {
            llm_client: Some(client),
            ..Self::default()
        }
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Engine").finish_non_exhaustive()
    }
}

/// Immediate output returned from handling a command.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutput {
    /// The command completed without direct output.
    None,
    /// A new session was created.
    SessionCreated(SessionInfo),
    /// Current in-memory sessions were listed.
    Sessions(Vec<SessionInfo>),
}

/// In-memory session metadata exposed by `ListSessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Stable session identity.
    pub id: SessionId,
    /// Configuration used to create the session.
    pub config: SessionConfig,
}

/// Error returned while handling an engine command.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EngineError {
    /// The command has no skeleton implementation yet.
    UnsupportedCommand {
        /// Stable command tag.
        command: &'static str,
    },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCommand { command } => {
                write!(formatter, "command `{command}` is not implemented yet")
            }
        }
    }
}

impl Error for EngineError {}

#[derive(Default)]
struct SessionManager {
    sessions: BTreeMap<SessionId, SessionRecord>,
}

impl SessionManager {
    fn create_session(&mut self, id: SessionId, config: SessionConfig) -> SessionInfo {
        let session = SessionInfo {
            id,
            config: config.clone(),
        };
        self.sessions.insert(
            id,
            SessionRecord {
                info: session.clone(),
                driver: Arc::new(Mutex::new(None)),
            },
        );
        session
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|record| record.info.clone())
            .collect()
    }

    fn session_entry(
        &self,
        id: SessionId,
    ) -> Option<(Arc<Mutex<Option<SessionDriver>>>, SessionConfig)> {
        self.sessions
            .get(&id)
            .map(|record| (Arc::clone(&record.driver), record.info.config.clone()))
    }
}

struct SessionRecord {
    info: SessionInfo,
    driver: Arc<Mutex<Option<SessionDriver>>>,
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
    use mag_service::{Command, Event, RoutingMode, SessionConfig};
    use tokio::time::{Duration, timeout};
    use tokio_stream::StreamExt;

    use super::{CommandOutput, Engine, EventStream};

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            routing: RoutingMode::ModelRouted,
        }
    }

    async fn next_event(events: &mut EventStream) -> Event {
        timeout(Duration::from_secs(1), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    #[tokio::test]
    async fn create_session_emits_session_created() {
        let engine = Engine::new();
        let mut events = engine.subscribe();
        let config = config("model-a");

        let output = engine
            .handle_command(Command::CreateSession {
                config: config.clone(),
            })
            .await
            .expect("create session");
        let event = next_event(&mut events).await;

        let CommandOutput::SessionCreated(session) = output else {
            panic!("unexpected command output: {output:?}");
        };
        assert_eq!(session.config, config);
        assert_eq!(
            event,
            Event::SessionCreated {
                id: session.id,
                config
            }
        );
    }

    #[tokio::test]
    async fn list_sessions_reflects_created_sessions() {
        let engine = Engine::new();

        let first = engine
            .handle_command(Command::CreateSession {
                config: config("model-a"),
            })
            .await
            .expect("create first session");
        let second = engine
            .handle_command(Command::CreateSession {
                config: config("model-b"),
            })
            .await
            .expect("create second session");

        let CommandOutput::SessionCreated(first) = first else {
            panic!("unexpected first output: {first:?}");
        };
        let CommandOutput::SessionCreated(second) = second else {
            panic!("unexpected second output: {second:?}");
        };

        let listed = engine
            .handle_command(Command::ListSessions)
            .await
            .expect("list sessions");

        assert_eq!(listed, CommandOutput::Sessions(vec![first, second]));
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_the_same_event() {
        let engine = Engine::new();
        let mut first_subscriber = engine.subscribe();
        let mut second_subscriber = engine.subscribe();
        let config = config("model-a");

        let output = engine
            .handle_command(Command::CreateSession {
                config: config.clone(),
            })
            .await
            .expect("create session");

        let CommandOutput::SessionCreated(session) = output else {
            panic!("unexpected command output: {output:?}");
        };
        let expected = Event::SessionCreated {
            id: session.id,
            config,
        };

        assert_eq!(next_event(&mut first_subscriber).await, expected);
        assert_eq!(next_event(&mut second_subscriber).await, expected);
    }

    #[tokio::test]
    async fn session_scoped_unimplemented_commands_emit_run_error() {
        let engine = Engine::new();
        let mut events = engine.subscribe();

        let created = engine
            .handle_command(Command::CreateSession {
                config: config("model-a"),
            })
            .await
            .expect("create session");
        let CommandOutput::SessionCreated(session) = created else {
            panic!("unexpected command output: {created:?}");
        };
        let _ = next_event(&mut events).await;

        let output = engine
            .handle_command(Command::CancelRun {
                session_id: session.id,
            })
            .await
            .expect("cancel skeleton command");
        assert_eq!(output, CommandOutput::None);

        let event = next_event(&mut events).await;
        assert_eq!(
            event,
            Event::RunError {
                id: session.id,
                message: "command `cancel_run` is not implemented yet".to_owned(),
            }
        );
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
    use mag_service::{Command, Event, RoutingMode, SessionConfig, UsageInfo};
    use tokio::time::{Duration, timeout};
    use tokio_stream::StreamExt;

    use crate::{
        EventStream,
        test_support::{FakeLlmClient, text_stream_with_usage},
    };

    use super::{CommandOutput, Engine};

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            routing: RoutingMode::ModelRouted,
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

    async fn next_event(events: &mut EventStream) -> Event {
        timeout(Duration::from_secs(1), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    async fn create_session(engine: &Engine, events: &mut EventStream) -> super::SessionInfo {
        let created = engine
            .handle_command(Command::CreateSession {
                config: config("fake-chat"),
            })
            .await
            .expect("create session");
        let CommandOutput::SessionCreated(session) = created else {
            panic!("unexpected create output: {created:?}");
        };
        assert_eq!(
            next_event(events).await,
            Event::SessionCreated {
                id: session.id,
                config: session.config.clone(),
            }
        );
        session
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
        let mut events = engine.subscribe();
        let session = create_session(&engine, &mut events).await;

        let output = engine
            .handle_command(Command::SendMessage {
                session_id: session.id,
                text: "hi".to_owned(),
                attachments: Vec::new(),
            })
            .await
            .expect("send message");
        assert_eq!(output, CommandOutput::None);

        let Event::RunStarted { id, run_id } = next_event(&mut events).await else {
            panic!("expected run_started");
        };
        assert_eq!(id, session.id);
        assert_ne!(run_id.into_uuid(), uuid::Uuid::nil());

        assert_eq!(
            next_event(&mut events).await,
            Event::TextDelta {
                id: session.id,
                text: "hel".to_owned(),
            }
        );
        assert_eq!(
            next_event(&mut events).await,
            Event::TextDelta {
                id: session.id,
                text: "lo".to_owned(),
            }
        );

        let Event::RunFinished { id, output } = next_event(&mut events).await else {
            panic!("expected run_finished");
        };
        assert_eq!(id, session.id);
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
    async fn send_message_accumulates_history_in_one_session() {
        let fake = FakeLlmClient::scripted(vec![
            text_stream_with_usage(&["first"], usage(3, 1)),
            text_stream_with_usage(&["second"], usage(5, 2)),
        ]);
        let engine = engine_with_fake(fake.clone());
        let mut events = engine.subscribe();
        let session = create_session(&engine, &mut events).await;

        engine
            .handle_command(Command::SendMessage {
                session_id: session.id,
                text: "hi".to_owned(),
                attachments: Vec::new(),
            })
            .await
            .expect("first send");
        for _ in 0..3 {
            let _ = next_event(&mut events).await;
        }

        engine
            .handle_command(Command::SendMessage {
                session_id: session.id,
                text: "again".to_owned(),
                attachments: Vec::new(),
            })
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
