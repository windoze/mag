//! The read-only `read_file` tool.

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::path::safe_join;
use crate::plugin::ToolPlugin;

/// Arguments accepted by [`ReadFileTool`].
#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    /// Worktree-relative path of the file to read.
    path: String,
}

/// Reads a UTF-8 text file located inside the run's worktree.
///
/// The requested path is resolved with [`safe_join`], so it cannot escape the
/// worktree. Missing files, directories, non-UTF-8 content, and traversal
/// attempts are reported as model-visible [`ToolResult::error`]s rather than
/// hard failures.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadFileTool;

impl ReadFileTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolPlugin for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: self.name().to_owned(),
            description: "Read a UTF-8 text file relative to the worktree root.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Worktree-relative path of the file to read."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn invoke(&self, ctx: ToolContext, args: Value) -> ToolResult {
        let args: ReadFileArgs = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("invalid arguments: {error}")),
        };
        let path = match safe_join(ctx.worktree.path(), &args.path) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => ToolResult::text(contents),
            Err(error) => ToolResult::error(format!("failed to read `{}`: {error}", args.path)),
        }
    }
}
