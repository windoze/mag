//! Transport-neutral engine entry point implementing [`MagService`].

use std::{
    collections::BTreeMap,
    fmt,
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
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{EventBus, session::SessionManager};

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
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EngineInner::new(None)),
        }
    }

    /// Creates an engine that drives chat turns through `client`.
    #[must_use]
    pub fn with_llm_client(client: Arc<dyn LlmClient>) -> Self {
        Self {
            inner: Arc::new(EngineInner::new(Some(client))),
        }
    }
}

#[async_trait]
impl MagService for Engine {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError> {
        let id = {
            let mut sessions = self.inner.sessions.lock().await;
            let id = self.inner.session_ids.next_id();
            sessions.insert(id, config.clone());
            id
        };

        self.inner.manager.create_session(id, config.clone());

        let _ = self
            .inner
            .event_bus
            .emit(mag_service::Event::SessionCreated { id, config });

        Ok(id)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        let sessions = self.inner.sessions.lock().await;
        Ok(sessions
            .iter()
            .map(|(id, config)| SessionInfo {
                id: *id,
                config: config.clone(),
            })
            .collect())
    }

    async fn resume_session(&self, _id: SessionId) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "resume_session".to_owned(),
        })
    }

    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError> {
        {
            let mut sessions = self.inner.sessions.lock().await;
            if sessions.remove(&id).is_none() {
                return Err(ServiceError::SessionNotFound { id });
            }
        }
        self.inner.manager.delete_session(id);
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
    manager: SessionManager,
}

impl EngineInner {
    fn new(client: Option<Arc<dyn LlmClient>>) -> Self {
        let event_bus = EventBus::new();
        let manager = SessionManager::new(client, event_bus.clone());
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            event_bus,
            session_ids: SessionIdSource::new(),
            manager,
        }
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
            routing: RoutingMode::ModelRouted,
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
        let id = engine
            .create_session(config("model-a"))
            .await
            .expect("create session");

        assert_eq!(
            engine.resume_session(id).await,
            Err(ServiceError::Unsupported {
                operation: "resume_session".to_owned(),
            })
        );
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
                ServiceEvent::RunError { id, message } if *id == s && message == "run cancelled"
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
