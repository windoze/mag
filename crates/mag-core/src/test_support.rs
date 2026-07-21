//! Scriptable, offline LLM test support for mag-core driver tests.
//!
//! The fixtures here let mag-core exercise the facade [`Agent`] path without a
//! network, real credentials, or a live provider: a [`FakeLlmClient`] replays a
//! scripted sequence of [`StreamEvent`]s and records every request it receives.

use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex},
};

use agent_lib::{
    client::{
        ANTHROPIC_DEFAULT_CAPABILITY, Capability, ChatRequest, ClientError, LlmClient, Response,
    },
    model::{
        content::ContentBlock,
        message::{Message, Role},
        normalized::{Normalized, StopReason},
        usage::Usage,
    },
    stream::{
        BlockId, BlockKind, Delta, StreamEvent,
        accumulator::{CollectError, collect},
    },
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::Value;

/// One scripted response the [`FakeLlmClient`] hands to the facade.
///
/// [`StreamScript::Complete`] yields its events and then ends the stream, while
/// [`StreamScript::Stall`] yields its events and then pends forever. The latter
/// models a run that is still in progress so a cancellation can land mid-stream;
/// dropping the facade stream tears the pending tail down cleanly.
#[derive(Clone, Debug)]
pub(crate) enum StreamScript {
    /// Yield the events, then terminate the stream normally.
    Complete(Vec<StreamEvent>),
    /// Yield the events, then pend forever (the run never completes on its own).
    Stall(Vec<StreamEvent>),
    /// Yield `head`, wait for `gate` to open, then yield `tail` and end the
    /// stream: a run parked mid-response so a pivot can be queued while no
    /// step boundary remains (`docs/CLI.md` §3.2).
    Gated {
        /// Events yielded before the gate.
        head: Vec<StreamEvent>,
        /// Gate holding back the tail events.
        gate: Arc<StreamGate>,
        /// Events yielded once the gate opens.
        tail: Vec<StreamEvent>,
    },
}

impl StreamScript {
    /// Returns the concrete events the script begins with.
    fn events(self) -> Vec<StreamEvent> {
        match self {
            Self::Complete(events) | Self::Stall(events) => events,
            // The non-streaming `chat` endpoint folds the whole response at
            // once, so the gate is meaningless there and the script degrades
            // to its plain concatenation.
            Self::Gated { head, tail, .. } => head.into_iter().chain(tail).collect(),
        }
    }
}

/// Test gate holding a scripted stream (or a stub tool) back until the test
/// releases it, giving cross-thread commands such as pivot or cancel a
/// deterministic window to land.
#[derive(Debug)]
pub(crate) struct StreamGate {
    permits: tokio::sync::Semaphore,
}

impl StreamGate {
    /// Creates a closed gate.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            permits: tokio::sync::Semaphore::new(0),
        })
    }

    /// Releases one waiter (or the next one to arrive).
    pub(crate) fn open(&self) {
        self.permits.add_permits(1);
    }

    /// Waits until [`open`](Self::open) releases the gate.
    pub(crate) async fn wait(&self) {
        let _ = self.permits.acquire().await;
    }
}

/// Offline scripted [`LlmClient`] fixture.
///
/// Scripts are consumed FIFO from one shared queue by default. When several
/// agents share the client — a supervisor and the child instances it spawns
/// through the `agent` tool (`docs/dyn-agents.md` §5) — their requests
/// interleave in an order the test does not control, so a flat FIFO queue is
/// not expressible. [`FakeLlmClient::scripted_routes`] instead matches each
/// request to a [`RequestRoute`] by content (system prompt marker, user text),
/// keeping per-agent script queues deterministic.
#[derive(Debug)]
pub(crate) struct FakeLlmClient {
    scripts: Mutex<VecDeque<StreamScript>>,
    routes: Mutex<Vec<RequestRoute>>,
    chat_requests: Mutex<Vec<ChatRequest>>,
    stream_requests: Mutex<Vec<ChatRequest>>,
}

impl FakeLlmClient {
    /// Creates a fake client from raw stream event scripts.
    ///
    /// Each script yields its events and then ends the stream; use
    /// [`FakeLlmClient::scripted_streams`] for scripts that stall instead.
    pub(crate) fn scripted(scripts: Vec<Vec<StreamEvent>>) -> Arc<Self> {
        Self::scripted_streams(scripts.into_iter().map(StreamScript::Complete).collect())
    }

    /// Creates a fake client from explicit [`StreamScript`]s.
    pub(crate) fn scripted_streams(scripts: Vec<StreamScript>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            routes: Mutex::new(Vec::new()),
            chat_requests: Mutex::new(Vec::new()),
            stream_requests: Mutex::new(Vec::new()),
        })
    }

    /// Creates a fake client that routes every request to a script queue by
    /// content (see [`RequestRoute`]).
    pub(crate) fn scripted_routes(routes: Vec<RequestRoute>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(VecDeque::new()),
            routes: Mutex::new(routes),
            chat_requests: Mutex::new(Vec::new()),
            stream_requests: Mutex::new(Vec::new()),
        })
    }

    /// Creates a fake client with one tool-use response stream.
    pub(crate) fn tool_use(tool_name: &str, tool_call_id: &str, input: Value) -> Arc<Self> {
        Self::scripted(vec![tool_use_stream(tool_name, tool_call_id, input)])
    }

    /// Returns requests made through the complete-response endpoint.
    pub(crate) fn chat_requests(&self) -> Vec<ChatRequest> {
        self.chat_requests
            .lock()
            .expect("chat requests lock")
            .clone()
    }

    /// Returns requests made through the streaming endpoint.
    pub(crate) fn stream_requests(&self) -> Vec<ChatRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .clone()
    }

    fn pop_script(&self, request: &ChatRequest) -> Result<StreamScript, ClientError> {
        let mut routes = self.routes.lock().expect("fake llm routes lock");
        if !routes.is_empty() {
            for route in routes.iter_mut() {
                if (route.matches)(request) {
                    return route.scripts.pop_front().ok_or_else(|| {
                        ClientError::Other("fake LLM route scripts exhausted".to_owned())
                    });
                }
            }
            return Err(ClientError::Other(
                "no fake LLM route matched the request".to_owned(),
            ));
        }
        drop(routes);
        self.scripts
            .lock()
            .expect("fake llm script lock")
            .pop_front()
            .ok_or_else(|| ClientError::Other("fake LLM script exhausted".to_owned()))
    }
}

/// A content-keyed script route of a [`FakeLlmClient`].
///
/// Routes are evaluated in order; the first route whose matcher accepts the
/// request supplies its next script. This lets one fake client interleave a
/// supervisor's and its spawned child instances' requests deterministically:
/// route the children by their layered system prompt (or opening task brief)
/// and keep a final [`RequestRoute::any`] catch-all for the supervisor.
pub(crate) struct RequestRoute {
    matches: Box<dyn Fn(&ChatRequest) -> bool + Send + Sync>,
    scripts: VecDeque<StreamScript>,
}

impl fmt::Debug for RequestRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestRoute")
            .field("pending_scripts", &self.scripts.len())
            .finish_non_exhaustive()
    }
}

impl RequestRoute {
    /// Creates a route from an arbitrary request predicate.
    pub(crate) fn new(
        matches: impl Fn(&ChatRequest) -> bool + Send + Sync + 'static,
        scripts: Vec<StreamScript>,
    ) -> Self {
        Self {
            matches: Box::new(matches),
            scripts: scripts.into(),
        }
    }

    /// A catch-all route; useful as the last route for the supervisor's own
    /// requests once the children are routed by content.
    pub(crate) fn any(scripts: Vec<StreamScript>) -> Self {
        Self::new(|_| true, scripts)
    }

    /// Routes requests whose system prompt contains `marker` — the subagent
    /// skeleton distinctly marks a spawned child's requests
    /// (`docs/dyn-agents.md` §4).
    pub(crate) fn system_contains(marker: &str, scripts: Vec<StreamScript>) -> Self {
        let marker = marker.to_owned();
        Self::new(
            move |request| {
                request
                    .system
                    .as_deref()
                    .is_some_and(|system| system.contains(&marker))
            },
            scripts,
        )
    }

    /// Routes requests carrying any user message whose text contains
    /// `marker` — a spawned child's opening user message is its task brief
    /// (`docs/dyn-agents.md` §5.1).
    pub(crate) fn user_text_contains(marker: &str, scripts: Vec<StreamScript>) -> Self {
        let marker = marker.to_owned();
        Self::new(
            move |request| {
                request.messages.iter().any(|message| {
                    message.role == Role::User && message_text(message).contains(&marker)
                })
            },
            scripts,
        )
    }
}

/// Concatenates the text blocks of one message (for [`RequestRoute`] matchers).
fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
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
            .push(request.clone());
        let events = self.pop_script(&request)?.events();
        collect(stream::iter(events.into_iter().map(Ok::<_, ClientError>)))
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
            .push(request.clone());
        match self.pop_script(&request)? {
            StreamScript::Complete(events) => {
                Ok(stream::iter(events.into_iter().map(Ok::<_, ClientError>)).boxed())
            }
            StreamScript::Stall(events) => Ok(stream::iter(events.into_iter().map(Ok))
                .chain(stream::pending::<Result<StreamEvent, ClientError>>())
                .boxed()),
            StreamScript::Gated { head, gate, tail } => {
                let gated_tail = stream::once(async move {
                    gate.wait().await;
                    stream::iter(tail.into_iter().map(Ok::<_, ClientError>))
                })
                .flatten();
                Ok(stream::iter(head.into_iter().map(Ok::<_, ClientError>))
                    .chain(gated_tail)
                    .boxed())
            }
        }
    }
}

/// Builds a complete text stream with explicit usage accounting.
pub(crate) fn text_stream_with_usage(chunks: &[&str], usage: Usage) -> Vec<StreamEvent> {
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

/// Builds a stalling text stream: the given chunks are emitted as live text
/// deltas and then the stream pends forever without a terminal `MessageStop`.
///
/// This models a run that is still in progress after producing some output, so a
/// cancellation can land while the facade stream is parked. The block is left
/// open on purpose; dropping the facade stream tears the pending tail down.
pub(crate) fn stalling_text_stream(chunks: &[&str]) -> StreamScript {
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
    StreamScript::Stall(events)
}

/// Builds a gated text stream: the given chunks are emitted as live text
/// deltas, then the stream parks on `gate` before closing the block with
/// `usage` and `MessageStop`.
///
/// This models a run parked mid-response with no further step boundary, so a
/// pivot queued from another thread can never land and must be reported as
/// [`PivotDropped`](mag_service::ServiceEvent::PivotDropped) once the gate
/// opens and the run finishes (`docs/CLI.md` §3.2).
pub(crate) fn gated_text_stream(
    chunks: &[&str],
    gate: Arc<StreamGate>,
    usage: Usage,
) -> StreamScript {
    let block_id = BlockId::new("text-1");
    let mut head = vec![
        StreamEvent::MessageStart {
            role: Role::Assistant,
        },
        StreamEvent::BlockStart {
            id: block_id.clone(),
            kind: BlockKind::Text,
        },
    ];
    head.extend(chunks.iter().map(|chunk| StreamEvent::BlockDelta {
        id: block_id.clone(),
        delta: Delta::Text((*chunk).to_owned()),
    }));
    let tail = vec![
        StreamEvent::BlockStop { id: block_id },
        StreamEvent::Usage(usage),
        StreamEvent::MessageStop {
            stop_reason: Normalized::from_mapped(StopReason::EndTurn, "end_turn"),
        },
    ];
    StreamScript::Gated { head, gate, tail }
}

/// Builds a complete tool-use response stream.
pub(crate) fn tool_use_stream(
    tool_name: &str,
    tool_call_id: &str,
    input: Value,
) -> Vec<StreamEvent> {
    let block_id = BlockId::new("tool-1");
    vec![
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
    ]
}

fn collect_error_to_client_error(error: CollectError<ClientError>) -> ClientError {
    match error {
        CollectError::Stream(error) => error,
        CollectError::Accumulator(error) => match error {
            agent_lib::stream::accumulator::AccumulatorError::Stream(error) => error,
            other => ClientError::Protocol(other.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use agent_lib::{
        client::{ChatRequest, ClientError, LlmClient},
        model::{
            content::ContentBlock,
            message::{Message, Role},
            normalized::{Normalized, StopReason},
        },
    };
    use serde_json::{Map, json};

    use super::{FakeLlmClient, RequestRoute, StreamScript, text_stream_with_usage};

    fn request() -> ChatRequest {
        ChatRequest {
            model: "fake-model".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
            system: None,
            max_tokens: 32,
            temperature: None,
            stream: false,
            provider_extras: None,
        }
    }

    #[tokio::test]
    async fn fake_client_scripts_tool_use_responses() {
        let client = FakeLlmClient::tool_use("get_weather", "call-1", json!({ "city": "Paris" }));

        let response = client.chat(request()).await.expect("tool response");

        assert_eq!(
            response.message.content,
            vec![ContentBlock::ToolUse {
                id: "call-1".to_owned(),
                name: "get_weather".to_owned(),
                input: json!({ "city": "Paris" }),
                extra: Map::new(),
            }]
        );
        assert_eq!(
            response.stop_reason,
            Normalized::from_mapped(StopReason::ToolUse, "tool_use")
        );
        assert_eq!(client.chat_requests().len(), 1);
        assert!(client.stream_requests().is_empty());
    }

    fn routed_client() -> std::sync::Arc<FakeLlmClient> {
        FakeLlmClient::scripted_routes(vec![
            RequestRoute::system_contains(
                "SUBAGENT",
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["child"],
                    Default::default(),
                ))],
            ),
            RequestRoute::user_text_contains(
                "brief",
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["briefed"],
                    Default::default(),
                ))],
            ),
            RequestRoute::any(vec![StreamScript::Complete(text_stream_with_usage(
                &["supervisor"],
                Default::default(),
            ))]),
        ])
    }

    fn text_of(response: &agent_lib::client::Response) -> &str {
        match &response.message.content[0] {
            ContentBlock::Text { text, .. } => text,
            other => panic!("expected a text block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn routes_match_by_system_marker_then_user_text_then_fallback() {
        let client = routed_client();

        let mut child_request = request();
        child_request.system = Some("prefix SUBAGENT suffix".to_owned());
        let response = client.chat(child_request).await.expect("child response");
        assert_eq!(text_of(&response), "child");

        let mut brief_request = request();
        brief_request.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "the brief".to_owned(),
                extra: Map::new(),
            }],
        });
        let response = client.chat(brief_request).await.expect("brief response");
        assert_eq!(text_of(&response), "briefed");

        let response = client.chat(request()).await.expect("fallback response");
        assert_eq!(text_of(&response), "supervisor");
    }

    #[tokio::test]
    async fn exhausted_route_and_unmatched_request_error() {
        let client = FakeLlmClient::scripted_routes(vec![RequestRoute::system_contains(
            "SUBAGENT",
            Vec::new(),
        )]);

        let mut child_request = request();
        child_request.system = Some("SUBAGENT".to_owned());
        let error = client
            .chat(child_request)
            .await
            .expect_err("an empty route queue errors");
        assert!(
            matches!(&error, ClientError::Other(message) if message.contains("exhausted")),
            "unexpected error: {error}"
        );

        let error = client
            .chat(request())
            .await
            .expect_err("no route matches a request without the marker");
        assert!(
            matches!(&error, ClientError::Other(message) if message.contains("no fake LLM route")),
            "unexpected error: {error}"
        );
    }
}
