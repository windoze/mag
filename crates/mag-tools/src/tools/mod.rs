//! The built-in minimal tool set (`docs/DESIGN.md` §7, `docs/CLI.md` §5 P6).
//!
//! The first version ships five tools: the read-only `read_file`, `list_dir`,
//! and `grep` (auto-allowed, path-constrained to the worktree), `shell` (gated
//! behind approval, cancellable), and `ask_user` (asks the interface user a
//! question through the host interaction bridge). Each is an independent
//! [`ToolPlugin`](crate::ToolPlugin); extending the tool surface is just adding
//! another plugin.

mod ask_user;
mod grep;
mod list_dir;
mod read_file;
mod shell;

pub use ask_user::AskUserTool;
pub use grep::GrepTool;
pub use list_dir::ListDirTool;
pub use read_file::ReadFileTool;
pub use shell::ShellTool;

use std::sync::Arc;

use crate::plugin::ToolPlugin;

/// Returns the built-in minimal tool set as shareable plugins.
///
/// The set is `read_file`, `list_dir`, `grep`, `shell`, and `ask_user`, in that
/// order. It is the default population for a [`ToolRegistry`](crate::ToolRegistry).
#[must_use]
pub fn builtin_tools() -> Vec<Arc<dyn ToolPlugin>> {
    vec![
        Arc::new(ReadFileTool::new()),
        Arc::new(ListDirTool::new()),
        Arc::new(GrepTool::new()),
        Arc::new(ShellTool::new()),
        Arc::new(AskUserTool::new()),
    ]
}
