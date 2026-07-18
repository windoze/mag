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

use mag_protocol::{Command, Event, SessionConfig, SessionId};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{EventBus, EventStream};

/// Transport-neutral command engine.
///
/// The skeleton stores session records in memory, emits lifecycle events through
/// an [`EventBus`], and leaves agent-lib driver execution for later milestones.
#[derive(Clone, Debug)]
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

    /// Subscribes to future engine events.
    #[must_use]
    pub fn subscribe(&self) -> EventStream {
        self.inner.event_bus.subscribe()
    }

    /// Handles one command and returns any immediate command output.
    ///
    /// Lifecycle events are emitted through [`subscribe`](Engine::subscribe).
    /// Commands that need the future agent driver currently emit or return a
    /// clear "not implemented" error instead of silently succeeding.
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
            Command::SendMessage { session_id, .. } => {
                self.emit_unimplemented(session_id, "send_message");
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
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct EngineInner {
    sessions: Mutex<SessionManager>,
    event_bus: EventBus,
    session_ids: SessionIdSource,
}

impl Default for EngineInner {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(SessionManager::default()),
            event_bus: EventBus::new(),
            session_ids: SessionIdSource::new(),
        }
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

#[derive(Debug, Default)]
struct SessionManager {
    sessions: BTreeMap<SessionId, SessionInfo>,
}

impl SessionManager {
    fn create_session(&mut self, id: SessionId, config: SessionConfig) -> SessionInfo {
        let session = SessionInfo { id, config };
        self.sessions.insert(id, session.clone());
        session
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.values().cloned().collect()
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
    use mag_protocol::{Command, Event, RoutingMode, SessionConfig};
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
