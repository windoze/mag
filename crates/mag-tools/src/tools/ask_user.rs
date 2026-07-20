//! The `ask_user` tool: asks the interface user a question and waits for an answer.

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::plugin::{ToolInvocation, ToolPlugin, UserInteractionRequest, UserInteractionResponse};

/// Arguments accepted by [`AskUserTool`].
#[derive(Debug, Deserialize)]
struct AskUserArgs {
    /// Question shown to the user.
    question: String,
    /// Optional fixed choices shown to the user.
    #[serde(default)]
    options: Option<Vec<String>>,
}

/// Asks the user a question through the host interface and returns the answer.
///
/// This ordinary [`ToolPlugin`] implements `docs/CLI.md` §5 P6 / decision D6:
/// the tool handler blocks on the host-provided user-interaction bridge, which
/// emits `Question` / `Choice` interactions through mag's existing
/// `IpcApproval` path. The handler races that wait against
/// [`ToolContext::cancel`] so cancelling the run promptly returns a model-visible
/// cancellation error instead of leaving the tool parked.
#[derive(Clone, Copy, Debug, Default)]
pub struct AskUserTool;

impl AskUserTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolPlugin for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: self.name().to_owned(),
            description: "Ask the user a question and wait for their answer.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "Question shown to the user."
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional fixed choices; omit for a free-form answer."
                    }
                },
                "required": ["question"]
            }),
        }
    }

    async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
        ToolResult::error("ask_user interaction bridge unavailable")
    }

    async fn invoke_with_context(&self, invocation: ToolInvocation, args: Value) -> ToolResult {
        let args: AskUserArgs = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("invalid arguments: {error}")),
        };
        let Some(bridge) = invocation.user_interaction().cloned() else {
            return ToolResult::error("ask_user interaction bridge unavailable");
        };

        let request = UserInteractionRequest::new(args.question, args.options.clone());
        let ctx = invocation.context().clone();
        let cancel = ctx.cancel.clone();
        if cancel.is_cancelled() {
            return ToolResult::error("ask_user cancelled");
        }

        let ask = tokio::spawn(async move { bridge.ask_user(ctx, request).await });
        let response = tokio::select! {
            result = ask => match result {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => return ToolResult::error(format!("ask_user failed: {error}")),
                Err(error) => return ToolResult::error(format!("ask_user task failed: {error}")),
            },
            () = cancel.cancelled() => return ToolResult::error("ask_user cancelled"),
        };
        if cancel.is_cancelled() {
            return ToolResult::error("ask_user cancelled");
        }

        match response {
            UserInteractionResponse::Answer(text) => ToolResult::text(text),
            UserInteractionResponse::Choice(index) => match args.options.as_ref() {
                Some(options) if index < options.len() => ToolResult::text(
                    json!({ "index": index, "option": options[index] }).to_string(),
                ),
                Some(options) => ToolResult::error(format!(
                    "choice index {index} is out of range for {} option(s)",
                    options.len()
                )),
                None => ToolResult::error("choice response received for a free-form question"),
            },
        }
    }
}
