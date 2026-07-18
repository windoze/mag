//! Single-turn driver wiring mag sessions to the agent-lib machine.

use std::{num::NonZeroU32, sync::Arc};

use agent_lib::{
    agent::{
        AgentError, AgentInput, AgentSpec, AgentState, BudgetLimits, DefaultAgentMachine,
        HandlerScope, LlmHandler, LlmStepMode, LoopCursor, LoopPolicy, ModelRef, RunContext,
        ToolFailurePolicy, ToolSetRef, WorktreeRef, drain,
    },
    client::LlmClient,
    conversation::{Conversation, ConversationConfig},
    model::{
        content::ContentBlock,
        message::{Message, Role},
        usage::Usage,
    },
};
use mag_protocol::{Event, RunId as WireRunId, RunOutput, SessionConfig, SessionId, UsageInfo};

use crate::{EventBus, MagIds, StreamingTapHandler};

const DEFAULT_MAX_TOKENS: u32 = 512;
const DEFAULT_MAX_STEPS: u32 = 8;
const DEFAULT_MAX_PARALLEL_TOOLS: u32 = 1;

/// One session's stateful agent-lib machine and identity source.
#[derive(Debug)]
pub(crate) struct SessionDriver {
    ids: Arc<MagIds>,
    machine: DefaultAgentMachine,
}

impl SessionDriver {
    /// Builds a fresh agent machine for the supplied session configuration.
    pub(crate) fn new(config: &SessionConfig) -> Self {
        let ids = Arc::new(MagIds::new());
        let spec = AgentSpec::new(
            ids.agent_id(),
            WorktreeRef::new("."),
            None,
            ToolSetRef::new(ids.tool_set_id(), Vec::new()),
            ModelRef::new(
                config.model.clone(),
                non_zero(DEFAULT_MAX_TOKENS),
                None,
                None,
            ),
            LoopPolicy::new(
                non_zero(DEFAULT_MAX_STEPS),
                non_zero(DEFAULT_MAX_PARALLEL_TOOLS),
                ToolFailurePolicy::ReturnErrorToModel,
            ),
        );
        let state = AgentState::new(
            spec,
            Conversation::new(ids.conversation_id(), ConversationConfig::new(None)),
        );
        let machine = DefaultAgentMachine::new(state, LlmStepMode::Streaming, ids.clone())
            .with_tool_execution_ids(ids.clone());

        Self { ids, machine }
    }

    /// Drives one user message through agent-lib and emits run lifecycle events.
    pub(crate) async fn send_message(
        &mut self,
        session_id: SessionId,
        text: String,
        client: Arc<dyn LlmClient>,
        events: EventBus,
    ) -> Result<RunOutput, AgentError> {
        let run_id = self.ids.run_id();
        let wire_run_id = WireRunId::new(run_id.into_uuid());
        let ctx = RunContext::new_root(run_id, BudgetLimits::unbounded(), self.ids.trace_root());
        let input = AgentInput::user_message(
            self.ids.turn_id(),
            self.ids.message_id(),
            user_text_message(text),
            self.ids.message_id(),
            self.ids.step_id(),
        )?;
        let scope = MagScope::new(client, events.clone(), session_id);

        let _ = events.emit(Event::RunStarted {
            id: session_id,
            run_id: wire_run_id,
        });

        let done = drain(&mut self.machine, input, &scope, None, &ctx).await?;
        if let LoopCursor::Error(error) = done.cursor() {
            return Err(AgentError::Other(error.message().to_owned()));
        }

        let output = run_output(self.machine.state().conversation());
        let _ = events.emit(Event::RunFinished {
            id: session_id,
            output: output.clone(),
        });
        Ok(output)
    }
}

/// Minimal handler scope for C1: only LLM effects are fulfilled.
struct MagScope {
    llm: StreamingTapHandler,
}

impl MagScope {
    fn new(client: Arc<dyn LlmClient>, events: EventBus, session_id: SessionId) -> Self {
        Self {
            llm: StreamingTapHandler::new(client, events, session_id),
        }
    }
}

impl HandlerScope for MagScope {
    fn llm(&self) -> Option<&dyn LlmHandler> {
        Some(&self.llm)
    }
}

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("driver constants are non-zero")
}

fn user_text_message(text: String) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text,
            extra: Default::default(),
        }],
    }
}

fn run_output(conversation: &Conversation) -> RunOutput {
    let Some(turn) = conversation.turns().last() else {
        return RunOutput {
            text: String::new(),
            usage: None,
        };
    };

    RunOutput {
        text: last_assistant_text(conversation),
        usage: Some(usage_info(turn.meta().usage())),
    }
}

fn last_assistant_text(conversation: &Conversation) -> String {
    let Some(turn) = conversation.turns().last() else {
        return String::new();
    };
    let Some(message) = turn.messages().last() else {
        return String::new();
    };

    message
        .payload()
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn usage_info(usage: &Usage) -> UsageInfo {
    UsageInfo {
        input_tokens: u64::from(usage.input),
        output_tokens: u64::from(usage.output),
        total_tokens: u64::from(usage.total.unwrap_or_else(|| usage.total_computed())),
    }
}
