//! Scriptable, offline LLM test support for mag-core driver tests.
//!
//! The fixtures here let mag-core exercise the facade [`Agent`] path without a
//! network, real credentials, or a live provider: a [`FakeLlmClient`] replays a
//! scripted sequence of [`StreamEvent`]s and records every request it receives.

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
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent, ClientError>>, ClientError>
    {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .push(request);
        let script = self.pop_script()?;
        Ok(stream::iter(script.into_iter().map(Ok::<_, ClientError>)).boxed())
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
        client::{ChatRequest, LlmClient},
        model::{
            content::ContentBlock,
            normalized::{Normalized, StopReason},
        },
    };
    use serde_json::{Map, json};

    use super::FakeLlmClient;

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
}
