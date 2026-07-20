//! The mag-side tool registry and its bound runtime projection.
//!
//! [`ToolRegistry`] is the transport-agnostic collector: it gathers
//! [`ToolPlugin`]s and produces the static projections mag needs to inject tools
//! into an agent — the provider-neutral [`declarations`](ToolRegistry::declarations)
//! and a [`ToolSetRef`] (`docs/DESIGN.md` §7). Because the
//! [`agent::ToolRegistry`](agent_lib::agent::ToolRegistry) contract executes with
//! no per-call context, execution needs the run-scoped handles (worktree,
//! cancellation) up front: [`ToolRegistry::bind`] stamps a
//! [`ToolContextParts`] onto the plugins to yield a [`PluginToolRegistry`] that
//! implements the agent-lib trait and dispatches each call to the matching
//! plugin by name. Tools such as `ask_user` can additionally receive a
//! host-provided user-interaction bridge via
//! [`ToolRegistry::bind_with_user_interaction`] (`docs/CLI.md` §5 P6).

use std::{fmt, sync::Arc};

use agent_lib::agent::{
    ToolRegistry as AgentToolRegistry, ToolRuntimeError, ToolSetId, ToolSetRef,
};
use agent_lib::conversation::ToolCallId;
use agent_lib::facade::{ToolContext, ToolContextParts};
use agent_lib::model::tool::{Tool, ToolCall, ToolResponse};
use async_trait::async_trait;

use crate::plugin::{PermissionSpec, ToolInvocation, ToolPlugin, UserInteractionBridge};
use crate::tools::builtin_tools;

/// A collection of [`ToolPlugin`]s assembled by mag.
///
/// The registry is transport-agnostic and carries no run-scoped state; it is the
/// single place tools are gathered before being projected into an agent. Clone
/// it freely — plugins are held behind `Arc`.
#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    plugins: Vec<Arc<dyn ToolPlugin>>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry populated with the built-in minimal tool set
    /// (`read_file`, `list_dir`, `grep`, `shell`, `ask_user`).
    #[must_use]
    pub fn with_builtins() -> Self {
        Self {
            plugins: builtin_tools(),
        }
    }

    /// Adds a plugin to the registry, returning `self` for chaining.
    #[must_use]
    pub fn register(mut self, plugin: Arc<dyn ToolPlugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// Returns the registered plugins.
    #[must_use]
    pub fn plugins(&self) -> &[Arc<dyn ToolPlugin>] {
        &self.plugins
    }

    /// Returns the provider-neutral declarations of every registered tool.
    #[must_use]
    pub fn declarations(&self) -> Vec<Tool> {
        self.plugins
            .iter()
            .map(|plugin| plugin.declaration())
            .collect()
    }

    /// Builds a [`ToolSetRef`] carrying the registry's declarations for the given
    /// tool-set identity, ready to inject into an `AgentSpec`.
    #[must_use]
    pub fn tool_set(&self, id: ToolSetId) -> ToolSetRef {
        ToolSetRef::new(id, self.declarations())
    }

    /// Returns the static approval metadata for the named tool, if it exists.
    ///
    /// A `None` result means either the tool is unknown or it is auto-allowed.
    #[must_use]
    pub fn permission(&self, name: &str) -> Option<PermissionSpec> {
        self.plugins
            .iter()
            .find(|plugin| plugin.name() == name)
            .and_then(|plugin| plugin.permission())
    }

    /// Binds run-scoped context to the registry, yielding a runtime registry that
    /// implements [`agent::ToolRegistry`](agent_lib::agent::ToolRegistry).
    ///
    /// The supplied [`ToolContextParts`] (worktree, cancellation token, run and
    /// agent identity, trace) are stamped onto every tool invocation.
    #[must_use]
    pub fn bind(&self, context: ToolContextParts) -> PluginToolRegistry {
        PluginToolRegistry {
            plugins: self.plugins.clone(),
            context,
            user_interaction: None,
        }
    }

    /// Binds run-scoped context plus a host user-interaction bridge.
    ///
    /// This is the direct registry path for `ask_user` (`docs/CLI.md` §5 P6).
    /// mag-core's facade-tool projection supplies the same bridge while building
    /// typed facade tools, so both execution paths share one plugin contract.
    #[must_use]
    pub fn bind_with_user_interaction(
        &self,
        context: ToolContextParts,
        bridge: Arc<dyn UserInteractionBridge>,
    ) -> PluginToolRegistry {
        PluginToolRegistry {
            plugins: self.plugins.clone(),
            context,
            user_interaction: Some(bridge),
        }
    }
}

/// A [`ToolRegistry`] bound to run-scoped context, ready to drive an Agent loop.
///
/// This is the concrete [`agent::ToolRegistry`](agent_lib::agent::ToolRegistry)
/// implementation: [`declarations`](AgentToolRegistry::declarations) mirrors the
/// collector, and [`execute`](AgentToolRegistry::execute) dispatches a model tool
/// call to the matching plugin, building a fresh [`ToolContext`] from the bound
/// parts for each call.
#[derive(Clone)]
pub struct PluginToolRegistry {
    plugins: Vec<Arc<dyn ToolPlugin>>,
    context: ToolContextParts,
    user_interaction: Option<Arc<dyn UserInteractionBridge>>,
}

impl fmt::Debug for PluginToolRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginToolRegistry")
            .field(
                "plugins",
                &self
                    .plugins
                    .iter()
                    .map(|plugin| plugin.name())
                    .collect::<Vec<_>>(),
            )
            .field("has_user_interaction", &self.user_interaction.is_some())
            .finish_non_exhaustive()
    }
}

impl PluginToolRegistry {
    /// Builds the per-call [`ToolContext`] from the bound run-scoped parts.
    fn context_for(&self, tool_call_id: ToolCallId) -> ToolContext {
        ToolContext {
            run_id: self.context.run_id,
            agent_id: self.context.agent_id,
            tool_call_id,
            worktree: self.context.worktree.clone(),
            cancel: self.context.cancel.clone(),
            trace: self.context.trace.clone(),
        }
    }

    /// Builds the extended mag invocation context for one tool call.
    fn invocation_for(&self, tool_call_id: ToolCallId) -> ToolInvocation {
        let invocation = ToolInvocation::new(self.context_for(tool_call_id));
        match &self.user_interaction {
            Some(bridge) => invocation.with_user_interaction(Arc::clone(bridge)),
            None => invocation,
        }
    }
}

#[async_trait]
impl AgentToolRegistry for PluginToolRegistry {
    fn declarations(&self) -> Vec<Tool> {
        self.plugins
            .iter()
            .map(|plugin| plugin.declaration())
            .collect()
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        call: ToolCall,
    ) -> Result<ToolResponse, ToolRuntimeError> {
        let Some(plugin) = self
            .plugins
            .iter()
            .find(|plugin| plugin.name() == call.name)
        else {
            return Err(ToolRuntimeError::UnknownTool { name: call.name });
        };

        let provider_call_id = call.id.clone();
        let result = plugin
            .invoke_with_context(self.invocation_for(call_id), call.input)
            .await;
        Ok(ToolResponse {
            tool_call_id: provider_call_id,
            content: result.content().to_vec(),
            status: result.status(),
            extra: result.extra().clone(),
        })
    }
}
