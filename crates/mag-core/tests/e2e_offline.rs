//! End-to-end, fully offline backbone integration test for `mag-core` (C5-1).
//!
//! This test drives the whole C1–C4 chain **only through the public
//! [`MagService`] surface** (`Arc<dyn MagService>`), never touching any `Engine`
//! internals. It is the stability evidence for the service milestone and the
//! gate before the interface (ACP) layer (`docs/DESIGN.md` §3.0/§10).
//!
//! Everything is offline: a scripted fake [`LlmClient`] replays canned stream
//! events, stub tool plugins provide an auto-allowed `read_file` and a gated
//! `shell`, persistence is a temporary file-backed SQLite database, and approval
//! round-trips happen in-process through `respond_interaction`. There is no
//! network, real credential, CLI, or real filesystem access, and every test
//! finishes well within a second.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use agent_lib::{
    client::{
        ANTHROPIC_DEFAULT_CAPABILITY, Capability, ChatRequest, ClientError, LlmClient, Response,
    },
    facade::{ToolContext, ToolResult},
    model::{
        message::Role,
        normalized::{Normalized, StopReason},
        tool::Tool,
        usage::Usage,
    },
    stream::{
        BlockId, BlockKind, Delta, StreamEvent,
        accumulator::{AccumulatorError, CollectError, collect},
    },
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use mag_core::Engine;
use mag_service::{
    ApprovalDecisionWire, InteractionKindWire, InteractionResponseWire, MagService, ServiceEvent,
    SessionId, StepIdWire, ToolCallIdWire, UserInput,
};
use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
use serde_json::{Value, json};
use tokio::time::{Duration, timeout};
use uuid::Uuid;

// —— Offline fake LLM client ————————————————————————————————————————————————

/// One scripted response the [`FakeLlm`] hands back for a chat turn.
///
/// [`Script::Complete`] yields its events and ends the stream; [`Script::Stall`]
/// yields its events and then pends forever, modelling a run still in flight so a
/// cancellation can land mid-stream.
#[derive(Clone, Debug)]
enum Script {
    Complete(Vec<StreamEvent>),
    Stall(Vec<StreamEvent>),
}

/// Scripted, offline [`LlmClient`].
///
/// Turns are answered either from a per-model queue (keyed by `ChatRequest.model`)
/// or, when a model has no remaining scripted turns, from a shared FIFO queue.
/// Per-model routing makes concurrent multi-session turns deterministic
/// regardless of the order the sessions happen to pull scripts.
#[derive(Debug)]
struct FakeLlm {
    fifo: Mutex<VecDeque<Script>>,
    by_model: Mutex<HashMap<String, VecDeque<Script>>>,
    stream_requests: Mutex<Vec<ChatRequest>>,
}

impl FakeLlm {
    /// Builds a client that answers every turn from one shared FIFO queue.
    fn fifo(scripts: Vec<Script>) -> Arc<Self> {
        Arc::new(Self {
            fifo: Mutex::new(scripts.into()),
            by_model: Mutex::new(HashMap::new()),
            stream_requests: Mutex::new(Vec::new()),
        })
    }

    /// Builds a client that routes turns to a per-model queue.
    fn by_model(models: Vec<(&str, Vec<Script>)>) -> Arc<Self> {
        let map = models
            .into_iter()
            .map(|(model, scripts)| (model.to_owned(), scripts.into()))
            .collect();
        Arc::new(Self {
            fifo: Mutex::new(VecDeque::new()),
            by_model: Mutex::new(map),
            stream_requests: Mutex::new(Vec::new()),
        })
    }

    /// Returns the streaming requests observed so far.
    fn stream_requests(&self) -> Vec<ChatRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .clone()
    }

    fn next_script(&self, model: &str) -> Result<Script, ClientError> {
        if let Some(queue) = self.by_model.lock().expect("by-model lock").get_mut(model)
            && let Some(script) = queue.pop_front()
        {
            return Ok(script);
        }
        self.fifo
            .lock()
            .expect("fifo lock")
            .pop_front()
            .ok_or_else(|| ClientError::Other("fake LLM script exhausted".to_owned()))
    }
}

#[async_trait]
impl LlmClient for FakeLlm {
    fn capability(&self) -> &Capability {
        &ANTHROPIC_DEFAULT_CAPABILITY
    }

    async fn chat(&self, request: ChatRequest) -> Result<Response, ClientError> {
        let model = request.model.clone();
        let events = match self.next_script(&model)? {
            Script::Complete(events) | Script::Stall(events) => events,
        };
        collect(stream::iter(events.into_iter().map(Ok::<_, ClientError>)))
            .await
            .map_err(collect_error_to_client_error)
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ClientError>>, ClientError> {
        let model = request.model.clone();
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .push(request);
        match self.next_script(&model)? {
            Script::Complete(events) => {
                Ok(stream::iter(events.into_iter().map(Ok::<_, ClientError>)).boxed())
            }
            Script::Stall(events) => Ok(stream::iter(events.into_iter().map(Ok))
                .chain(stream::pending::<Result<StreamEvent, ClientError>>())
                .boxed()),
        }
    }
}

fn collect_error_to_client_error(error: CollectError<ClientError>) -> ClientError {
    match error {
        CollectError::Stream(error) => error,
        CollectError::Accumulator(AccumulatorError::Stream(error)) => error,
        CollectError::Accumulator(other) => ClientError::Protocol(other.to_string()),
    }
}

// —— Scripted stream builders ——————————————————————————————————————————————

fn usage(input: u32, output: u32) -> Usage {
    Usage {
        input,
        output,
        total: Some(input + output),
        ..Usage::default()
    }
}

/// Builds a complete text stream with explicit usage accounting.
fn text_events(chunks: &[&str], usage: Usage) -> Vec<StreamEvent> {
    let block_id = BlockId::new("text-1");
    let mut events = vec![
        StreamEvent::MessageStart {
            role: Role::Assistant,
        },
        StreamEvent::BlockStart {
            id: block_id.clone(),
            kind: BlockKind::Text,
        },
    ];
    events.extend(chunks.iter().map(|chunk| StreamEvent::BlockDelta {
        id: block_id.clone(),
        delta: Delta::Text((*chunk).to_owned()),
    }));
    events.push(StreamEvent::BlockStop {
        id: block_id.clone(),
    });
    events.push(StreamEvent::Usage(usage));
    events.push(StreamEvent::MessageStop {
        stop_reason: Normalized::from_mapped(StopReason::EndTurn, "end_turn"),
    });
    events
}

/// A completed text turn.
fn text_turn(chunks: &[&str], usage: Usage) -> Script {
    Script::Complete(text_events(chunks, usage))
}

/// A completed single tool-call turn.
fn tool_turn(tool_name: &str, tool_call_id: &str, input: Value) -> Script {
    let block_id = BlockId::new("tool-1");
    Script::Complete(vec![
        StreamEvent::MessageStart {
            role: Role::Assistant,
        },
        StreamEvent::BlockStart {
            id: block_id.clone(),
            kind: BlockKind::ToolInput {
                tool_name: tool_name.to_owned(),
                tool_call_id: tool_call_id.to_owned(),
            },
        },
        StreamEvent::BlockDelta {
            id: block_id.clone(),
            delta: Delta::Json(input.to_string()),
        },
        StreamEvent::BlockStop { id: block_id },
        StreamEvent::MessageStop {
            stop_reason: Normalized::from_mapped(StopReason::ToolUse, "tool_use"),
        },
    ])
}

/// A turn that streams `chunks` as live deltas and then pends forever, leaving the
/// run in flight so a cancellation can land mid-stream.
fn stalling_turn(chunks: &[&str]) -> Script {
    let block_id = BlockId::new("text-1");
    let mut events = vec![
        StreamEvent::MessageStart {
            role: Role::Assistant,
        },
        StreamEvent::BlockStart {
            id: block_id.clone(),
            kind: BlockKind::Text,
        },
    ];
    events.extend(chunks.iter().map(|chunk| StreamEvent::BlockDelta {
        id: block_id.clone(),
        delta: Delta::Text((*chunk).to_owned()),
    }));
    Script::Stall(events)
}

// —— Stub tool plugins ——————————————————————————————————————————————————————

/// A canned tool plugin: it ignores its arguments and returns fixed text, so a
/// turn's tool events are deterministic and fully offline.
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
fn tool_registry() -> ToolRegistry {
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

// —— Temp SQLite database ————————————————————————————————————————————————————

/// A unique temporary database path whose files are removed on drop.
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
            "mag-e2e-{}-{nanos}-{unique}.sqlite",
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

/// A unique temporary directory holding a `config.toml`, removed on drop.
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
        path.push(format!("mag-e2e-cfg-{}-{nanos}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp config dir");
        Self(path)
    }

    fn config_path(&self) -> PathBuf {
        self.0.join("config.toml")
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// —— Shared helpers ——————————————————————————————————————————————————————————

/// Builds an approval response; `step_id`/`call_id` are reconstructed from the
/// stored interaction, so nil placeholders are fine (`docs/DESIGN.md` §3.3).
fn approval(decision: ApprovalDecisionWire) -> InteractionResponseWire {
    InteractionResponseWire::Approval {
        step_id: StepIdWire::new(Uuid::nil()),
        call_id: ToolCallIdWire::new(Uuid::nil()),
        decision,
        message: None,
    }
}

async fn next_event(events: &mut BoxStream<'static, ServiceEvent>) -> ServiceEvent {
    timeout(Duration::from_secs(2), events.next())
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

/// Reads events until an `InteractionRequested` arrives, returning the events
/// seen before it (inclusive of it) and the pending request id.
async fn drain_to_interaction(
    events: &mut BoxStream<'static, ServiceEvent>,
    session: SessionId,
) -> (Vec<ServiceEvent>, mag_service::RequestId) {
    let mut collected = Vec::new();
    loop {
        let event = next_event(events).await;
        if let ServiceEvent::InteractionRequested {
            id,
            request_id,
            kind,
            ..
        } = &event
        {
            assert_eq!(*id, session, "interaction leaked to another session");
            assert!(
                matches!(kind, InteractionKindWire::Approval { .. }),
                "expected an approval interaction, got {kind:?}",
            );
            let request_id = *request_id;
            collected.push(event);
            return (collected, request_id);
        }
        collected.push(event);
    }
}

fn last_text(events: &[ServiceEvent]) -> String {
    match events.last().expect("terminal event") {
        ServiceEvent::RunFinished { output, .. } => output.text.clone(),
        other => panic!("expected run_finished, got {other:?}"),
    }
}

fn has_tool_event(events: &[ServiceEvent], name: &str, started: bool) -> bool {
    events.iter().any(|event| match (event, started) {
        (ServiceEvent::ToolStarted { trace, .. }, true) => trace.name == name,
        (ServiceEvent::ToolFinished { trace, .. }, false) => trace.name == name,
        _ => false,
    })
}

// —— Tests ——————————————————————————————————————————————————————————————————

/// The full offline backbone, driven end-to-end through `Arc<dyn MagService>`:
/// create session → streamed conversation → auto read tool → approved shell tool
/// → denied shell tool → cancel mid-run → restart into a fresh engine over the
/// same store → resume and continue with the prior context intact.
#[tokio::test]
async fn full_offline_backbone_through_service() {
    let db = TempDb::new();

    // First "process": scripts for every step up to (and including) the cancel.
    let fake1 = FakeLlm::fifo(vec![
        text_turn(&["hi ", "there"], usage(4, 2)), // A: plain turn
        tool_turn("read_file", "call-r", json!({ "path": "R" })), // B: auto tool call
        text_turn(&["file ", "summary"], usage(5, 2)), // B: follow-up
        tool_turn("shell", "call-s1", json!({ "command": "ls" })), // C: gated tool call
        text_turn(&["listed"], usage(6, 1)),       // C: follow-up
        tool_turn("shell", "call-s2", json!({ "command": "rm" })), // D: gated tool call
        text_turn(&["understood"], usage(3, 1)),   // D: follow-up
        stalling_turn(&["working"]),               // E: stalls for cancel
    ]);
    let client1: Arc<dyn LlmClient> = fake1;
    let engine1 =
        Engine::with_persistence(client1, tool_registry(), &db.path).expect("open first engine");
    let service1: Arc<dyn MagService> = Arc::new(engine1);

    // Step 0: create a session and observe the SessionCreated event.
    let mut global = service1.subscribe(None);
    let session = service1
        .create_session(None, None)
        .await
        .expect("create session");
    assert!(
        matches!(
            next_event(&mut global).await,
            ServiceEvent::SessionCreated { id, .. } if id == session
        ),
        "create_session must announce SessionCreated",
    );
    drop(global);

    let mut events = service1.subscribe(Some(session));

    // Step A: a plain streamed conversation turn.
    let run_a = service1
        .send_message(session, UserInput::text("hello"))
        .await
        .expect("send hello");
    assert!(
        matches!(
            next_event(&mut events).await,
            ServiceEvent::RunStarted { id, run_id } if id == session && run_id == run_a
        ),
        "turn A must start",
    );
    let a = drain_run(&mut events).await;
    assert!(
        a.iter()
            .any(|event| matches!(event, ServiceEvent::TextDelta { id, .. } if *id == session)),
        "turn A must stream text: {a:?}",
    );
    assert_eq!(last_text(&a), "hi there", "turn A final text: {a:?}");

    // Step B: an auto-allowed read tool runs without any interaction.
    service1
        .send_message(session, UserInput::text("read the file"))
        .await
        .expect("send read");
    assert!(matches!(
        next_event(&mut events).await,
        ServiceEvent::RunStarted { .. }
    ));
    let b = drain_run(&mut events).await;
    assert!(
        !b.iter()
            .any(|event| matches!(event, ServiceEvent::InteractionRequested { .. })),
        "an auto-allowed tool must not pause for approval: {b:?}",
    );
    assert!(
        has_tool_event(&b, "read_file", true) && has_tool_event(&b, "read_file", false),
        "the auto tool must emit ToolStarted+ToolFinished: {b:?}",
    );
    assert_eq!(last_text(&b), "file summary", "turn B final text: {b:?}");

    // Step C: a gated shell tool pauses for approval, then runs once approved.
    service1
        .send_message(session, UserInput::text("run ls"))
        .await
        .expect("send run ls");
    assert!(matches!(
        next_event(&mut events).await,
        ServiceEvent::RunStarted { .. }
    ));
    let (before_c, req_c) = drain_to_interaction(&mut events, session).await;
    assert!(
        !has_tool_event(&before_c, "shell", true),
        "a gated tool must not execute before approval: {before_c:?}",
    );
    service1
        .respond_interaction(session, req_c, approval(ApprovalDecisionWire::Approve))
        .await
        .expect("approve shell");
    let c = drain_run(&mut events).await;
    assert!(
        has_tool_event(&c, "shell", true) && has_tool_event(&c, "shell", false),
        "an approved gated tool must emit ToolStarted+ToolFinished: {c:?}",
    );
    assert_eq!(last_text(&c), "listed", "turn C final text: {c:?}");

    // Step D: the same gated tool is denied, so it never executes but the run
    // still finishes with the model's follow-up.
    service1
        .send_message(session, UserInput::text("delete everything"))
        .await
        .expect("send delete");
    assert!(matches!(
        next_event(&mut events).await,
        ServiceEvent::RunStarted { .. }
    ));
    let (_before_d, req_d) = drain_to_interaction(&mut events, session).await;
    service1
        .respond_interaction(session, req_d, approval(ApprovalDecisionWire::Deny))
        .await
        .expect("deny shell");
    let d = drain_run(&mut events).await;
    assert!(
        !d.iter().any(|event| matches!(
            event,
            ServiceEvent::ToolStarted { .. } | ServiceEvent::ToolFinished { .. }
        )),
        "a denied tool must never execute: {d:?}",
    );
    assert_eq!(last_text(&d), "understood", "turn D final text: {d:?}");

    // Step E: a run that stalls mid-stream is cancelled; it terminates with a
    // cancellation error and is never committed to the store.
    service1
        .send_message(session, UserInput::text("long task"))
        .await
        .expect("send long task");
    assert!(matches!(
        next_event(&mut events).await,
        ServiceEvent::RunStarted { .. }
    ));
    assert_eq!(
        next_event(&mut events).await,
        ServiceEvent::TextDelta {
            id: session,
            text: "working".to_owned(),
        },
        "the stalling run must stream before it is cancelled",
    );
    service1.cancel(session).await.expect("cancel run");
    let cancelled = next_event(&mut events).await;
    assert!(
        matches!(
            &cancelled,
            ServiceEvent::RunError { id, message, kind } if *id == session && message == "run cancelled" && *kind == mag_service::RunErrorKind::Cancelled
        ),
        "cancel must surface a RunError: {cancelled:?}",
    );

    // "Restart": drop the first engine (joins its session threads); the latest
    // committed snapshot (through step D) is already durable. The cancelled step E
    // was never committed.
    drop(events);
    drop(service1);

    // Second "process": a fresh engine over the same database resumes the session.
    let fake2 = FakeLlm::fifo(vec![text_turn(&["resumed ", "reply"], usage(9, 2))]);
    let fake2_probe = fake2.clone();
    let client2: Arc<dyn LlmClient> = fake2;
    let engine2 =
        Engine::with_persistence(client2, tool_registry(), &db.path).expect("open second engine");
    let service2: Arc<dyn MagService> = Arc::new(engine2);

    // The persisted session is visible before it is resumed.
    let listed = service2.list_sessions().await.expect("list sessions");
    assert!(
        listed.iter().any(|info| info.id == session),
        "the persisted session must be listed after restart: {listed:?}",
    );

    service2
        .resume_session(session)
        .await
        .expect("resume session");
    let mut events2 = service2.subscribe(Some(session));

    service2
        .send_message(session, UserInput::text("continue please"))
        .await
        .expect("send continue");
    assert!(matches!(
        next_event(&mut events2).await,
        ServiceEvent::RunStarted { id, .. } if id == session
    ));
    let f = drain_run(&mut events2).await;
    assert_eq!(
        last_text(&f),
        "resumed reply",
        "resumed turn final text: {f:?}"
    );

    // The resumed turn carries the pre-restart committed history (steps A–D) but
    // not the cancelled step E, proving the restored agent continued the exact
    // snapshotted conversation.
    let requests = fake2_probe.stream_requests();
    assert_eq!(
        requests.len(),
        1,
        "the resumed engine drove exactly one turn"
    );
    let flat = format!("{:?}", requests[0].messages);
    for fragment in [
        "hello",
        "read the file",
        "run ls",
        "delete everything",
        "understood",
    ] {
        assert!(
            flat.contains(fragment),
            "restored context is missing committed fragment `{fragment}`: {flat}",
        );
    }
    assert!(
        !flat.contains("long task"),
        "the cancelled turn must not be in the restored history: {flat}",
    );
}

/// Two sessions run concurrently on one engine; each session-filtered
/// subscription observes only its own session's events, proving `subscribe(Some)`
/// isolation across the shared event bus.
#[tokio::test]
async fn concurrent_sessions_are_isolated_by_subscription() {
    // Each session binds a distinct agent template (`a` / `b`) whose model keys
    // the fake client's per-model queue, so each session's reply is deterministic
    // regardless of the order the two concurrent actors pull scripts.
    let fake = FakeLlm::by_model(vec![
        ("fake-a", vec![text_turn(&["alpha"], usage(1, 1))]),
        ("fake-b", vec![text_turn(&["beta"], usage(1, 1))]),
    ]);
    let client: Arc<dyn LlmClient> = fake;

    let dir = TempConfigDir::new();
    std::fs::write(
        dir.config_path(),
        r#"
[agents.a]
model = "fake-a"

[agents.b]
model = "fake-b"
"#,
    )
    .expect("write config");
    let config =
        Arc::new(mag_core::ConfigService::load_or_default(dir.config_path()).expect("load config"));
    let service: Arc<dyn MagService> = Arc::new(Engine::with_config_service(
        client,
        ToolRegistry::new(),
        config,
    ));

    let a = service
        .create_session(None, Some("a".to_owned()))
        .await
        .expect("create session a");
    let b = service
        .create_session(None, Some("b".to_owned()))
        .await
        .expect("create session b");
    let mut events_a = service.subscribe(Some(a));
    let mut events_b = service.subscribe(Some(b));

    // Start both runs before draining either, so the two per-session actors are in
    // flight concurrently.
    let run_a = service
        .send_message(a, UserInput::text("hi a"))
        .await
        .expect("send to a");
    let run_b = service
        .send_message(b, UserInput::text("hi b"))
        .await
        .expect("send to b");

    let a_events = drain_run(&mut events_a).await;
    let b_events = drain_run(&mut events_b).await;

    // Every event on a session-filtered subscription is scoped to that session.
    for event in &a_events {
        assert_eq!(
            event.session_id(),
            Some(a),
            "session A subscription leaked a foreign event: {event:?}",
        );
    }
    for event in &b_events {
        assert_eq!(
            event.session_id(),
            Some(b),
            "session B subscription leaked a foreign event: {event:?}",
        );
    }

    // Each subscription sees exactly one run lifecycle, with its own text.
    assert_eq!(
        a_events
            .iter()
            .filter(|event| matches!(event, ServiceEvent::RunStarted { .. }))
            .count(),
        1,
        "session A must see exactly one RunStarted: {a_events:?}",
    );
    assert!(
        matches!(&a_events[0], ServiceEvent::RunStarted { run_id, .. } if *run_id == run_a),
        "session A's first event is its own RunStarted: {a_events:?}",
    );
    assert_eq!(
        last_text(&a_events),
        "alpha",
        "session A text: {a_events:?}"
    );

    assert_eq!(
        b_events
            .iter()
            .filter(|event| matches!(event, ServiceEvent::RunStarted { .. }))
            .count(),
        1,
        "session B must see exactly one RunStarted: {b_events:?}",
    );
    assert!(
        matches!(&b_events[0], ServiceEvent::RunStarted { run_id, .. } if *run_id == run_b),
        "session B's first event is its own RunStarted: {b_events:?}",
    );
    assert_eq!(last_text(&b_events), "beta", "session B text: {b_events:?}");
}
