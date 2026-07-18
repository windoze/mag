//! The `shell` tool: runs an external command, gated behind approval.

use std::process::Stdio;
use std::time::Duration;

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::plugin::{PermissionSpec, ToolCategory, ToolPlugin, ToolRisk};

/// How often the run loop re-checks the cancellation token while the child runs.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// Arguments accepted by [`ShellTool`].
#[derive(Debug, Deserialize)]
struct ShellArgs {
    /// The command line executed through `sh -c`.
    command: String,
}

/// Runs a shell command inside the run's worktree, interruptible via cancel.
///
/// The command is executed with `sh -c` and its working directory set to the
/// worktree root. Standard output and error are captured. The
/// [`ToolContext::cancel`] token is polled while the child runs; when it fires,
/// the child is killed and the tool returns a cancellation error. Unlike the
/// read-only tools, `shell` returns a [`PermissionSpec`] so the approval gate
/// pauses on it (`docs/DESIGN.md` §3.3).
#[derive(Clone, Copy, Debug, Default)]
pub struct ShellTool;

impl ShellTool {
    /// Creates the tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Outcome of waiting for the child process to finish.
enum WaitOutcome {
    /// The process exited (successfully or not) with this wait result.
    Exited(std::io::Result<std::process::ExitStatus>),
    /// The cancellation token fired and the process was killed.
    Cancelled,
}

#[async_trait]
impl ToolPlugin for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn declaration(&self) -> Tool {
        Tool {
            name: self.name().to_owned(),
            description: "Run a shell command in the worktree via `sh -c`.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Command line executed through `sh -c`."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(&self, ctx: ToolContext, args: Value) -> ToolResult {
        let args: ShellArgs = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(format!("invalid arguments: {error}")),
        };

        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(ctx.worktree.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return ToolResult::error(format!("failed to spawn command: {error}")),
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cancel = ctx.cancel.clone();

        // Poll the (poll-based) cancellation token while draining both pipes so a
        // cancel promptly kills the child even if it produces no output.
        let wait = async {
            loop {
                match tokio::time::timeout(CANCEL_POLL, child.wait()).await {
                    Ok(result) => return WaitOutcome::Exited(result),
                    Err(_elapsed) => {
                        if cancel.is_cancelled() {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                            return WaitOutcome::Cancelled;
                        }
                    }
                }
            }
        };

        let (outcome, out, err) = tokio::join!(wait, drain(stdout), drain(stderr));

        match outcome {
            WaitOutcome::Cancelled => ToolResult::error("shell command cancelled"),
            WaitOutcome::Exited(Err(error)) => {
                ToolResult::error(format!("failed to wait for command: {error}"))
            }
            WaitOutcome::Exited(Ok(status)) => {
                let body = combine(out, err);
                if status.success() {
                    ToolResult::text(body)
                } else {
                    let code = status
                        .code()
                        .map_or_else(|| "signal".to_owned(), |code| code.to_string());
                    ToolResult::error(format!("command exited with status {code}\n{body}"))
                }
            }
        }
    }

    fn permission(&self) -> Option<PermissionSpec> {
        Some(PermissionSpec::new(ToolCategory::Shell, ToolRisk::Medium))
    }

    fn permission_for(&self, args: &Value) -> Option<PermissionSpec> {
        let risk = args
            .get("command")
            .and_then(Value::as_str)
            .map_or(ToolRisk::Medium, command_risk);
        Some(PermissionSpec::new(ToolCategory::Shell, risk))
    }
}

/// Reads an optional child pipe to end as a UTF-8 (lossy) string.
async fn drain<R: AsyncReadExt + Unpin>(pipe: Option<R>) -> String {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buffer).await;
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Joins captured stdout and stderr into one model-visible body.
fn combine(out: String, err: String) -> String {
    match (out.is_empty(), err.is_empty()) {
        (false, false) => format!("{}\n{}", out.trim_end(), err.trim_end()),
        (false, true) => out.trim_end().to_owned(),
        (true, false) => err.trim_end().to_owned(),
        (true, true) => String::new(),
    }
}

/// Estimates the [`ToolRisk`] of a shell command from its text.
///
/// This is a deliberately conservative heuristic that honors the
/// `docs/DESIGN.md` §7 note "risk 按命令": destructive or privileged commands are
/// [`ToolRisk::High`], a small allowlist of read-only commands with no shell
/// metacharacters is [`ToolRisk::Low`], and everything else is
/// [`ToolRisk::Medium`].
fn command_risk(command: &str) -> ToolRisk {
    const DANGEROUS: &[&str] = &[
        "rm -rf", "rm -r", "sudo ", "mkfs", "dd ", ":(){", "shutdown", "reboot", "> /dev/",
        "chmod -r", "chown -r",
    ];
    const SAFE: &[&str] = &[
        "echo", "true", "pwd", "ls", "cat", "head", "tail", "date", "whoami",
    ];

    let lowered = command.to_ascii_lowercase();
    if DANGEROUS.iter().any(|needle| lowered.contains(needle)) {
        return ToolRisk::High;
    }

    let trimmed = command.trim();
    let first = trimmed.split_whitespace().next().unwrap_or_default();
    let has_metachars = trimmed.contains([';', '|', '&', '>', '<', '$', '`', '(']);
    if !has_metachars && SAFE.contains(&first) {
        return ToolRisk::Low;
    }

    ToolRisk::Medium
}

#[cfg(test)]
mod tests {
    use super::{ToolRisk, command_risk};

    #[test]
    fn classifies_command_risk_by_content() {
        assert_eq!(command_risk("echo hello"), ToolRisk::Low);
        assert_eq!(command_risk("make build"), ToolRisk::Medium);
        assert_eq!(command_risk("echo hi | grep h"), ToolRisk::Medium);
        assert_eq!(command_risk("rm -rf /tmp/x"), ToolRisk::High);
        assert_eq!(command_risk("sudo reboot"), ToolRisk::High);
    }
}
