//! Integration tests for the mag-tools registry and built-in tool set.
//!
//! Every test is offline: it drives the tools against a `tempfile` worktree with
//! a hand-built [`ToolContextParts`], never touching the network or any real
//! agent loop.

use std::path::Path;

use agent_lib::agent::{
    AgentId, CancellationToken, RunId, ToolRegistry as AgentToolRegistry, ToolRuntimeError,
    ToolSetId, TraceHandle, TraceNodeId, WorktreeRef,
};
use agent_lib::conversation::ToolCallId;
use agent_lib::facade::ToolContextParts;
use agent_lib::model::content::ContentBlock;
use agent_lib::model::tool::{ToolCall, ToolStatus};
use mag_tools::{PermissionSpec, ToolCategory, ToolRegistry, ToolRisk};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

/// Builds run-scoped context rooted at `worktree` with the given cancel token.
fn parts(worktree: &Path, cancel: CancellationToken) -> ToolContextParts {
    let run_id = RunId::new(Uuid::from_u128(1));
    ToolContextParts {
        run_id,
        agent_id: AgentId::new(Uuid::from_u128(2)),
        worktree: WorktreeRef::new(worktree),
        cancel,
        trace: TraceHandle::new_root(TraceNodeId::new("test-root"), run_id),
    }
}

/// Builds a model tool call with a fixed provider id.
fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call-1".to_owned(),
        name: name.to_owned(),
        input,
    }
}

fn tool_call_id() -> ToolCallId {
    ToolCallId::new(Uuid::from_u128(42))
}

/// Extracts the concatenated text of a tool response's content blocks.
fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn read_file_returns_file_contents() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("hello.txt"), "hi there").expect("write file");

    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));
    let response = registry
        .execute(
            tool_call_id(),
            call("read_file", json!({ "path": "hello.txt" })),
        )
        .await
        .expect("read_file executes");

    assert_eq!(response.status, ToolStatus::Ok);
    assert_eq!(text_of(&response.content), "hi there");
}

#[tokio::test]
async fn read_file_rejects_worktree_escape() {
    let dir = TempDir::new().expect("temp dir");
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));

    let response = registry
        .execute(
            tool_call_id(),
            call("read_file", json!({ "path": "../escape.txt" })),
        )
        .await
        .expect("read_file executes");

    assert_eq!(response.status, ToolStatus::Error);
    assert!(text_of(&response.content).contains("escapes the worktree"));
}

#[tokio::test]
async fn list_dir_lists_sorted_entries() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("b.txt"), "b").expect("write b");
    std::fs::write(dir.path().join("a.txt"), "a").expect("write a");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir sub");

    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));
    let response = registry
        .execute(tool_call_id(), call("list_dir", json!({})))
        .await
        .expect("list_dir executes");

    assert_eq!(response.status, ToolStatus::Ok);
    assert_eq!(text_of(&response.content), "a.txt\nb.txt\nsub/");
}

#[tokio::test]
async fn grep_reports_matching_lines() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("one.txt"), "alpha\nbeta\nneedle here\n").expect("write one");
    std::fs::create_dir(dir.path().join("nested")).expect("mkdir");
    std::fs::write(
        dir.path().join("nested/two.txt"),
        "no match\nneedle again\n",
    )
    .expect("write two");

    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));
    let response = registry
        .execute(tool_call_id(), call("grep", json!({ "pattern": "needle" })))
        .await
        .expect("grep executes");

    assert_eq!(response.status, ToolStatus::Ok);
    let body = text_of(&response.content);
    assert!(body.contains("one.txt:3:needle here"), "body was: {body}");
    assert!(
        body.contains("nested/two.txt:2:needle again"),
        "body was: {body}"
    );
}

#[tokio::test]
async fn grep_reports_no_matches() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("one.txt"), "alpha\nbeta\n").expect("write one");

    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));
    let response = registry
        .execute(tool_call_id(), call("grep", json!({ "pattern": "zzz" })))
        .await
        .expect("grep executes");

    assert_eq!(response.status, ToolStatus::Ok);
    assert_eq!(text_of(&response.content), "no matches");
}

#[tokio::test]
async fn shell_runs_command_and_returns_stdout() {
    let dir = TempDir::new().expect("temp dir");
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));

    let response = registry
        .execute(
            tool_call_id(),
            call("shell", json!({ "command": "echo hello world" })),
        )
        .await
        .expect("shell executes");

    assert_eq!(response.status, ToolStatus::Ok);
    assert_eq!(text_of(&response.content), "hello world");
}

#[tokio::test]
async fn shell_runs_in_the_worktree() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("marker.txt"), "x").expect("write marker");
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));

    let response = registry
        .execute(tool_call_id(), call("shell", json!({ "command": "ls" })))
        .await
        .expect("shell executes");

    assert_eq!(response.status, ToolStatus::Ok);
    assert!(text_of(&response.content).contains("marker.txt"));
}

#[tokio::test]
async fn shell_reports_nonzero_exit_as_error() {
    let dir = TempDir::new().expect("temp dir");
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));

    let response = registry
        .execute(
            tool_call_id(),
            call("shell", json!({ "command": "exit 3" })),
        )
        .await
        .expect("shell executes");

    assert_eq!(response.status, ToolStatus::Error);
    assert!(text_of(&response.content).contains("status 3"));
}

#[tokio::test]
async fn shell_cancellation_interrupts_the_command() {
    let dir = TempDir::new().expect("temp dir");
    let cancel = CancellationToken::new();
    // Cancel up front; a pre-cancelled token must abort the long sleep promptly.
    cancel.cancel();
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), cancel));

    let started = std::time::Instant::now();
    let response = registry
        .execute(
            tool_call_id(),
            call("shell", json!({ "command": "sleep 30" })),
        )
        .await
        .expect("shell executes");

    assert_eq!(response.status, ToolStatus::Error);
    assert!(text_of(&response.content).contains("cancelled"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "cancellation should interrupt well before the sleep finishes"
    );
}

#[tokio::test]
async fn registry_declares_the_four_builtin_tools() {
    let registry = ToolRegistry::with_builtins();
    let names: Vec<String> = registry
        .declarations()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(names, ["read_file", "list_dir", "grep", "shell"]);
}

#[tokio::test]
async fn execute_unknown_tool_reports_unknown_tool() {
    let dir = TempDir::new().expect("temp dir");
    let registry = ToolRegistry::with_builtins().bind(parts(dir.path(), CancellationToken::new()));

    let error = registry
        .execute(tool_call_id(), call("does_not_exist", json!({})))
        .await
        .expect_err("unknown tool is rejected");

    assert_eq!(
        error,
        ToolRuntimeError::UnknownTool {
            name: "does_not_exist".to_owned()
        }
    );
}

#[test]
fn permission_metadata_matches_the_gate_policy() {
    let registry = ToolRegistry::with_builtins();

    // Read-only tools are auto-allowed (no permission spec).
    assert_eq!(registry.permission("read_file"), None);
    assert_eq!(registry.permission("list_dir"), None);
    assert_eq!(registry.permission("grep"), None);

    // Shell is gated with a conservative baseline.
    assert_eq!(
        registry.permission("shell"),
        Some(PermissionSpec::new(ToolCategory::Shell, ToolRisk::Medium))
    );

    // Unknown tools have no permission metadata.
    assert_eq!(registry.permission("nope"), None);
}

#[test]
fn tool_set_carries_the_declarations() {
    let registry = ToolRegistry::with_builtins();
    let tool_set = registry.tool_set(ToolSetId::new(Uuid::from_u128(7)));
    let names: Vec<&str> = tool_set
        .tools()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, ["read_file", "list_dir", "grep", "shell"]);
}
