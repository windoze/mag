//! The [`ToolPlugin`] extension point and its approval metadata.
//!
//! `mag` builds its tool surface from plugins rather than hard-wiring tools into
//! the driver (`docs/DESIGN.md` §7). A plugin owns three responsibilities:
//!
//! - a provider-neutral [`declaration`](ToolPlugin::declaration) advertised to
//!   the model (name / description / JSON input schema, mirroring
//!   [`agent_lib::facade::Tool::function_with_schema`]),
//! - an async [`invoke`](ToolPlugin::invoke) that runs the tool against a
//!   run-scoped [`ToolContext`] (worktree / cancellation / call id), and
//! - optional [`permission`](ToolPlugin::permission) metadata that decides
//!   whether the tool passes through the approval gate (`docs/DESIGN.md` §3.3,
//!   §8.1).
//!
//! Registering a new tool is therefore just adding a `ToolPlugin`; the driver
//! and protocol are untouched (`docs/DESIGN.md` §8.3).

use agent_lib::facade::{ToolContext, ToolResult};
use agent_lib::model::tool::Tool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Coarse classification of what a tool acts on, used by the approval gate and
/// surfaced to the UI / future AI-permission policy (`docs/DESIGN.md` §8.1).
///
/// Only the categories currently produced by the built-in tool set are modeled;
/// new categories are added as new plugins need them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// The tool runs an external shell command.
    Shell,
}

/// Ordered risk estimate carried by a [`PermissionSpec`].
///
/// The ordering (`Low < Medium < High`) is meaningful: a future rule- or
/// LLM-based [`PermissionDecider`](crate) can compare a request's risk against a
/// threshold, and the UI can render escalating severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    /// A trivially safe action (for example printing a constant).
    Low,
    /// A potentially state-changing but ordinary action.
    Medium,
    /// A destructive or privileged action that warrants extra scrutiny.
    High,
}

/// Approval metadata attached to a tool that must pass the approval gate.
///
/// A tool that returns `Some(PermissionSpec)` from
/// [`ToolPlugin::permission`] is gated (the approval handler is consulted); a
/// tool that returns `None` is auto-allowed. The `category` / `risk` fields are
/// carried through to the UI and the future AI-permission policy and are fully
/// `serde` so they can be fed to a model verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSpec {
    /// What the tool acts on.
    pub category: ToolCategory,
    /// How risky the (current) invocation is judged to be.
    pub risk: ToolRisk,
}

impl PermissionSpec {
    /// Creates a permission specification from a category and risk level.
    #[must_use]
    pub const fn new(category: ToolCategory, risk: ToolRisk) -> Self {
        Self { category, risk }
    }
}

/// A pluggable tool contributed to the [`ToolRegistry`](crate::ToolRegistry).
///
/// Implementors are cheap, stateless descriptors: the run-scoped state (which
/// worktree, which cancellation token, which call id) arrives per-invocation via
/// the [`ToolContext`], never as mutable plugin state, so a plugin can be shared
/// across sessions behind an `Arc`.
#[async_trait]
pub trait ToolPlugin: Send + Sync + fmt::Debug {
    /// Returns the tool name advertised to the model.
    ///
    /// This must equal `self.declaration().name`; the registry dispatches
    /// execution by matching a model tool call against this name.
    fn name(&self) -> &str;

    /// Returns the provider-neutral declaration advertised to the model.
    ///
    /// The declaration carries the tool name, human description, and JSON input
    /// schema (the same shape produced by
    /// [`agent_lib::facade::Tool::function_with_schema`]).
    fn declaration(&self) -> Tool;

    /// Runs one tool call against the run-scoped `ctx` and JSON `args`.
    ///
    /// The returned [`ToolResult`] carries the model-visible content and an
    /// explicit status; a handler reports recoverable failures (bad arguments,
    /// I/O errors, cancellation) as [`ToolResult::error`] rather than panicking,
    /// so the model can react to them.
    async fn invoke(&self, ctx: ToolContext, args: Value) -> ToolResult;

    /// Returns the approval metadata for this tool, or `None` to auto-allow it.
    ///
    /// This is the static, declaration-time baseline. Tools whose risk depends
    /// on the concrete arguments override [`permission_for`](Self::permission_for).
    fn permission(&self) -> Option<PermissionSpec> {
        None
    }

    /// Returns the approval metadata refined for a specific `args` payload.
    ///
    /// The default returns the static [`permission`](Self::permission). A tool
    /// such as `shell` overrides this to raise the [`ToolRisk`] for dangerous
    /// commands (`docs/DESIGN.md` §7, "risk 按命令"), giving the approval gate
    /// and UI a per-call risk estimate without changing the gating decision.
    fn permission_for(&self, _args: &Value) -> Option<PermissionSpec> {
        self.permission()
    }
}
