//! The read-only `list_dir` tool.

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::path::safe_join;
use crate::plugin::ToolPlugin;

/// Arguments accepted by [`ListDirTool`].
#[derive(Debug, Deserialize)]
struct ListDirArgs {
    /// Worktree-relative directory to list; defaults to the worktree root.
    #[serde(default)]
    path: String,
}

/// Lists the entries of a directory inside the run's worktree.
///
/// The requested path is resolved with [`safe_join`]. Entries are returned one
/// per line, sorted, with a trailing `/` on sub-directories. An empty or `.`
/// path lists the worktree root.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListDirTool;

impl ListDirTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolPlugin for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: self.name().to_owned(),
            description: "List the entries of a directory relative to the worktree root."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Worktree-relative directory to list (defaults to the root)."
                    }
                }
            }),
        }
    }

    async fn invoke(&self, ctx: ToolContext, args: Value) -> ToolResult {
        let args: ListDirArgs = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("invalid arguments: {error}")),
        };
        let dir = match safe_join(ctx.worktree.path(), &args.path) {
            Ok(dir) => dir,
            Err(error) => return ToolResult::error(error.to_string()),
        };

        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(reader) => reader,
            Err(error) => {
                return ToolResult::error(format!("failed to list `{}`: {error}", args.path));
            }
        };

        let mut entries: Vec<String> = Vec::new();
        loop {
            match reader.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let is_dir = entry
                        .file_type()
                        .await
                        .map(|file_type| file_type.is_dir())
                        .unwrap_or(false);
                    entries.push(if is_dir { format!("{name}/") } else { name });
                }
                Ok(None) => break,
                Err(error) => {
                    return ToolResult::error(format!("failed to list `{}`: {error}", args.path));
                }
            }
        }

        entries.sort();
        ToolResult::text(entries.join("\n"))
    }
}
