//! End-to-end tests for the assembled Engine + terminal CLI path (`TODO.md`
//! M6-5).
//!
//! These tests stay fully offline: a local scripted [`LlmClient`] drives the real
//! [`mag_core::Engine`], `mag-cli` is driven through in-memory pipes, and the
//! external ACP path uses a tiny local shell process.

use std::{
    collections::VecDeque,
    fs,
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
        accumulator::{CollectError, collect},
    },
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use mag_cli::{Cli, CliOptions};
use mag_core::{ConfigService, Engine};
use mag_service::{MagService, RoutingMode, SessionConfig, SessionId};
use mag_tools::{ToolPlugin, ToolRegistry};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
    time::{Duration, timeout},
};

/// Unique temp directory per test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    /// Creates a temp directory with a human-readable tag.
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mag-engine-cli-{tag}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    /// Returns the config file path inside this temp directory.
    fn config_path(&self) -> PathBuf {
        self.0.join("config.toml")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One scripted response stream consumed by [`FakeLlmClient`].
enum StreamScript {
    /// Yield all events and terminate normally.
    Complete(Vec<StreamEvent>),
    /// Yield the events and then pend forever until the run is cancelled.
    Stall(Vec<StreamEvent>),
}

impl StreamScript {
    /// Flattens the script for the non-streaming `chat` endpoint.
    fn events(self) -> Vec<StreamEvent> {
        match self {
            Self::Complete(events) | Self::Stall(events) => events,
        }
    }
}

/// Offline scripted LLM client used by the Engine e2e tests.
struct FakeLlmClient {
    scripts: Mutex<VecDeque<StreamScript>>,
    chat_requests: Mutex<Vec<ChatRequest>>,
    stream_requests: Mutex<Vec<ChatRequest>>,
}

impl FakeLlmClient {
    /// Creates a fake client from explicit stream scripts.
    fn scripted(scripts: Vec<StreamScript>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            chat_requests: Mutex::new(Vec::new()),
            stream_requests: Mutex::new(Vec::new()),
        })
    }

    /// Returns the requests made through the streaming supervisor endpoint.
    fn stream_requests(&self) -> Vec<ChatRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .clone()
    }

    /// Pops the next scripted response.
    fn pop_script(&self) -> Result<StreamScript, ClientError> {
        self.scripts
            .lock()
            .expect("fake LLM script lock")
            .pop_front()
            .ok_or_else(|| ClientError::Other("fake LLM script exhausted".to_owned()))
    }
}

#[async_trait]
impl LlmClient for FakeLlmClient {
    fn capability(&self) -> &Capability {
        &ANTHROPIC_DEFAULT_CAPABILITY
    }

    async fn chat(&self, request: ChatRequest) -> Result<Response, ClientError> {
        self.chat_requests
            .lock()
            .expect("chat requests lock")
            .push(request);
        collect(stream::iter(
            self.pop_script()?
                .events()
                .into_iter()
                .map(Ok::<_, ClientError>),
        ))
        .await
        .map_err(collect_error_to_client_error)
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent, ClientError>>, ClientError>
    {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .push(request);
        match self.pop_script()? {
            StreamScript::Complete(events) => {
                Ok(stream::iter(events.into_iter().map(Ok::<_, ClientError>)).boxed())
            }
            StreamScript::Stall(events) => Ok(stream::iter(events.into_iter().map(Ok))
                .chain(stream::pending::<Result<StreamEvent, ClientError>>())
                .boxed()),
        }
    }
}

/// Converts stream collection failures into client errors.
fn collect_error_to_client_error(error: CollectError<ClientError>) -> ClientError {
    match error {
        CollectError::Stream(error) => error,
        CollectError::Accumulator(error) => match error {
            agent_lib::stream::accumulator::AccumulatorError::Stream(error) => error,
            other => ClientError::Protocol(other.to_string()),
        },
    }
}

/// Gate used by the pivot test to park a tool call at a deterministic boundary.
#[derive(Debug)]
struct Gate {
    permits: tokio::sync::Semaphore,
}

impl Gate {
    /// Creates a closed gate.
    fn new() -> Arc<Self> {
        Arc::new(Self {
            permits: tokio::sync::Semaphore::new(0),
        })
    }

    /// Opens the gate once.
    fn open(&self) {
        self.permits.add_permits(1);
    }

    /// Waits until the gate opens.
    async fn wait(&self) {
        let _ = self.permits.acquire().await;
    }
}

/// Tool plugin that blocks until its test gate is opened.
#[derive(Debug)]
struct GateTool {
    gate: Arc<Gate>,
}

#[async_trait]
impl ToolPlugin for GateTool {
    fn name(&self) -> &str {
        "gate"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: "gate".to_owned(),
            description: "waits for the test gate".to_owned(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
        self.gate.wait().await;
        ToolResult::text("gate opened")
    }
}

/// Builds a complete text stream.
fn text_stream(chunks: &[&str]) -> StreamScript {
    StreamScript::Complete(text_events(chunks))
}

/// Builds a complete text stream event list.
fn text_events(chunks: &[&str]) -> Vec<StreamEvent> {
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
    events.push(StreamEvent::Usage(Usage {
        input: 1,
        output: 1,
        total: Some(2),
        ..Usage::default()
    }));
    events.push(StreamEvent::MessageStop {
        stop_reason: Normalized::from_mapped(StopReason::EndTurn, "end_turn"),
    });
    events
}

/// Builds a complete tool-use stream.
fn tool_use_stream(tool_name: &str, tool_call_id: &str, input: Value) -> StreamScript {
    let block_id = BlockId::new(format!("tool-{tool_call_id}"));
    StreamScript::Complete(vec![
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

/// Builds a stalling text stream used to test CLI cancellation.
fn stalling_text_stream(chunks: &[&str]) -> StreamScript {
    let block_id = BlockId::new("stall-text");
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
    StreamScript::Stall(events)
}

/// Creates a config-backed engine using a scripted fake client and tool registry.
fn engine_with_config(
    dir: &TempDir,
    toml: &str,
    fake: Arc<FakeLlmClient>,
    tools: ToolRegistry,
) -> Engine {
    fs::write(dir.config_path(), toml).expect("write config");
    let config = Arc::new(ConfigService::load_or_default(dir.config_path()).expect("load config"));
    let client: Arc<dyn LlmClient> = fake;
    Engine::with_config_service(client, tools, config)
}

/// CLI options used by the in-process tests.
fn cli_options() -> CliOptions {
    CliOptions {
        session: SessionConfig {
            provider: "default".to_owned(),
            model: "fake-chat".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        },
        resume: None,
        prompt: "mag> ".to_owned(),
    }
}

/// Spawns the real CLI with in-memory stdin/stdout.
fn spawn_cli(
    engine: Engine,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    JoinHandle<Result<(), mag_cli::CliError>>,
) {
    spawn_cli_with_options(engine, cli_options())
}

/// Spawns the real CLI with caller-supplied options.
fn spawn_cli_with_options(
    engine: Engine,
    options: CliOptions,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    JoinHandle<Result<(), mag_cli::CliError>>,
) {
    let (stdin_writer, stdin_reader) = tokio::io::duplex(2048);
    let (stdout_writer, stdout_reader) = tokio::io::duplex(16 * 1024);
    let service: Arc<dyn MagService> = Arc::new(engine);
    let run = tokio::spawn(async move {
        Cli::run_with_io(service, options, stdin_reader, stdout_writer).await
    });
    (stdin_writer, stdout_reader, run)
}

/// Reads CLI stdout until `needle` appears in the cumulative output.
async fn read_until<R>(reader: &mut R, output: &mut String, needle: &str)
where
    R: AsyncRead + Unpin,
{
    read_until_count(reader, output, needle, 1).await;
}

/// Reads CLI stdout until `needle` appears at least `count` times.
async fn read_until_count<R>(reader: &mut R, output: &mut String, needle: &str, count: usize)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 512];
    while output.matches(needle).count() < count {
        let read = timeout(Duration::from_secs(10), reader.read(&mut buffer))
            .await
            .expect("CLI output timed out")
            .expect("read CLI output");
        assert!(
            read > 0,
            "CLI output ended before `{needle}` appeared {count} times; got {output:?}"
        );
        output.push_str(std::str::from_utf8(&buffer[..read]).expect("utf-8 CLI output"));
    }
}

/// Waits until the fake client has seen at least `count` streaming requests.
async fn wait_for_stream_requests(fake: &FakeLlmClient, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while fake.stream_requests().len() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {count} stream requests"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Extracts the current session id from CLI output.
fn first_session_id(output: &str) -> String {
    output
        .lines()
        .find_map(|line| {
            line.strip_prefix("[session ")
                .and_then(|rest| rest.strip_suffix(']'))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("missing session id in output: {output}"))
}

/// Finishes a spawned CLI run cleanly.
async fn finish_cli(
    mut stdin: tokio::io::DuplexStream,
    run: JoinHandle<Result<(), mag_cli::CliError>>,
) {
    stdin.write_all(b"/quit\n").await.expect("write /quit");
    stdin.shutdown().await.expect("close CLI stdin");
    timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run timed out")
        .expect("CLI task joined")
        .expect("CLI run succeeded");
}

/// Baseline config with the bound default entry and ask_user available.
const LOCAL_CONFIG: &str = r#"
[agents.default]
model = "model-main"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_cli_runs_dialog_ask_user_and_config_reload() {
    let dir = TempDir::new("dialog");
    let fake = FakeLlmClient::scripted(vec![
        text_stream(&["hello from engine"]),
        tool_use_stream("ask_user", "ask-1", json!({ "question": "Name?" })),
        text_stream(&["user said Ada"]),
        text_stream(&["reloaded final"]),
    ]);
    let engine = engine_with_config(
        &dir,
        LOCAL_CONFIG,
        fake.clone(),
        ToolRegistry::with_builtins(),
    );
    let (mut stdin, mut stdout, run) = spawn_cli(engine);
    let mut output = String::new();

    read_until(&mut stdout, &mut output, "[session ").await;
    stdin.write_all(b"hello\n").await.expect("send hello");
    read_until(&mut stdout, &mut output, "hello from engine").await;
    read_until_count(&mut stdout, &mut output, "[finished", 1).await;

    stdin.write_all(b"ask user\n").await.expect("send ask_user");
    read_until(&mut stdout, &mut output, "[question] Name?").await;
    stdin.write_all(b"Ada\n").await.expect("answer ask_user");
    read_until(&mut stdout, &mut output, "user said Ada").await;
    read_until_count(&mut stdout, &mut output, "[finished", 2).await;

    fs::write(
        dir.config_path(),
        LOCAL_CONFIG.replace("model-main", "model-main-reloaded"),
    )
    .expect("rewrite config before reload");
    stdin
        .write_all(b"/config reload\n")
        .await
        .expect("reload config");
    read_until(&mut stdout, &mut output, "[config reloaded]").await;
    read_until(&mut stdout, &mut output, "[config changed revision=1]").await;

    stdin
        .write_all(b"/config apply\n")
        .await
        .expect("apply reloaded config");
    read_until(&mut stdout, &mut output, "next turn boundary").await;
    stdin
        .write_all(b"after apply\n")
        .await
        .expect("send after apply");
    read_until(&mut stdout, &mut output, "reloaded final").await;
    read_until_count(&mut stdout, &mut output, "[finished", 3).await;

    finish_cli(stdin, run).await;

    let stream_requests = fake.stream_requests();
    let first_tools: Vec<&str> = stream_requests[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert!(
        first_tools.contains(&"ask_user"),
        "CLI drove an Engine with the ask_user tool: {first_tools:?}"
    );
    for tool in ["agent", "agent_result", "agent_cancel"] {
        assert!(
            first_tools.contains(&tool),
            "the instance tool `{tool}` is on the CLI-driven surface (M3-5): {first_tools:?}"
        );
    }
    assert!(
        !first_tools
            .iter()
            .any(|name| name.starts_with("ask_") && *name != "ask_user"),
        "no legacy delegate start tool remains: {first_tools:?}"
    );
    assert_eq!(
        stream_requests
            .last()
            .expect("request after /config apply")
            .model,
        "model-main-reloaded",
        "/config apply should reconfigure the live idle session before the next turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_cli_resumes_persisted_session_after_engine_restart() {
    let dir = TempDir::new("persisted-resume");
    let db = dir.0.join("sessions.db");

    let fake1 = FakeLlmClient::scripted(vec![text_stream(&["persisted first"])]);
    let client1: Arc<dyn LlmClient> = fake1;
    let engine1 = Engine::with_persistence(client1, ToolRegistry::with_builtins(), &db)
        .expect("open first persisted engine");
    let (mut stdin1, mut stdout1, run1) = spawn_cli(engine1);
    let mut output1 = String::new();

    read_until(&mut stdout1, &mut output1, "[session ").await;
    let session_id = first_session_id(&output1);
    stdin1
        .write_all(b"remember me\n")
        .await
        .expect("send persisted turn");
    read_until(&mut stdout1, &mut output1, "persisted first").await;
    read_until_count(&mut stdout1, &mut output1, "[finished", 1).await;
    finish_cli(stdin1, run1).await;

    let fake2 = FakeLlmClient::scripted(vec![text_stream(&["after restart"])]);
    let fake2_probe = fake2.clone();
    let client2: Arc<dyn LlmClient> = fake2;
    let engine2 = Engine::with_persistence(client2, ToolRegistry::with_builtins(), &db)
        .expect("open restarted persisted engine");
    let mut resumed_options = cli_options();
    resumed_options.resume = Some(SessionId::parse_str(&session_id).expect("session id"));
    let (mut stdin2, mut stdout2, run2) = spawn_cli_with_options(engine2, resumed_options);
    let mut output2 = String::new();

    read_until(
        &mut stdout2,
        &mut output2,
        &format!("[session {session_id} resumed]"),
    )
    .await;
    stdin2
        .write_all(b"continue after restart\n")
        .await
        .expect("send resumed turn");
    read_until(&mut stdout2, &mut output2, "after restart").await;
    read_until_count(&mut stdout2, &mut output2, "[finished", 1).await;
    finish_cli(stdin2, run2).await;

    let requests = fake2_probe.stream_requests();
    assert_eq!(requests.len(), 1, "resumed CLI should drive one new turn");
    let restored_context = format!("{:?}", requests[0].messages);
    for fragment in ["remember me", "persisted first", "continue after restart"] {
        assert!(
            restored_context.contains(fragment),
            "restored CLI context is missing `{fragment}`: {restored_context}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_cli_pivots_and_cancels_real_engine_runs() {
    let dir = TempDir::new("pivot-cancel");
    let gate = Gate::new();
    let fake = FakeLlmClient::scripted(vec![
        tool_use_stream("gate", "gate-1", json!({})),
        text_stream(&["pivot complete"]),
        stalling_text_stream(&["working"]),
    ]);
    let tools = ToolRegistry::new().register(Arc::new(GateTool { gate: gate.clone() }));
    let engine = engine_with_config(
        &dir,
        "[agents.default]\nmodel = \"model-main\"\n",
        fake.clone(),
        tools,
    );
    let (mut stdin, mut stdout, run) = spawn_cli(engine);
    let mut output = String::new();

    read_until(&mut stdout, &mut output, "[session ").await;
    stdin
        .write_all(b"start pivot\n")
        .await
        .expect("send first run");
    wait_for_stream_requests(&fake, 1).await;
    stdin
        .write_all(b"pivot update\n")
        .await
        .expect("send pivot text");
    read_until(&mut stdout, &mut output, "[pivot queued").await;
    gate.open();
    read_until(&mut stdout, &mut output, "[pivot applied").await;
    read_until(&mut stdout, &mut output, "pivot complete").await;
    read_until_count(&mut stdout, &mut output, "[finished", 1).await;
    let pivot_requests = fake.stream_requests();
    assert!(
        format!("{:?}", pivot_requests[1].messages).contains("pivot update"),
        "pivot text should enter the follow-up LLM request: {:?}",
        pivot_requests[1].messages
    );

    stdin
        .write_all(b"cancel target\n")
        .await
        .expect("send cancellable run");
    read_until(&mut stdout, &mut output, "working").await;
    stdin
        .write_all("\u{3}\n".as_bytes())
        .await
        .expect("send Ctrl-C");
    read_until(&mut stdout, &mut output, "[cancel requested").await;
    read_until(&mut stdout, &mut output, "[error cancelled]").await;

    finish_cli(stdin, run).await;
}

/// The `/resume` command recovers the current session and keeps it drivable
/// (the external ACP delegation leg of the retired static path is gone with
/// M3-5; the rebuilt external runtime's e2e lands with M4).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_cli_resume_command_recovers_the_session() {
    let dir = TempDir::new("resume-command");
    let fake = FakeLlmClient::scripted(vec![
        text_stream(&["first final"]),
        text_stream(&["resumed final"]),
    ]);
    let engine = engine_with_config(&dir, LOCAL_CONFIG, fake, ToolRegistry::with_builtins());
    let (mut stdin, mut stdout, run) = spawn_cli(engine);
    let mut output = String::new();

    read_until(&mut stdout, &mut output, "[session ").await;
    let session_id = first_session_id(&output);
    stdin
        .write_all(b"first turn\n")
        .await
        .expect("send first turn");
    read_until(&mut stdout, &mut output, "first final").await;
    read_until_count(&mut stdout, &mut output, "[finished", 1).await;

    stdin
        .write_all(format!("/resume {session_id}\n").as_bytes())
        .await
        .expect("resume current session");
    read_until(
        &mut stdout,
        &mut output,
        &format!("[session {session_id} resumed]"),
    )
    .await;
    stdin
        .write_all(b"after resume\n")
        .await
        .expect("send after resume");
    read_until(&mut stdout, &mut output, "resumed final").await;
    read_until_count(&mut stdout, &mut output, "[finished", 2).await;

    finish_cli(stdin, run).await;
}
