#![warn(missing_docs)]

//! Web transport adapter for the transport-neutral `mag-service` contract.
//!
//! `mag-web` is intentionally a protocol translator: HTTP handlers deserialize
//! wire payloads, call an injected [`MagService`], and serialize the response or
//! REST error projection. It does not depend on engine crates and contains no
//! agent runtime logic.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
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
    fs,
    io::{self, Read},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path as FsPath, PathBuf},
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

/// Router plus resolved options produced from [`ServeOptions`].
pub struct PreparedRouter {
    router: Router,
    options: ResolvedServeOptions,
}

impl PreparedRouter {
    /// Returns the resolved options, including any generated auth token.
    pub const fn options(&self) -> &ResolvedServeOptions {
        &self.options
    }

    /// Consumes the prepared value and returns the axum router.
    pub fn into_router(self) -> Router {
        self.router
    }
}

/// Fully resolved serving options after applying auth and asset defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedServeOptions {
    address: SocketAddr,
    auth_token: Option<String>,
    static_assets: StaticAssets,
}

impl ResolvedServeOptions {
    /// Returns the TCP address that should be bound.
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Returns the bearer token when API auth is enabled.
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// Returns whether API auth is enabled.
    pub const fn auth_enabled(&self) -> bool {
        self.auth_token.is_some()
    }

    /// Returns the filesystem asset directory when this build serves one.
    pub fn static_assets_dir(&self) -> Option<&FsPath> {
        match &self.static_assets {
            StaticAssets::Directory(path) => Some(path.as_path()),
            #[cfg(not(debug_assertions))]
            StaticAssets::Embedded => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StaticAssets {
    Directory(PathBuf),
    #[cfg(not(debug_assertions))]
    Embedded,
}

#[derive(Clone)]
struct AuthState {
    token: Arc<str>,
}

/// Resolves serving options and builds the axum router.
///
/// # Errors
///
/// Returns an I/O error if the default random token cannot be generated.
pub fn prepare_router(
    service: Arc<dyn MagService>,
    opts: ServeOptions,
) -> io::Result<PreparedRouter> {
    let options = resolve_serve_options(opts)?;
    let router = router_with_resolved_options(service, options.clone(), SseConfig::default());

    Ok(PreparedRouter { router, options })
}

/// Resolves auth policy, bind address, and static asset source from raw options.
///
/// # Errors
///
/// Returns an I/O error if token generation is required but the OS random source fails.
pub fn resolve_serve_options(opts: ServeOptions) -> io::Result<ResolvedServeOptions> {
    let address = SocketAddr::new(opts.host, opts.port);
    let auth_token = resolve_auth_token(opts.host, opts.token_policy)?;
    let static_assets = resolve_static_assets(opts.static_assets_dir);

    Ok(ResolvedServeOptions {
        address,
        auth_token,
        static_assets,
    })
}

/// Runs the web adapter until the HTTP server exits.
///
/// # Errors
///
/// Returns the bind or server I/O error reported by Tokio/axum.
pub async fn serve(service: Arc<dyn MagService>, opts: ServeOptions) -> io::Result<()> {
    let prepared = prepare_router(service, opts)?;
    let address = prepared.options().address();
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, prepared.into_router()).await
}

/// Builds an unauthenticated router for tests and trusted in-process callers.
pub fn router(service: Arc<dyn MagService>) -> Router {
    router_with_sse_config(service, SseConfig::default())
}

fn router_with_sse_config(service: Arc<dyn MagService>, sse: SseConfig) -> Router {
    router_with_resolved_options(service, unauthenticated_options(), sse)
}

fn router_with_resolved_options(
    service: Arc<dyn MagService>,
    options: ResolvedServeOptions,
    sse: SseConfig,
) -> Router {
    let state = AppState {
        service,
        sse,
        static_assets: options.static_assets.clone(),
    };

    let api = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/events", get(events))
        .route("/sessions/{id}/resume", post(resume_session))
        .route("/sessions/{id}", delete(delete_session))
        .route("/sessions/{id}/history", get(get_session_history))
        .route("/sessions/{id}/messages", post(send_message))
        .route("/sessions/{id}/pivot", post(pivot_message))
        .route("/sessions/{id}/cancel", post(cancel))
        .route(
            "/sessions/{id}/interactions/{request_id}",
            post(respond_interaction),
        )
        .route("/sources", get(list_sources))
        .route("/sources/probe", post(probe_local_agents))
        .route("/config", get(get_config).put(update_config))
        .route("/config/reload", post(reload_config))
        .route("/config/apply", post(apply_config))
        .fallback(api_not_found);

    let api = match options.auth_token {
        Some(token) => api.layer(middleware::from_fn_with_state(
            AuthState {
                token: Arc::from(token),
            },
            require_bearer_auth,
        )),
        None => api,
    };

    Router::new()
        .nest("/api", api)
        .fallback(static_asset)
        .with_state(state)
}

#[derive(Clone)]
struct AppState {
    service: Arc<dyn MagService>,
    sse: SseConfig,
    static_assets: StaticAssets,
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

fn unauthenticated_options() -> ResolvedServeOptions {
    ResolvedServeOptions {
        address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        auth_token: None,
        static_assets: resolve_static_assets(None),
    }
}

fn resolve_auth_token(host: IpAddr, policy: TokenPolicy) -> io::Result<Option<String>> {
    let non_loopback = !host.is_loopback();
    if non_loopback {
        eprintln!(
            "mag-web warning: binding non-loopback host {host}; bearer token auth is required"
        );
    }

    match policy {
        TokenPolicy::Generate => generate_token().map(Some),
        TokenPolicy::Provided(token) => Ok(Some(token)),
        TokenPolicy::Disabled if non_loopback => {
            eprintln!("mag-web warning: ignoring --no-auth for non-loopback host {host}");
            generate_token().map(Some)
        }
        TokenPolicy::Disabled => Ok(None),
    }
}

fn generate_token() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes)?;

    // Encode an RFC 4122 version 4 UUID without depending on another runtime crate.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

#[cfg(unix)]
fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(not(unix))]
fn fill_random(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "default token generation requires an OS random source",
    ))
}

fn resolve_static_assets(override_dir: Option<PathBuf>) -> StaticAssets {
    match override_dir {
        Some(path) => StaticAssets::Directory(path),
        None => default_static_assets(),
    }
}

#[cfg(debug_assertions)]
fn default_static_assets() -> StaticAssets {
    StaticAssets::Directory(default_static_assets_dir())
}

#[cfg(not(debug_assertions))]
fn default_static_assets() -> StaticAssets {
    StaticAssets::Embedded
}

#[cfg(debug_assertions)]
fn default_static_assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/apps/web/dist")
}

async fn require_bearer_auth(
    State(auth): State<AuthState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, AuthError> {
    if bearer_token(request.headers()) == Some(auth.token.as_ref()) {
        Ok(next.run(request).await)
    } else {
        Err(AuthError)
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

#[derive(Clone, Debug)]
struct AuthError;

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                kind: "unauthorized".to_owned(),
                message: "missing or invalid bearer token".to_owned(),
            }),
        )
            .into_response()
    }
}

async fn api_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
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

async fn static_asset(State(state): State<AppState>, uri: Uri) -> Response {
    if is_api_path(uri.path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    match static_asset_response(&state.static_assets, uri.path()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("mag-web failed to read static asset: {error}");
            ApiError::Internal.into_response()
        }
    }
}

fn static_asset_response(assets: &StaticAssets, request_path: &str) -> io::Result<Response> {
    let Some(asset_path) = normalize_asset_path(request_path) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    if let Some(bytes) = load_static_asset(assets, &asset_path)? {
        return Ok(bytes_response(&asset_path, bytes));
    }

    if let Some(bytes) = load_static_asset(assets, "index.html")? {
        return Ok(bytes_response("index.html", bytes));
    }

    Ok(placeholder_response(assets))
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn normalize_asset_path(request_path: &str) -> Option<String> {
    let trimmed = request_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_owned());
    }

    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') {
            return None;
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        Some("index.html".to_owned())
    } else {
        Some(segments.join("/"))
    }
}

fn load_static_asset(assets: &StaticAssets, path: &str) -> io::Result<Option<Vec<u8>>> {
    match assets {
        StaticAssets::Directory(root) => load_directory_asset(root, path),
        #[cfg(not(debug_assertions))]
        StaticAssets::Embedded => {
            Ok(EmbeddedAssets::get(path).map(|asset| asset.data.into_owned()))
        }
    }
}

fn load_directory_asset(root: &FsPath, path: &str) -> io::Result<Option<Vec<u8>>> {
    match fs::read(root.join(path)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) => match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::IsADirectory => Ok(None),
            _ => Err(error),
        },
    }
}

fn bytes_response(path: &str, bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type(path))],
        Body::from(bytes),
    )
        .into_response()
}

fn placeholder_response(assets: &StaticAssets) -> Response {
    let source = match assets {
        StaticAssets::Directory(path) => path.display().to_string(),
        #[cfg(not(debug_assertions))]
        StaticAssets::Embedded => "embedded release assets".to_owned(),
    };
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>mag web assets missing</title></head>
<body>
<main style="font-family: system-ui, sans-serif; max-width: 48rem; margin: 4rem auto; line-height: 1.5;">
<h1>mag web UI is not built yet</h1>
<p>No <code>index.html</code> was found in <code>{source}</code>.</p>
<p>Run <code>pnpm build</code> in <code>ui/</code>, then restart <code>mag --web</code>.</p>
</main>
</body>
</html>"#
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
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

#[cfg(not(debug_assertions))]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/apps/web/dist/"]
struct EmbeddedAssets;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{HeaderValue, Method, Request},
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

    fn request_with_token(
        method: Method,
        uri: impl AsRef<str>,
        body: Option<Value>,
        token: &str,
    ) -> Request<Body> {
        let mut request = request(method, uri, body);
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("token header is valid"),
        );
        request
    }

    fn prepared_app(
        service: Arc<ScriptedService>,
        token_policy: TokenPolicy,
        static_assets_dir: Option<PathBuf>,
    ) -> (Router, ResolvedServeOptions) {
        let prepared = prepare_router(
            service,
            ServeOptions {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                token_policy,
                static_assets_dir,
            },
        )
        .expect("router prepares");
        let options = prepared.options().clone();
        (prepared.into_router(), options)
    }

    fn token_shape_is_uuid(value: &str) -> bool {
        value.len() == 36
            && value.chars().enumerate().all(|(index, ch)| match index {
                8 | 13 | 18 | 23 => ch == '-',
                _ => ch.is_ascii_hexdigit(),
            })
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

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body readable");
        String::from_utf8(bytes.to_vec()).expect("response body is utf-8")
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
    async fn api_auth_requires_matching_bearer_token() {
        let service = Arc::new(ScriptedService::default());
        let (app, options) = prepared_app(
            service.clone(),
            TokenPolicy::Provided("secret-token".to_owned()),
            None,
        );

        assert!(options.auth_enabled());
        assert_eq!(options.auth_token(), Some("secret-token"));

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/sessions", None))
            .await
            .expect("missing auth response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_json::<Value>(response).await,
            json!({ "kind": "unauthorized", "message": "missing or invalid bearer token" })
        );

        let response = app
            .clone()
            .oneshot(request_with_token(
                Method::GET,
                "/api/sessions",
                None,
                "wrong-token",
            ))
            .await
            .expect("wrong auth response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        assert!(
            service.calls().is_empty(),
            "rejected auth must not call service methods"
        );

        let response = app
            .oneshot(request_with_token(
                Method::GET,
                "/api/sessions",
                None,
                "secret-token",
            ))
            .await
            .expect("authorized response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json::<Value>(response).await,
            json_value(vec![SessionInfo::new(session_id(), config())])
        );
        assert_eq!(service.calls(), vec![Call::ListSessions]);
    }

    #[tokio::test]
    async fn token_policy_generate_creates_uuid_shaped_token() {
        let options = resolve_serve_options(ServeOptions {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            token_policy: TokenPolicy::Generate,
            static_assets_dir: None,
        })
        .expect("options resolve");

        let token = options.auth_token().expect("generated token exists");
        assert!(token_shape_is_uuid(token), "token: {token}");
    }

    #[tokio::test]
    async fn no_auth_policy_allows_loopback_api_without_token() {
        let service = Arc::new(ScriptedService::default());
        let (app, options) = prepared_app(service.clone(), TokenPolicy::Disabled, None);

        assert!(!options.auth_enabled());

        let response = app
            .oneshot(request(Method::GET, "/api/sessions", None))
            .await
            .expect("no-auth response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(service.calls(), vec![Call::ListSessions]);
    }

    #[tokio::test]
    async fn non_loopback_no_auth_is_ignored_and_requires_generated_token() {
        let service = Arc::new(ScriptedService::default());
        let prepared = prepare_router(
            service.clone(),
            ServeOptions {
                host: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 0,
                token_policy: TokenPolicy::Disabled,
                static_assets_dir: None,
            },
        )
        .expect("router prepares");
        let options = prepared.options().clone();
        let token = options
            .auth_token()
            .expect("non-loopback no-auth resolves to a generated token")
            .to_owned();
        let app = prepared.into_router();

        assert!(options.auth_enabled());
        assert!(token_shape_is_uuid(&token), "token: {token}");

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/api/sessions", None))
            .await
            .expect("missing auth response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(service.calls().is_empty());

        let response = app
            .oneshot(request_with_token(
                Method::GET,
                "/api/sessions",
                None,
                &token,
            ))
            .await
            .expect("authorized response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(service.calls(), vec![Call::ListSessions]);
    }

    #[tokio::test]
    async fn static_assets_are_served_without_auth_and_spa_fallbacks_to_index() {
        let service = Arc::new(ScriptedService::default());
        let temp = tempfile::tempdir().expect("temp dir creates");
        let assets = temp.path();
        std::fs::create_dir(assets.join("assets")).expect("assets dir creates");
        std::fs::write(
            assets.join("index.html"),
            "<!doctype html><main>mag web shell</main>",
        )
        .expect("index writes");
        std::fs::write(assets.join("assets/app.js"), "console.log('mag');").expect("asset writes");

        let (app, options) = prepared_app(
            service.clone(),
            TokenPolicy::Provided("secret-token".to_owned()),
            Some(assets.to_path_buf()),
        );

        assert_eq!(options.static_assets_dir(), Some(assets));

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/", None))
            .await
            .expect("index response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );
        assert!(body_text(response).await.contains("mag web shell"));

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/assets/app.js", None))
            .await
            .expect("asset response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/javascript; charset=utf-8"))
        );
        assert_eq!(body_text(response).await, "console.log('mag');");

        let response = app
            .clone()
            .oneshot(request(Method::GET, "/sessions/local-route", None))
            .await
            .expect("spa fallback response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("mag web shell"));

        let response = app
            .oneshot(request(Method::GET, "/api/sessions", None))
            .await
            .expect("api still requires auth");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(service.calls().is_empty());
    }

    #[tokio::test]
    async fn missing_static_assets_return_friendly_placeholder() {
        let service = Arc::new(ScriptedService::default());
        let temp = tempfile::tempdir().expect("temp dir creates");
        let missing_assets = temp.path().join("missing-dist");
        let (app, _) = prepared_app(service, TokenPolicy::Disabled, Some(missing_assets.clone()));

        let response = app
            .oneshot(request(Method::GET, "/", None))
            .await
            .expect("placeholder response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );
        let body = body_text(response).await;
        assert!(body.contains("pnpm build"));
        assert!(body.contains(&missing_assets.display().to_string()));
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
