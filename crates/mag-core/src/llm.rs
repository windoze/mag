//! LLM handlers used by the mag engine driver.

use std::sync::Arc;

use agent_lib::{
    agent::{LlmHandler, LlmStepMode, RequirementResult, RunContext},
    client::{ChatRequest, ClientError, LlmClient, Response},
    stream::{
        Delta, StreamEvent,
        accumulator::{Accumulator, AccumulatorError},
    },
};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use mag_protocol::{Event, SessionId};

use crate::EventBus;

/// Streams one LLM generation while tapping visible text deltas into the event bus.
#[derive(Clone)]
pub struct StreamingTapHandler {
    client: Arc<dyn LlmClient>,
    events: EventBus,
    session_id: SessionId,
}

impl StreamingTapHandler {
    /// Creates a handler for `session_id` backed by `client` and `events`.
    #[must_use]
    pub fn new(client: Arc<dyn LlmClient>, events: EventBus, session_id: SessionId) -> Self {
        Self {
            client,
            events,
            session_id,
        }
    }

    /// Folds a streaming response into a complete [`Response`].
    ///
    /// Every assistant-visible text delta is emitted as [`Event::TextDelta`]
    /// before the event is validated and folded into the accumulator.
    ///
    /// # Errors
    ///
    /// Returns provider transport errors directly. Stream-shape violations from
    /// the accumulator are reported as [`ClientError::Protocol`].
    pub async fn fold(
        &self,
        mut stream: BoxStream<'static, Result<StreamEvent, ClientError>>,
    ) -> Result<Response, ClientError> {
        let mut accumulator = Accumulator::new();

        while let Some(item) = stream.next().await {
            let event = item?;
            if let StreamEvent::BlockDelta {
                delta: Delta::Text(text),
                ..
            } = &event
            {
                self.emit_text_delta(text.clone());
            }
            accumulator
                .push(event)
                .map_err(accumulator_error_to_client_error)?;
        }

        accumulator
            .finish()
            .map_err(accumulator_error_to_client_error)
    }

    fn emit_text_delta(&self, text: String) {
        let _ = self.events.emit(Event::TextDelta {
            id: self.session_id,
            text,
        });
    }
}

#[async_trait]
impl LlmHandler for StreamingTapHandler {
    async fn fulfill(
        &self,
        request: &ChatRequest,
        _mode: LlmStepMode,
        _ctx: &RunContext,
    ) -> RequirementResult {
        let mut request = request.clone();
        request.stream = true;

        let result = match self.client.chat_stream(request).await {
            Ok(stream) => self.fold(stream).await,
            Err(error) => Err(error),
        };

        RequirementResult::Llm(result)
    }
}

fn accumulator_error_to_client_error(error: AccumulatorError) -> ClientError {
    match error {
        AccumulatorError::Stream(error) => error,
        other => ClientError::Protocol(other.to_string()),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Scriptable LLM test support for mag-core driver tests.

    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use agent_lib::{
        client::{
            ANTHROPIC_DEFAULT_CAPABILITY, Capability, ChatRequest, ClientError, LlmClient, Response,
        },
        model::{
            message::Role,
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

    /// Offline scripted [`LlmClient`] fixture.
    #[derive(Debug)]
    pub(crate) struct FakeLlmClient {
        scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
        chat_requests: Mutex<Vec<ChatRequest>>,
        stream_requests: Mutex<Vec<ChatRequest>>,
    }

    impl FakeLlmClient {
        /// Creates a fake client from raw stream event scripts.
        pub(crate) fn scripted(scripts: Vec<Vec<StreamEvent>>) -> Arc<Self> {
            Arc::new(Self {
                scripts: Mutex::new(scripts.into()),
                chat_requests: Mutex::new(Vec::new()),
                stream_requests: Mutex::new(Vec::new()),
            })
        }

        /// Creates a fake client with one assistant text response stream.
        pub(crate) fn text(chunks: &[&str]) -> Arc<Self> {
            Self::scripted(vec![text_stream(chunks)])
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

        fn pop_script(&self) -> Result<Vec<StreamEvent>, ClientError> {
            self.scripts
                .lock()
                .expect("fake llm script lock")
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
            let script = self.pop_script()?;
            collect(stream::iter(script.into_iter().map(Ok::<_, ClientError>)))
                .await
                .map_err(collect_error_to_client_error)
        }

        async fn chat_stream(
            &self,
            request: ChatRequest,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<StreamEvent, ClientError>>,
            ClientError,
        > {
            self.stream_requests
                .lock()
                .expect("stream requests lock")
                .push(request);
            let script = self.pop_script()?;
            Ok(stream::iter(script.into_iter().map(Ok::<_, ClientError>)).boxed())
        }
    }

    /// Builds a complete text stream with default usage accounting.
    pub(crate) fn text_stream(chunks: &[&str]) -> Vec<StreamEvent> {
        text_stream_with_usage(chunks, Usage::default())
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
}

#[cfg(test)]
mod stream {
    use agent_lib::{
        agent::{BudgetLimits, LlmHandler, LlmStepMode, RequirementResult, RunContext},
        client::LlmClient,
        model::{
            content::ContentBlock,
            message::Role,
            normalized::{Normalized, StopReason},
            usage::Usage,
        },
    };
    use futures::StreamExt as _;
    use mag_protocol::{Event, SessionId};
    use serde_json::{Map, json};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::{
        EventBus, StreamingTapHandler,
        ids::MagIds,
        llm::test_support::{FakeLlmClient, text_stream, text_stream_with_usage},
    };

    fn session_id() -> SessionId {
        SessionId::new(Uuid::from_u128(1))
    }

    fn request(stream: bool) -> agent_lib::client::ChatRequest {
        agent_lib::client::ChatRequest {
            model: "fake-model".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
            system: None,
            max_tokens: 32,
            temperature: None,
            stream,
            provider_extras: None,
        }
    }

    fn context() -> RunContext {
        let ids = MagIds::new();
        RunContext::new_root(ids.run_id(), BudgetLimits::default(), ids.trace_root())
    }

    async fn next_event(events: &mut crate::EventStream) -> Event {
        timeout(Duration::from_secs(1), events.next())
            .await
            .expect("event timed out")
            .expect("event stream closed")
    }

    #[tokio::test]
    async fn fold_emits_text_deltas_and_returns_complete_response() {
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let handler =
            StreamingTapHandler::new(FakeLlmClient::text(&["Hel", "lo"]), bus, session_id());
        let stream = futures::stream::iter(
            text_stream_with_usage(
                &["Hel", "lo"],
                Usage {
                    input: 3,
                    output: 2,
                    ..Usage::default()
                },
            )
            .into_iter()
            .map(Ok::<_, agent_lib::client::ClientError>),
        )
        .boxed();

        let response = handler.fold(stream).await.expect("fold response");

        assert_eq!(
            response.message.content,
            vec![ContentBlock::Text {
                text: "Hello".to_owned(),
                extra: Map::new(),
            }]
        );
        assert_eq!(response.message.role, Role::Assistant);
        assert_eq!(
            response.stop_reason,
            Normalized::from_mapped(StopReason::EndTurn, "end_turn")
        );
        assert_eq!(response.usage.input, 3);
        assert_eq!(response.usage.output, 2);
        assert_eq!(
            next_event(&mut events).await,
            Event::TextDelta {
                id: session_id(),
                text: "Hel".to_owned(),
            }
        );
        assert_eq!(
            next_event(&mut events).await,
            Event::TextDelta {
                id: session_id(),
                text: "lo".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn fulfill_forces_streaming_and_returns_llm_result() {
        let client = FakeLlmClient::scripted(vec![text_stream(&["go"])]);
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let handler = StreamingTapHandler::new(client.clone(), bus, session_id());

        let RequirementResult::Llm(result) = handler
            .fulfill(&request(false), LlmStepMode::NonStreaming, &context())
            .await
        else {
            panic!("unexpected requirement result");
        };
        let response = result.expect("llm result");

        assert_eq!(
            response.message.content,
            vec![ContentBlock::Text {
                text: "go".to_owned(),
                extra: Map::new(),
            }]
        );
        assert_eq!(
            client.stream_requests(),
            vec![agent_lib::client::ChatRequest {
                stream: true,
                ..request(false)
            }]
        );
        assert!(client.chat_requests().is_empty());
        assert_eq!(
            next_event(&mut events).await,
            Event::TextDelta {
                id: session_id(),
                text: "go".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn fake_client_scripts_tool_use_responses() {
        let client = FakeLlmClient::tool_use("get_weather", "call-1", json!({ "city": "Paris" }));

        let response = client.chat(request(false)).await.expect("tool response");

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
    }
}
