#![warn(missing_docs)]

//! Pluggable tool registry and the built-in minimal tool set for mag.
//!
//! `mag` builds its tool surface from [`ToolPlugin`]s rather than hard-wiring
//! tools into the driver (`docs/DESIGN.md` §7). A [`ToolRegistry`] collects
//! plugins and produces the two static projections mag injects into an agent:
//! provider-neutral [declarations](ToolRegistry::declarations) and a
//! [`ToolSetRef`](agent_lib::agent::ToolSetRef). Because the agent-lib
//! [`agent::ToolRegistry`](agent_lib::agent::ToolRegistry) contract runs with no
//! per-call context, [`ToolRegistry::bind`] stamps run-scoped handles
//! (worktree, cancellation) onto the plugins to yield a [`PluginToolRegistry`]
//! that executes calls by dispatching to the matching plugin.
//!
//! The first-version tool set is [`ReadFileTool`], [`ListDirTool`], and
//! [`GrepTool`] (read-only, path-constrained to the worktree, auto-allowed) plus
//! [`ShellTool`] (gated behind approval, cancellable).

mod path;
mod plugin;
mod registry;
mod tools;

pub use path::{PathError, safe_join};
pub use plugin::{PermissionSpec, ToolCategory, ToolPlugin, ToolRisk};
pub use registry::{PluginToolRegistry, ToolRegistry};
pub use tools::{GrepTool, ListDirTool, ReadFileTool, ShellTool, builtin_tools};
