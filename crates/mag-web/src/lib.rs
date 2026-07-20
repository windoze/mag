#![warn(missing_docs)]

//! Web transport adapter for the transport-neutral `mag-service` contract.
//!
//! `mag-web` is intentionally a protocol translator: HTTP handlers deserialize
//! wire payloads, call an injected [`MagService`], and serialize the response or
//! REST error projection. It does not depend on engine crates and contains no
//! agent runtime logic.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, Sse},
    },
    routing::{delete, get, post},
};
use futures::{Stream, StreamExt, stream};
use mag_service::{
    ConfigDto, HistoryEntry, InteractionResponseWire, MagService, RequestId, RunId, ServiceError,
    ServiceEvent, SessionConfig, SessionId, SessionInfo, SourceInfo, UserInput,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{self, Instant, MissedTickBehavior},
};

/// Options used when serving the web adapter over TCP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    /// Host interface to bind.
    pub host: IpAddr,
    /// TCP port to bind.
    pub port: u16,
    /// Token policy reserved for the W2 auth layer.
    pub token_policy: TokenPolicy,
    /// Optional static asset directory reserved for the W2 static resource layer.
    pub static_assets_dir: Option<PathBuf>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8080,
            token_policy: TokenPolicy::Generate,
            static_assets_dir: None,
        }
    }
}

/// Authentication token strategy for the web server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenPolicy {
    /// Generate a token at startup.
    Generate,
    /// Use an externally supplied bearer token.
    Provided(String),
    /// Disable API authentication when allowed by the binding policy.
    Disabled,
}

/// Runs the web adapter until the HTTP server exits.
///
/// # Errors
///
/// Returns the bind or server I/O error reported by Tokio/axum.
pub async fn serve(service: Arc<dyn MagService>, opts: ServeOptions) -> std::io::Result<()> {
    let address = SocketAddr::new(opts.host, opts.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router(service)).await
}

/// Builds the REST router for an injected [`MagService`].
pub fn router(service: Arc<dyn MagService>) -> Router {
    router_with_sse_config(service, SseConfig::default())
}

fn router_with_sse_config(service: Arc<dyn MagService>, sse: SseConfig) -> Router {
    let state = AppState { service, sse };

    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/events", get(events))
        .route("/api/sessions/{id}/resume", post(resume_session))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/sessions/{id}/history", get(get_session_history))
        .route("/api/sessions/{id}/messages", post(send_message))
        .route("/api/sessions/{id}/pivot", post(pivot_message))
        .route("/api/sessions/{id}/cancel", post(cancel))
        .route(
            "/api/sessions/{id}/interactions/{request_id}",
            post(respond_interaction),
        )
        .route("/api/sources", get(list_sources))
        .route("/api/sources/probe", post(probe_local_agents))
        .route("/api/config", get(get_config).put(update_config))
        .route("/api/config/reload", post(reload_config))
        .route("/api/config/apply", post(apply_config))
        .with_state(state)
}

#[derive(Clone)]
struct AppState {
    service: Arc<dyn MagService>,
    sse: SseConfig,
}

#[derive(Clone, Copy)]
struct SseConfig {
    heartbeat_interval: Duration,
    queue_capacity: usize,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(15),
            queue_capacity: 64,
        }
    }
}

/// Error type returned by web API handlers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// A transport-neutral service error that should be projected per `docs/WEB.md` §2.3.
    Service(ServiceError),
    /// A non-service internal failure; the response body intentionally hides details.
    Internal,
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Service(error) => (
                service_error_status(&error),
                ErrorBody {
                    kind: error.kind().to_owned(),
                    message: error.to_string(),
                },
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody {
                    kind: "internal".to_owned(),
                    message: "internal server error".to_owned(),
                },
            ),
        };

        (status, Json(body)).into_response()
    }
}

fn service_error_status(error: &ServiceError) -> StatusCode {
    match error {
        ServiceError::SessionNotFound { .. } | ServiceError::InteractionNotFound { .. } => {
            StatusCode::NOT_FOUND
        }
        ServiceError::NotPivotable { .. } => StatusCode::CONFLICT,
        ServiceError::InvalidInput { .. } | ServiceError::Config { .. } => StatusCode::BAD_REQUEST,
        ServiceError::Unsupported { .. } => StatusCode::NOT_IMPLEMENTED,
        ServiceError::Backend { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Serialize)]
struct ErrorBody {
    kind: String,
    message: String,
}

#[derive(Serialize)]
struct CreateSessionResponse {
    id: SessionId,
    config: SessionConfig,
}

#[derive(Serialize)]
struct SendMessageResponse {
    run_id: RunId,
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    Ok(Json(state.service.list_sessions().await?))
}

async fn create_session(
    State(state): State<AppState>,
    Json(config): Json<SessionConfig>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let id = state.service.create_session(config.clone()).await?;
    Ok(Json(CreateSessionResponse { id, config }))
}

async fn resume_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<StatusCode, ApiError> {
    state.service.resume_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<StatusCode, ApiError> {
    state.service.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_session_history(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    Ok(Json(state.service.get_session_history(id).await?))
}

async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<UserInput>,
) -> Result<Json<SendMessageResponse>, ApiError> {
    let run_id = state.service.send_message(id, input).await?;
    Ok(Json(SendMessageResponse { run_id }))
}

async fn pivot_message(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<UserInput>,
) -> Result<StatusCode, ApiError> {
    state.service.pivot_message(id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<StatusCode, ApiError> {
    state.service.cancel(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn respond_interaction(
    State(state): State<AppState>,
    Path((id, request_id)): Path<(SessionId, RequestId)>,
    Json(response): Json<InteractionResponseWire>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .respond_interaction(id, request_id, response)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_sources(State(state): State<AppState>) -> Result<Json<Vec<SourceInfo>>, ApiError> {
    Ok(Json(state.service.list_sources().await?))
}

async fn probe_local_agents(
    State(state): State<AppState>,
) -> Result<Json<Vec<SourceInfo>>, ApiError> {
    Ok(Json(state.service.probe_local_agents().await?))
}

async fn get_config(State(state): State<AppState>) -> Result<Json<ConfigDto>, ApiError> {
    Ok(Json(state.service.get_config().await?))
}

async fn update_config(
    State(state): State<AppState>,
    Json(config): Json<ConfigDto>,
) -> Result<StatusCode, ApiError> {
    state.service.update_config(config).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reload_config(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.service.reload_config().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_config(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.service.apply_config().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let events = spawn_event_forwarder(state.service, state.sse.queue_capacity);
    Sse::new(sse_response_stream(events, state.sse.heartbeat_interval))
}

fn spawn_event_forwarder(
    service: Arc<dyn MagService>,
    queue_capacity: usize,
) -> mpsc::Receiver<SseEvent> {
    let (sender, receiver) = mpsc::channel(queue_capacity.max(1));

    tokio::spawn(async move {
        let mut events = service.subscribe(None);
        let mut next_id = 1_u64;

        loop {
            tokio::select! {
                _ = sender.closed() => break,
                next = events.next() => {
                    let Some(event) = next else {
                        break;
                    };

                    let frame = match service_event_to_sse(next_id, &event) {
                        Ok(frame) => frame,
                        Err(error) => {
                            eprintln!("mag-web failed to serialize SSE event: {error}");
                            break;
                        }
                    };

                    match sender.try_send(frame) {
                        Ok(()) => next_id += 1,
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            eprintln!("mag-web SSE client queue overflow; closing event stream");
                            break;
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
            }
        }
    });

    receiver
}

fn sse_response_stream(
    receiver: mpsc::Receiver<SseEvent>,
    heartbeat_interval: Duration,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    let mut heartbeat = time::interval_at(Instant::now() + heartbeat_interval, heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    stream::unfold(
        (receiver, heartbeat),
        |(mut receiver, mut heartbeat)| async move {
            tokio::select! {
                event = receiver.recv() => {
                    event.map(|event| (Ok(event), (receiver, heartbeat)))
                }
                _ = heartbeat.tick() => {
                    Some((Ok(SseEvent::default().comment("ping")), (receiver, heartbeat)))
                }
            }
        },
    )
}

fn service_event_to_sse(event_id: u64, event: &ServiceEvent) -> Result<SseEvent, String> {
    let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
    let event_type = event_type(&value)?.to_owned();
    let data = serde_json::to_string(&value).map_err(|error| error.to_string())?;

    Ok(SseEvent::default()
        .id(event_id.to_string())
        .event(event_type)
        .data(data))
}

fn event_type(value: &Value) -> Result<&str, String> {
    value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "serialized ServiceEvent missing type tag".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request},
    };
    use futures::{StreamExt, stream};
    use mag_service::{RoutingMode, ServiceEvent, SourceKindWire};
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::{
        collections::BTreeMap,
        net::{Ipv4Addr, SocketAddr},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant as StdInstant},
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::broadcast,
        task::JoinHandle,
        time::timeout,
    };
    use tower::ServiceExt;

    #[derive(Clone, Debug, PartialEq)]
    enum Call {
        CreateSession(SessionConfig),
        ListSessions,
        ResumeSession(SessionId),
        GetSessionHistory(SessionId),
        DeleteSession(SessionId),
        SendMessage(SessionId, UserInput),
        PivotMessage(SessionId, UserInput),
        Cancel(SessionId),
        RespondInteraction(SessionId, RequestId, InteractionResponseWire),
        Subscribe(Option<SessionId>),
        ListSources,
        ProbeLocalAgents,
        GetConfig,
        UpdateConfig(ConfigDto),
        ReloadConfig,
        ApplyConfig,
    }

    struct ScriptedService {
        calls: Mutex<Vec<Call>>,
        next_error: Mutex<Option<ServiceError>>,
        events: broadcast::Sender<ServiceEvent>,
        subscriptions: AtomicUsize,
        active_subscriptions: Arc<AtomicUsize>,
    }

    impl Default for ScriptedService {
        fn default() -> Self {
            let (events, _) = broadcast::channel(32);
            Self {
                calls: Mutex::new(Vec::new()),
                next_error: Mutex::new(None),
                events,
                subscriptions: AtomicUsize::new(0),
                active_subscriptions: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ScriptedService {
        fn push(&self, call: Call) {
            self.calls.lock().expect("calls mutex poisoned").push(call);
        }

        fn result<T>(&self, success: T) -> Result<T, ServiceError> {
            match self.next_error.lock().expect("error mutex poisoned").take() {
                Some(error) => Err(error),
                None => Ok(success),
            }
        }

        fn set_error(&self, error: ServiceError) {
            *self.next_error.lock().expect("error mutex poisoned") = Some(error);
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("calls mutex poisoned").clone()
        }

        fn send_event(&self, event: ServiceEvent) {
            let _ = self.events.send(event);
        }

        fn subscription_count(&self) -> usize {
            self.subscriptions.load(Ordering::SeqCst)
        }

        fn active_subscription_count(&self) -> usize {
            self.active_subscriptions.load(Ordering::SeqCst)
        }
    }

    struct SubscriptionGuard {
        active: Arc<AtomicUsize>,
    }

    impl Drop for SubscriptionGuard {
        fn drop(&mut self) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl MagService for ScriptedService {
        async fn create_session(&self, config: SessionConfig) -> Result<SessionId, ServiceError> {
            self.push(Call::CreateSession(config));
            self.result(session_id())
        }

        async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
            self.push(Call::ListSessions);
            self.result(vec![SessionInfo::new(session_id(), config())])
        }

        async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
            self.push(Call::ResumeSession(id));
            self.result(())
        }

        async fn get_session_history(
            &self,
            id: SessionId,
        ) -> Result<Vec<HistoryEntry>, ServiceError> {
            self.push(Call::GetSessionHistory(id));
            self.result(vec![HistoryEntry::UserMessage {
                text: "hello".to_owned(),
                attachments: Vec::new(),
            }])
        }

        async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError> {
            self.push(Call::DeleteSession(id));
            self.result(())
        }

        async fn send_message(
            &self,
            id: SessionId,
            input: UserInput,
        ) -> Result<RunId, ServiceError> {
            self.push(Call::SendMessage(id, input));
            self.result(run_id())
        }

        async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
            self.push(Call::Cancel(id));
            self.result(())
        }

        async fn pivot_message(&self, id: SessionId, input: UserInput) -> Result<(), ServiceError> {
            self.push(Call::PivotMessage(id, input));
            self.result(())
        }

        async fn respond_interaction(
            &self,
            id: SessionId,
            request_id: RequestId,
            response: InteractionResponseWire,
        ) -> Result<(), ServiceError> {
            self.push(Call::RespondInteraction(id, request_id, response));
            self.result(())
        }

        fn subscribe(
            &self,
            id: Option<SessionId>,
        ) -> futures::stream::BoxStream<'static, ServiceEvent> {
            self.push(Call::Subscribe(id));
            self.subscriptions.fetch_add(1, Ordering::SeqCst);
            self.active_subscriptions.fetch_add(1, Ordering::SeqCst);

            let receiver = self.events.subscribe();
            let guard = SubscriptionGuard {
                active: self.active_subscriptions.clone(),
            };

            stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => return Some((event, (receiver, guard))),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            })
            .boxed()
        }

        async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
            self.push(Call::ListSources);
            self.result(vec![source()])
        }

        async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
            self.push(Call::ProbeLocalAgents);
            self.result(vec![source()])
        }

        async fn get_config(&self) -> Result<ConfigDto, ServiceError> {
            self.push(Call::GetConfig);
            self.result(ConfigDto::default())
        }

        async fn update_config(&self, config: ConfigDto) -> Result<(), ServiceError> {
            self.push(Call::UpdateConfig(config));
            self.result(())
        }

        async fn reload_config(&self) -> Result<(), ServiceError> {
            self.push(Call::ReloadConfig);
            self.result(())
        }

        async fn apply_config(&self) -> Result<(), ServiceError> {
            self.push(Call::ApplyConfig);
            self.result(())
        }
    }

    fn session_id() -> SessionId {
        SessionId::parse_str("00000000-0000-0000-0000-000000000001").expect("valid session id")
    }

    fn request_id() -> RequestId {
        RequestId::parse_str("00000000-0000-0000-0000-000000000002").expect("valid request id")
    }

    fn run_id() -> RunId {
        RunId::parse_str("00000000-0000-0000-0000-000000000003").expect("valid run id")
    }

    fn config() -> SessionConfig {
        SessionConfig {
            provider: "openai".to_owned(),
            model: "gpt-5-codex".to_owned(),
            tool_profile: Some("default".to_owned()),
            cwd: None,
            routing: RoutingMode::ModelRouted,
            budget: None,
        }
    }

    fn input(text: &str) -> UserInput {
        UserInput::text(text)
    }

    fn interaction_response() -> InteractionResponseWire {
        InteractionResponseWire::Answer {
            text: "approved".to_owned(),
        }
    }

    fn source() -> SourceInfo {
        SourceInfo {
            id: "codex".to_owned(),
            name: "Codex".to_owned(),
            kind: SourceKindWire::LocalAgent,
            available: true,
            version: Some("1.2.3".to_owned()),
            path: None,
            capabilities: vec!["edit".to_owned()],
        }
    }

    fn request(method: Method, uri: impl AsRef<str>, body: Option<Value>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri.as_ref());
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }

        let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        builder.body(body).expect("request builds")
    }

    fn json_value<T: Serialize>(value: T) -> Value {
        serde_json::to_value(value).expect("value serializes")
    }

    async fn body_json<T: DeserializeOwned>(response: Response) -> T {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body readable");
        serde_json::from_slice(&bytes).expect("response body is JSON")
    }

    async fn assert_empty_body(response: Response) {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body readable");
        assert!(bytes.is_empty(), "204 response body must be empty");
    }

    fn text_delta(text: &str) -> ServiceEvent {
        ServiceEvent::TextDelta {
            id: session_id(),
            text: text.to_owned(),
        }
    }

    async fn spawn_loopback_server(
        service: Arc<ScriptedService>,
        heartbeat_interval: Duration,
    ) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("loopback listener binds");
        let address = listener.local_addr().expect("listener has local address");
        let app = router_with_sse_config(
            service,
            SseConfig {
                heartbeat_interval,
                queue_capacity: 8,
            },
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test server runs");
        });

        (address, handle)
    }

    async fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = StdInstant::now() + Duration::from_secs(2);
        while StdInstant::now() < deadline {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(predicate(), "condition was not met before timeout");
    }

    struct SseClient {
        reader: BufReader<TcpStream>,
        chunked: bool,
        decoded: Vec<u8>,
    }

    impl SseClient {
        async fn connect(address: SocketAddr) -> Self {
            let mut stream = TcpStream::connect(address)
                .await
                .expect("SSE client connects");
            let request = format!(
                "GET /api/events HTTP/1.1\r\nHost: {address}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("SSE request writes");

            let mut reader = BufReader::new(stream);
            let mut status = String::new();
            reader
                .read_line(&mut status)
                .await
                .expect("status line reads");
            assert!(
                status.starts_with("HTTP/1.1 200"),
                "unexpected status line: {status:?}"
            );

            let mut chunked = false;
            let mut event_stream = false;
            loop {
                let mut header = String::new();
                let read = reader
                    .read_line(&mut header)
                    .await
                    .expect("header line reads");
                assert!(read > 0, "response ended before headers completed");

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

            assert!(event_stream, "SSE response must use text/event-stream");

            Self {
                reader,
                chunked,
                decoded: Vec::new(),
            }
        }

        async fn next_frame(&mut self) -> String {
            timeout(Duration::from_secs(2), self.read_frame())
                .await
                .expect("SSE frame arrives before timeout")
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
                    .expect("chunk size line reads");
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
                    .expect("chunk body reads");
                self.decoded.extend(chunk);

                let mut trailer = [0_u8; 2];
                self.reader
                    .read_exact(&mut trailer)
                    .await
                    .expect("chunk trailer reads");
                assert_eq!(&trailer, b"\r\n", "chunk must end with CRLF");
                true
            } else {
                let mut buffer = [0_u8; 1024];
                let read = self.reader.read(&mut buffer).await.expect("SSE body reads");
                if read == 0 {
                    return false;
                }
                self.decoded.extend_from_slice(&buffer[..read]);
                true
            }
        }
    }

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

    fn frame_fields(frame: &str) -> BTreeMap<String, String> {
        frame
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                Some((key.to_owned(), value.trim_start().to_owned()))
            })
            .collect()
    }

    #[tokio::test]
    async fn rest_routes_map_to_service_methods_and_success_bodies() {
        let service = Arc::new(ScriptedService::default());
        let app = router(service.clone());
        let id = session_id();
        let request_id = request_id();
        let session_path = format!("/api/sessions/{id}");
        let config = config();
        let message = input("hello");
        let pivot = input("steer");
        let interaction = interaction_response();

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/sessions", None))
            .await
            .expect("list sessions response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(vec![SessionInfo::new(id, config.clone())])
        );

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/sessions",
                Some(json_value(config.clone())),
            ))
            .await
            .expect("create session response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json!({ "id": id, "config": config.clone() })
        );

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                format!("{session_path}/resume"),
                None,
            ))
            .await
            .expect("resume session response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                format!("{session_path}/history"),
                None,
            ))
            .await
            .expect("history response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(vec![HistoryEntry::UserMessage {
                text: "hello".to_owned(),
                attachments: Vec::new(),
            }])
        );

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                format!("{session_path}/messages"),
                Some(json_value(message.clone())),
            ))
            .await
            .expect("send message response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json!({ "run_id": run_id() })
        );

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                format!("{session_path}/pivot"),
                Some(json_value(pivot.clone())),
            ))
            .await
            .expect("pivot response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                format!("{session_path}/cancel"),
                None,
            ))
            .await
            .expect("cancel response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                format!("{session_path}/interactions/{request_id}"),
                Some(json_value(interaction.clone())),
            ))
            .await
            .expect("interaction response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/sources", None))
            .await
            .expect("list sources response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(vec![source()])
        );

        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/sources/probe", None))
            .await
            .expect("probe sources response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(vec![source()])
        );

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/config", None))
            .await
            .expect("get config response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(ConfigDto::default())
        );

        let response = app
            .clone()
            .oneshot(request(
                Method::PUT,
                "/api/config",
                Some(json_value(ConfigDto::default())),
            ))
            .await
            .expect("update config response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/config/reload", None))
            .await
            .expect("reload config response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/config/apply", None))
            .await
            .expect("apply config response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        let response = app
            .oneshot(request(Method::DELETE, session_path, None))
            .await
            .expect("delete response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_empty_body(response).await;

        assert_eq!(
            service.calls(),
            vec![
                Call::ListSessions,
                Call::CreateSession(config.clone()),
                Call::ResumeSession(id),
                Call::GetSessionHistory(id),
                Call::SendMessage(id, message),
                Call::PivotMessage(id, pivot),
                Call::Cancel(id),
                Call::RespondInteraction(id, request_id, interaction),
                Call::ListSources,
                Call::ProbeLocalAgents,
                Call::GetConfig,
                Call::UpdateConfig(ConfigDto::default()),
                Call::ReloadConfig,
                Call::ApplyConfig,
                Call::DeleteSession(id),
            ]
        );
    }

    #[tokio::test]
    async fn service_errors_project_to_documented_status_and_body() {
        let cases = vec![
            (
                ServiceError::SessionNotFound { id: session_id() },
                StatusCode::NOT_FOUND,
            ),
            (
                ServiceError::InteractionNotFound {
                    request_id: request_id(),
                },
                StatusCode::NOT_FOUND,
            ),
            (
                ServiceError::InvalidInput {
                    message: "empty message".to_owned(),
                },
                StatusCode::BAD_REQUEST,
            ),
            (
                ServiceError::NotPivotable {
                    id: session_id(),
                    reason: "no in-progress run".to_owned(),
                },
                StatusCode::CONFLICT,
            ),
            (
                ServiceError::Unsupported {
                    operation: "get_config".to_owned(),
                },
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                ServiceError::Config {
                    message: "invalid config".to_owned(),
                },
                StatusCode::BAD_REQUEST,
            ),
            (
                ServiceError::Backend {
                    message: "store unavailable".to_owned(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, expected_status) in cases {
            let service = Arc::new(ScriptedService::default());
            service.set_error(error.clone());

            let response = router(service)
                .oneshot(request(Method::GET, "/api/sessions", None))
                .await
                .expect("error response");

            assert_eq!(response.status(), expected_status);
            assert_eq!(
                body_json::<Value>(response).await,
                json!({ "kind": error.kind(), "message": error.to_string() })
            );
        }
    }

    #[tokio::test]
    async fn internal_errors_use_generic_projection() {
        let response = ApiError::Internal.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json::<Value>(response).await,
            json!({ "kind": "internal", "message": "internal server error" })
        );
    }

    #[tokio::test]
    async fn sse_endpoint_streams_event_frames_with_ids_and_heartbeats() {
        let service = Arc::new(ScriptedService::default());
        let (address, server) =
            spawn_loopback_server(service.clone(), Duration::from_millis(50)).await;
        let mut client = SseClient::connect(address).await;
        wait_for(|| service.subscription_count() == 1).await;

        service.send_event(text_delta("hello"));

        let frame = client.next_frame().await;
        let fields = frame_fields(&frame);
        assert_eq!(fields.get("id").map(String::as_str), Some("1"));
        assert_eq!(fields.get("event").map(String::as_str), Some("text_delta"));
        let data: Value =
            serde_json::from_str(fields.get("data").expect("event frame contains JSON data"))
                .expect("event data is JSON");
        assert_eq!(data["type"], "text_delta");
        assert_eq!(data["id"], json!(session_id()));
        assert_eq!(data["text"], "hello");

        let heartbeat = client.next_frame().await;
        let heartbeat_fields = frame_fields(&heartbeat);
        assert_eq!(heartbeat_fields.get("").map(String::as_str), Some("ping"));
        assert!(
            !heartbeat_fields.contains_key("id"),
            "heartbeat comments should not consume event ids"
        );

        drop(client);
        server.abort();
    }

    #[tokio::test]
    async fn sse_endpoint_broadcasts_to_independent_connections() {
        let service = Arc::new(ScriptedService::default());
        let (address, server) =
            spawn_loopback_server(service.clone(), Duration::from_secs(5)).await;
        let mut first = SseClient::connect(address).await;
        let mut second = SseClient::connect(address).await;
        wait_for(|| service.subscription_count() == 2).await;

        service.send_event(ServiceEvent::ConfigChanged { revision: 7 });

        for client in [&mut first, &mut second] {
            let frame = client.next_frame().await;
            let fields = frame_fields(&frame);
            assert_eq!(fields.get("id").map(String::as_str), Some("1"));
            assert_eq!(
                fields.get("event").map(String::as_str),
                Some("config_changed")
            );
            let data: Value =
                serde_json::from_str(fields.get("data").expect("event frame contains JSON data"))
                    .expect("event data is JSON");
            assert_eq!(data, json!({ "type": "config_changed", "revision": 7 }));
        }

        assert_eq!(
            service.calls(),
            vec![Call::Subscribe(None), Call::Subscribe(None)]
        );

        drop(first);
        drop(second);
        server.abort();
    }

    #[tokio::test]
    async fn sse_forwarder_cleans_up_subscription_after_client_disconnect() {
        let service = Arc::new(ScriptedService::default());
        let (address, server) =
            spawn_loopback_server(service.clone(), Duration::from_millis(50)).await;
        let client = SseClient::connect(address).await;
        wait_for(|| service.active_subscription_count() == 1).await;

        drop(client);

        wait_for(|| service.active_subscription_count() == 0).await;
        server.abort();
    }

    #[tokio::test]
    async fn sse_forwarder_disconnects_when_bounded_queue_overflows() {
        let service = Arc::new(ScriptedService::default());
        let _receiver = spawn_event_forwarder(service.clone(), 1);
        wait_for(|| service.active_subscription_count() == 1).await;

        service.send_event(text_delta("first"));
        service.send_event(text_delta("second"));

        wait_for(|| service.active_subscription_count() == 0).await;
    }
}
