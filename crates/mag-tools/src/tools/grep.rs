//! The read-only `grep` tool.

use std::fs;
use std::path::{Path, PathBuf};

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::path::safe_join;
use crate::plugin::ToolPlugin;

/// Maximum number of matching lines returned by a single search.
const MAX_MATCHES: usize = 1_000;

/// Arguments accepted by [`GrepTool`].
#[derive(Debug, Deserialize)]
struct GrepArgs {
    /// Literal substring to search for.
    pattern: String,
    /// Worktree-relative directory (or file) to search; defaults to the root.
    #[serde(default)]
    path: String,
}

/// Searches files under a worktree path for a literal substring.
///
/// The search is a case-sensitive substring match (no regular expressions, to
/// keep the tool dependency-free). It recurses into sub-directories without
/// following symlinks, skips non-UTF-8 files, and reports up to 1000 matching
/// lines as `relative/path:line:content`. The base path is resolved with
/// [`safe_join`] so the search cannot escape the worktree.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrepTool;

impl GrepTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolPlugin for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: self.name().to_owned(),
            description: "Search files under a worktree path for a literal substring.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Literal substring to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Worktree-relative directory or file to search (defaults to the root)."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(&self, ctx: ToolContext, args: Value) -> ToolResult {
        let args: GrepArgs = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("invalid arguments: {error}")),
        };
        if args.pattern.is_empty() {
            return ToolResult::error("pattern must not be empty");
        }
        let base = match safe_join(ctx.worktree.path(), &args.path) {
            Ok(base) => base,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let root = ctx.worktree.path().to_path_buf();

        // The recursive walk is synchronous file-system work, so it runs on the
        // blocking pool to avoid stalling the async runtime.
        let result = tokio::task::spawn_blocking(move || search(&root, &base, &args.pattern)).await;

        match result {
            Ok(Ok(matches)) if matches.is_empty() => ToolResult::text("no matches"),
            Ok(Ok(matches)) => ToolResult::text(matches.join("\n")),
            Ok(Err(error)) => ToolResult::error(error),
            Err(error) => ToolResult::error(format!("grep task failed: {error}")),
        }
    }
}

/// Searches `base` (relative to `root` for reporting), collecting matches.
fn search(root: &Path, base: &Path, pattern: &str) -> Result<Vec<String>, String> {
    let mut matches = Vec::new();
    let metadata =
        fs::symlink_metadata(base).map_err(|error| format!("failed to access path: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Ok(matches);
    }
    if metadata.is_file() {
        search_file(root, base, pattern, &mut matches);
    } else if metadata.is_dir() {
        search_dir(root, base, pattern, &mut matches);
    }
    Ok(matches)
}

/// Recursively walks `dir`, appending matches until [`MAX_MATCHES`] is reached.
fn search_dir(root: &Path, dir: &Path, pattern: &str, matches: &mut Vec<String>) {
    if matches.len() >= MAX_MATCHES {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if matches.len() >= MAX_MATCHES {
            return;
        }
        let file_type = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.file_type(),
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            search_dir(root, &path, pattern, matches);
        } else if file_type.is_file() {
            search_file(root, &path, pattern, matches);
        }
    }
}

/// Appends every matching line in `file` to `matches`.
fn search_file(root: &Path, file: &Path, pattern: &str, matches: &mut Vec<String>) {
    let contents = match fs::read_to_string(file) {
        Ok(contents) => contents,
        Err(_) => return, // Skip binary / non-UTF-8 / unreadable files.
    };
    let display = file.strip_prefix(root).unwrap_or(file).display();
    for (index, line) in contents.lines().enumerate() {
        if matches.len() >= MAX_MATCHES {
            return;
        }
        if line.contains(pattern) {
            matches.push(format!("{display}:{}:{line}", index + 1));
        }
    }
}
