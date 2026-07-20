//! Protocol-level e2e coverage for `mag --web` assembly pieces (`TODO.md` W2-4).
//!
//! The test stays fully offline: a scripted [`LlmClient`] drives a real
//! [`Engine`], `mag-web` is served on a loopback TCP listener, and tiny raw
//! HTTP/SSE clients exercise the public REST + SSE protocol.

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
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
use mag_core::{ConfigService, Engine};
use mag_service::{
    ApprovalDecisionWire, HistoryEntry, InteractionKindWire, InteractionResponseWire, MagService,
    RoutingMode, RunErrorKind, ServiceEvent, SessionConfig, SessionId, StepIdWire, ToolCallIdWire,
    ToolStatusWire, UserInput,
};
use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::{Duration, timeout},
};

const WEB_TOKEN: &str = "web-e2e-token";

/// Unique temp directory per test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    /// Creates a temp directory with a readable tag.
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mag-web-e2e-{tag}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    /// Returns the config path inside this temp directory.
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

/// Offline scripted LLM client used by the web e2e test.
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

    /// Returns requests made through the streaming endpoint.
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

/// Gate used by the pivot step to park a tool call until the test releases it.
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

/// Permission-gated shell stub used to exercise `interaction_requested`.
#[derive(Debug)]
struct ShellTool;

#[async_trait]
impl ToolPlugin for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: "shell".to_owned(),
            description: "offline shell stub".to_owned(),
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

/// Auto-approved tool that holds the run open so a pivot can be posted.
#[derive(Debug)]
struct HoldTool {
    gate: Arc<Gate>,
}

#[async_trait]
impl ToolPlugin for HoldTool {
    fn name(&self) -> &str {
        "hold"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: "hold".to_owned(),
            description: "offline gated tool".to_owned(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
        self.gate.wait().await;
        ToolResult::text("hold released")
    }
}

/// Builds the tool registry used by the e2e engine.
fn registry(gate: Arc<Gate>) -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(ShellTool))
        .register(Arc::new(HoldTool { gate }))
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

/// Builds a stalling text stream used to test web cancellation.
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

/// Session config used by the web client.
fn session_config() -> SessionConfig {
    SessionConfig {
        provider: "default".to_owned(),
        model: "wire-model".to_owned(),
        tool_profile: None,
        cwd: None,
        routing: RoutingMode::ModelRouted,
        budget: None,
    }
}

/// A spawned loopback web server.
struct TestServer {
    address: SocketAddr,
    token: String,
    handle: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Starts `mag-web` with API auth enabled on a loopback listener.
async fn spawn_web_server(engine: Engine) -> TestServer {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind loopback web listener");
    let address = listener.local_addr().expect("listener local address");
    let service: Arc<dyn MagService> = Arc::new(engine);
    let prepared = mag_web::prepare_router(
        service,
        mag_web::ServeOptions {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            token_policy: mag_web::TokenPolicy::Provided(WEB_TOKEN.to_owned()),
            static_assets_dir: None,
        },
    )
    .expect("prepare web router");
    let handle = tokio::spawn(async move {
        mag_web::serve_prepared(listener, prepared)
            .await
            .expect("web server runs");
    });

    TestServer {
        address,
        token: WEB_TOKEN.to_owned(),
        handle,
    }
}

/// Minimal HTTP response decoded by the raw loopback client.
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

/// Sends one authenticated HTTP request to the test server.
async fn http_request(
    server: &TestServer,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> HttpResponse {
    let body = body.map(|value| value.to_string()).unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n",
        server.address, server.token
    );
    if body.is_empty() {
        request.push_str("Content-Length: 0\r\n\r\n");
    } else {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
    }

    let mut stream = TcpStream::connect(server.address)
        .await
        .expect("connect HTTP client");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write HTTP request");
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("HTTP response timed out")
        .expect("read HTTP response");
    parse_http_response(&response)
}

/// Parses an HTTP/1.1 response, including chunked bodies.
fn parse_http_response(bytes: &[u8]) -> HttpResponse {
    let (head, raw_body) = split_http_head(bytes);
    let head = std::str::from_utf8(head).expect("HTTP headers are utf-8");
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("HTTP status exists")
        .parse::<u16>()
        .expect("HTTP status is numeric");
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        decode_chunked(raw_body)
    } else {
        raw_body.to_vec()
    };

    HttpResponse { status, body }
}

/// Splits raw HTTP bytes into header and body sections.
fn split_http_head(bytes: &[u8]) -> (&[u8], &[u8]) {
    let index = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator exists");
    (&bytes[..index], &bytes[index + 4..])
}

/// Decodes a chunked transfer body.
fn decode_chunked(bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut cursor = 0;
    loop {
        let Some(line_end) = bytes[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
        else {
            panic!("chunk size line missing");
        };
        let size_line = std::str::from_utf8(&bytes[cursor..line_end]).expect("chunk size utf-8");
        let size_hex = size_line.split(';').next().expect("chunk size present");
        let size = usize::from_str_radix(size_hex, 16).expect("chunk size is hex");
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        decoded.extend_from_slice(&bytes[cursor..cursor + size]);
        cursor += size + 2;
    }
    decoded
}

/// Deserializes a JSON response body.
fn response_json(response: &HttpResponse) -> Value {
    serde_json::from_slice(&response.body).expect("response body is JSON")
}

/// SSE client for the authenticated `/api/events` endpoint.
struct SseClient {
    reader: BufReader<TcpStream>,
    chunked: bool,
    decoded: Vec<u8>,
}

impl SseClient {
    /// Connects to the test server's SSE endpoint.
    async fn connect(server: &TestServer) -> Self {
        let mut stream = TcpStream::connect(server.address)
            .await
            .expect("connect SSE client");
        let request = format!(
            "GET /api/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
            server.address, server.token
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write SSE request");

        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader
            .read_line(&mut status)
            .await
            .expect("read SSE status");
        assert!(status.starts_with("HTTP/1.1 200"), "SSE status: {status:?}");

        let mut chunked = false;
        let mut event_stream = false;
        loop {
            let mut header = String::new();
            let read = reader.read_line(&mut header).await.expect("read header");
            assert!(read > 0, "SSE response ended before headers");
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let lower = header.to_ascii_lowercase();
            if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
                chunked = true;
            }
            if lower.starts_with("content-type:") && lower.contains("text/event-stream") {
                event_stream = true;
            }
        }
        assert!(event_stream, "SSE content-type must be text/event-stream");

        Self {
            reader,
            chunked,
            decoded: Vec::new(),
        }
    }

    /// Reads the next service event, skipping heartbeat comments.
    async fn next_event(&mut self) -> ServiceEvent {
        loop {
            let frame = timeout(Duration::from_secs(5), self.read_frame())
                .await
                .expect("SSE frame timed out");
            if let Some(event) = service_event_from_frame(&frame) {
                return event;
            }
        }
    }

    async fn read_frame(&mut self) -> String {
        loop {
            if let Some((index, separator_len)) = frame_separator(&self.decoded) {
                let frame = self.decoded.drain(..index).collect::<Vec<_>>();
                self.decoded.drain(..separator_len);
                return String::from_utf8(frame).expect("SSE frame is utf-8");
            }

            assert!(self.read_more().await, "SSE stream ended before next frame");
        }
    }

    async fn read_more(&mut self) -> bool {
        if self.chunked {
            let mut size_line = String::new();
            let read = self
                .reader
                .read_line(&mut size_line)
                .await
                .expect("read chunk size");
            if read == 0 {
                return false;
            }
            let size_hex = size_line
                .trim_end()
                .split(';')
                .next()
                .expect("chunk size exists");
            let size = usize::from_str_radix(size_hex, 16).expect("chunk size is hex");
            if size == 0 {
                return false;
            }
            let mut chunk = vec![0; size];
            self.reader
                .read_exact(&mut chunk)
                .await
                .expect("read chunk body");
            self.decoded.extend(chunk);
            let mut trailer = [0_u8; 2];
            self.reader
                .read_exact(&mut trailer)
                .await
                .expect("read chunk trailer");
            assert_eq!(&trailer, b"\r\n", "chunk trailer must be CRLF");
            true
        } else {
            let mut buffer = [0_u8; 1024];
            let read = self.reader.read(&mut buffer).await.expect("read SSE body");
            if read == 0 {
                return false;
            }
            self.decoded.extend_from_slice(&buffer[..read]);
            true
        }
    }
}

/// Finds the end of one SSE frame.
fn frame_separator(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
}

/// Converts an SSE frame into a service event when the frame carries data.
fn service_event_from_frame(frame: &str) -> Option<ServiceEvent> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return None;
    }
    Some(serde_json::from_str(&data).expect("SSE data is a ServiceEvent"))
}

/// Reads events until one satisfies `predicate`.
async fn wait_for_event(
    sse: &mut SseClient,
    context: &str,
    predicate: impl Fn(&ServiceEvent) -> bool,
) -> ServiceEvent {
    for _ in 0..64 {
        let event = sse.next_event().await;
        if predicate(&event) {
            return event;
        }
    }
    panic!("timed out waiting for {context}");
}

/// Reads events until a run terminal event for `session` is observed.
async fn collect_until_terminal(sse: &mut SseClient, session: SessionId) -> Vec<ServiceEvent> {
    let mut events = Vec::new();
    loop {
        let event = sse.next_event().await;
        let terminal = matches!(
            &event,
            ServiceEvent::RunFinished { id, .. } | ServiceEvent::RunError { id, .. } if *id == session
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

/// Builds an approval response; step/call ids are reconstructed by the engine.
fn approve() -> InteractionResponseWire {
    InteractionResponseWire::Approval {
        step_id: StepIdWire::parse_str("00000000-0000-0000-0000-000000000000")
            .expect("nil step id parses"),
        call_id: ToolCallIdWire::parse_str("00000000-0000-0000-0000-000000000000")
            .expect("nil call id parses"),
        decision: ApprovalDecisionWire::Approve,
        message: None,
    }
}

const CONFIG: &str = r#"
[providers.fake]
wire = "openai"
base_url = "https://example.invalid"
api_key = { env = "MAG_WEB_E2E_API_KEY" }

[agents.default]
provider = "fake"
model = "model-web"

[tools.shell]
approval = "ask"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_protocol_e2e_drives_engine_over_http_and_sse() {
    let dir = TempDir::new("protocol");
    let pivot_gate = Gate::new();
    let fake = FakeLlmClient::scripted(vec![
        tool_use_stream("shell", "shell-1", json!({ "command": "echo hi" })),
        text_stream(&["tool done"]),
        tool_use_stream("hold", "hold-1", json!({})),
        text_stream(&["pivot done"]),
        stalling_text_stream(&["working"]),
    ]);
    let engine = engine_with_config(&dir, CONFIG, fake.clone(), registry(pivot_gate.clone()));
    let server = spawn_web_server(engine).await;
    let mut sse = SseClient::connect(&server).await;

    let response = http_request(
        &server,
        "POST",
        "/api/sessions",
        Some(serde_json::to_value(session_config()).expect("session config serializes")),
    )
    .await;
    assert_eq!(response.status, 200, "create session response");
    let created = response_json(&response);
    let session = SessionId::parse_str(created["id"].as_str().expect("created id is string"))
        .expect("created id parses");
    assert!(matches!(
        wait_for_event(&mut sse, "session_created", |event| matches!(
            event,
            ServiceEvent::SessionCreated { id, .. } if *id == session
        ))
        .await,
        ServiceEvent::SessionCreated { .. }
    ));

    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/messages"),
        Some(serde_json::to_value(UserInput::text("needs approval")).expect("input serializes")),
    )
    .await;
    assert_eq!(response.status, 200, "send message response");
    wait_for_event(&mut sse, "run_started", |event| {
        matches!(
            event,
            ServiceEvent::RunStarted { id, .. } if *id == session
        )
    })
    .await;
    let request_id = match wait_for_event(&mut sse, "interaction_requested", |event| {
        matches!(
            event,
            ServiceEvent::InteractionRequested { id, kind, .. }
                if *id == session && matches!(kind, InteractionKindWire::Approval { .. })
        )
    })
    .await
    {
        ServiceEvent::InteractionRequested { request_id, .. } => request_id,
        other => panic!("expected interaction_requested, got {other:?}"),
    };

    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/interactions/{request_id}"),
        Some(serde_json::to_value(approve()).expect("approval serializes")),
    )
    .await;
    assert_eq!(response.status, 204, "approve response");
    let approved_run = collect_until_terminal(&mut sse, session).await;
    assert!(
        approved_run.iter().any(|event| matches!(
            event,
            ServiceEvent::ToolStarted { id, trace } if *id == session && trace.name == "shell"
        )),
        "approved run starts shell tool: {approved_run:?}"
    );
    assert!(
        approved_run.iter().any(|event| matches!(
            event,
            ServiceEvent::ToolFinished { id, trace }
                if *id == session && trace.name == "shell" && trace.status == ToolStatusWire::Finished
        )),
        "approved run finishes shell tool: {approved_run:?}"
    );
    assert!(matches!(
        approved_run.last(),
        Some(ServiceEvent::RunFinished { output, .. }) if output.text == "tool done"
    ));

    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/messages"),
        Some(serde_json::to_value(UserInput::text("start pivot")).expect("input serializes")),
    )
    .await;
    assert_eq!(response.status, 200, "pivot setup message response");
    wait_for_event(&mut sse, "pivot run_started", |event| {
        matches!(
            event,
            ServiceEvent::RunStarted { id, .. } if *id == session
        )
    })
    .await;
    wait_for_event(&mut sse, "hold tool started", |event| {
        matches!(
            event,
            ServiceEvent::ToolStarted { id, trace } if *id == session && trace.name == "hold"
        )
    })
    .await;

    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/pivot"),
        Some(serde_json::to_value(UserInput::text("pivot text")).expect("input serializes")),
    )
    .await;
    assert_eq!(response.status, 204, "pivot response");
    wait_for_event(&mut sse, "pivot_queued", |event| {
        matches!(
            event,
            ServiceEvent::PivotQueued { id } if *id == session
        )
    })
    .await;
    pivot_gate.open();
    let pivoted_run = collect_until_terminal(&mut sse, session).await;
    assert!(
        pivoted_run.iter().any(|event| matches!(
            event,
            ServiceEvent::PivotApplied { id } if *id == session
        )),
        "pivot is applied before terminal event: {pivoted_run:?}"
    );
    assert!(matches!(
        pivoted_run.last(),
        Some(ServiceEvent::RunFinished { output, .. }) if output.text == "pivot done"
    ));

    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/messages"),
        Some(serde_json::to_value(UserInput::text("cancel target")).expect("input serializes")),
    )
    .await;
    assert_eq!(response.status, 200, "cancellable message response");
    wait_for_event(&mut sse, "cancel run_started", |event| {
        matches!(
            event,
            ServiceEvent::RunStarted { id, .. } if *id == session
        )
    })
    .await;
    wait_for_event(&mut sse, "cancellable text", |event| {
        matches!(
            event,
            ServiceEvent::TextDelta { id, text } if *id == session && text == "working"
        )
    })
    .await;
    let response = http_request(
        &server,
        "POST",
        &format!("/api/sessions/{session}/cancel"),
        None,
    )
    .await;
    assert_eq!(response.status, 204, "cancel response");
    wait_for_event(&mut sse, "cancelled run_error", |event| matches!(
        event,
        ServiceEvent::RunError { id, kind, .. } if *id == session && *kind == RunErrorKind::Cancelled
    ))
    .await;

    let response = http_request(
        &server,
        "GET",
        &format!("/api/sessions/{session}/history"),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "history response");
    let history: Vec<HistoryEntry> = serde_json::from_slice(&response.body).expect("history JSON");
    assert!(
        history.iter().any(|entry| matches!(
            entry,
            HistoryEntry::UserMessage { text, .. } if text == "needs approval"
        )),
        "history includes the first user message: {history:?}"
    );
    assert!(
        history.iter().any(|entry| matches!(
            entry,
            HistoryEntry::ToolCall { trace }
                if trace.name == "shell" && trace.status == ToolStatusWire::Finished
        )),
        "history includes the approved shell tool call: {history:?}"
    );

    let response = http_request(&server, "GET", "/api/config", None).await;
    assert_eq!(response.status, 200, "config get response");
    let config = response_json(&response);
    assert_eq!(config["agents"]["default"]["model"], "model-web");

    fs::write(
        dir.config_path(),
        CONFIG.replace("model-web", "model-web-reloaded"),
    )
    .expect("rewrite config");
    let response = http_request(&server, "POST", "/api/config/reload", None).await;
    assert_eq!(response.status, 204, "config reload response");
    wait_for_event(&mut sse, "config_changed", |event| {
        matches!(event, ServiceEvent::ConfigChanged { .. })
    })
    .await;
    let response = http_request(&server, "POST", "/api/config/apply", None).await;
    assert_eq!(response.status, 204, "config apply response");

    let response = http_request(&server, "GET", "/api/sources", None).await;
    assert_eq!(response.status, 200, "sources response");
    let sources = response_json(&response);
    assert!(
        sources
            .as_array()
            .expect("sources array")
            .iter()
            .any(|source| source["id"] == "fake"),
        "configured provider is listed as a source: {sources}"
    );
    let response = http_request(&server, "POST", "/api/sources/probe", None).await;
    assert_eq!(response.status, 200, "source probe response");
    assert!(response_json(&response).is_array());

    let stream_requests = fake.stream_requests();
    assert!(
        stream_requests
            .iter()
            .any(|request| request.model == "model-web"),
        "web e2e drove the configured Engine model: {stream_requests:?}"
    );
    assert!(
        stream_requests
            .iter()
            .any(|request| format!("{:?}", request.messages).contains("pivot text")),
        "pivot text entered the follow-up model context: {stream_requests:?}"
    );
}
