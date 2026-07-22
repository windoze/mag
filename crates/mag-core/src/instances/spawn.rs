//! The `agent` spawn tool, its `agent_result` / `agent_cancel` companions,
//! and the instance drive tasks (`docs/dyn-agents.md` §4/§5/§6, TODO
//! M3-3/M3-4/M4-1).
//!
//! One shared [`InstanceSpawnContext`] per spawning agent (the session's root
//! supervisor at depth `0`, or a spawned instance at its own depth) backs the
//! `agent` tool: the handler validates the call synchronously (known type,
//! nesting budget left), registers the instance, announces
//! [`Event::AgentInstanceStarted`], and immediately returns
//! `{"id": ..., "status": "running"}`. The real work happens in a
//! `tokio::task::spawn_local` drive task on the session's `LocalSet` — the
//! facade run future is deliberately `!Send`, so `tokio::spawn` is not an
//! option and never compiles here (see `crate::session`).
//!
//! The drive task assembles the child from the definition: a `kind: local`
//! definition builds a child facade agent (the layered system prompt of
//! [`SUBAGENT_SKELETON`] + definition body, §4; the supervisor's tool surface
//! filtered by the supervisor's own surface bound and the definition's
//! `tools` allowlist plus the `agent` tool itself for nesting, §5.4/§7; the
//! supervisor's approval-policy projection), while a `kind: acp` definition
//! launches one external ACP process per instance and drives it through
//! agent-lib's one-shot [`run_external_once`](agent_lib::facade::run_external_once)
//! (§6, feature-gated `external-acp`). Both paths bubble every paused
//! interaction to the root session's [`IpcApproval`] through an
//! [`OriginRouter`] with the instance's attribution (§4/§6/§7). The run's
//! final text (a local child's final assistant message, §4; the external
//! peer's final message, §6) becomes the instance report; the terminal
//! transition goes through [`AgentInstanceRegistry::complete`] and is
//! announced as [`Event::AgentInstanceFinished`].
//!
//! `agent_result` (§5.1, D8) blocks on one instance's terminal state under a
//! caller-bounded timeout (`timeout_secs`, default 600): it answers the
//! terminal snapshot (`completed` carries the report, `failed` the error), or
//! `{"status": "running"}` when the timeout elapses — a timeout is not a
//! failure and never transitions the instance. The wait follows the race-free
//! check → enable → re-check → wait pattern [`Instance::done`] mandates and
//! selects on [`ToolContext::cancel`], so a cancelled run pre-empts the wait
//! instead of parking the tool until the timeout (the
//! `fulfill_batch_cancellable` contract of agent-lib's drive).
//! `agent_cancel` goes through [`AgentInstanceRegistry::cancel`] — the
//! first-terminal-wins transition plus the cooperative cancel handle — and
//! answers the instance's state after the call.

use std::{
    convert::Infallible,
    fmt::Write as _,
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};
#[cfg(feature = "external-acp")]
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use agent_lib::{
    agent::{
        ApprovalDecision, ApprovalResponse, Interaction, InteractionHandler, InteractionKind,
        InteractionOrigin, InteractionResponse, PermissionResponse, RequirementResult, RunContext,
        WorktreeRef,
    },
    client::LlmClient,
    facade::{Agent, ModelRef, Tool, ToolContext, ToolResult},
};
#[cfg(feature = "external-acp")]
use agent_lib::{
    agent::{
        BudgetLimits, CancellationToken,
        external::{AcpAdapter, AcpConfig, ExternalSessionRegistry, GitWorktreeManager},
    },
    facade::{FacadeIds, ManagedExternalAgent, RegistryExternalSessionHandler, run_external_once},
};
use async_trait::async_trait;
use mag_config::{AgentDefinition, AgentDefinitionRegistry, AgentKindDef};
use mag_service::{AgentInstanceStatusWire, Event, SessionId};
use mag_tools::ToolRegistry;
use serde_json::{Value, json};

use super::{AgentInstanceRegistry, Instance, InstanceStatus};
use crate::{
    EventBus,
    assembly::ApprovalOverrides,
    driver::{IpcUserInteractionBridge, apply_per_tool_tiers, project_tool_plugins},
    engine::approval::IpcApproval,
};

/// Name of the spawn tool (§5.1, D5); the [`AGENT_RESULT_TOOL_NAME`] /
/// [`AGENT_CANCEL_TOOL_NAME`] companions (M3-4) share its [`agent_tools`]
/// extension point.
pub(crate) const AGENT_TOOL_NAME: &str = "agent";

/// Name of the blocking result-collection tool (§5.1, D8).
pub(crate) const AGENT_RESULT_TOOL_NAME: &str = "agent_result";

/// Name of the instance-cancellation tool (§5.1).
pub(crate) const AGENT_CANCEL_TOOL_NAME: &str = "agent_cancel";

/// Wait budget of [`AGENT_RESULT_TOOL_NAME`] when the call omits
/// `timeout_secs` (§5.1).
const DEFAULT_RESULT_TIMEOUT_SECS: u64 = 600;

/// Default agent type when the `agent` call omits `type` (§3.3).
const DEFAULT_AGENT_TYPE: &str = "general-purpose";

/// Maximum nesting depth for agent instances (§5.4), mirroring agent-lib's
/// `DEFAULT_MAX_DELEGATION_DEPTH` semantics: the root supervisor drives at
/// depth `0`, and a spawner already at this depth is refused (so the deepest
/// possible instance sits at depth `MAX_INSTANCE_DEPTH`).
pub(crate) const MAX_INSTANCE_DEPTH: u32 = 8;

/// Step budget for an instance whose definition sets no `max_steps` (§7:
/// every instance has a budget backstop).
const DEFAULT_INSTANCE_MAX_STEPS: u32 = 16;

/// First layer of every local instance's system prompt (§4): the shared
/// skeleton declaring the subagent role and the report contract. The
/// definition body is appended as the second layer by
/// [`layered_system_prompt`]; the skeleton only states the role and the
/// contract so it never conflicts with a body.
pub(crate) const SUBAGENT_SKELETON: &str = "\
You are a subagent spawned by a supervisor agent; the opening user message is
your task brief.

- Work autonomously within your step budget to complete the task end to end.
- You do not interact with the end user directly: any approval your tools
  require bubbles up to the supervisor's session and is answered there.
- Your final message is your report to the supervisor, not a chat reply:
  state your conclusions, the changes you made, the key file references
  (`path:line`), and anything left unresolved.";

/// Session-wide spawn state shared by every [`InstanceSpawnContext`] of one
/// session: the merged agent-definition table, the supervisor's effective
/// model, and the supervisor's plugin-surface filter
/// (`docs/dyn-agents.md` §3.2/§7).
///
/// All three pieces are hot-swappable behind one lock: the session driver
/// rebuilds the definition table at config-apply time (the TOML definition
/// layer is re-projected and re-merged), the supervisor model follows the
/// built agent's authoritative value at build and at every turn start (an
/// applied `SetModel` reconfiguration lands at the turn boundary), and the
/// surface filter follows a bound tool-list change the same way. The root
/// supervisor's context and every child context derived from it share the
/// same cell, so a rebuilt table takes effect for the next spawn at any depth
/// (definitions only affect *later* spawns, M3-5).
#[derive(Clone, Debug)]
pub(crate) struct SharedSpawnState {
    inner: Arc<RwLock<SpawnStateInner>>,
}

/// The swappable triple behind [`SharedSpawnState`].
#[derive(Clone, Debug)]
struct SpawnStateInner {
    /// Merged definition table the `agent` tool's `type` resolves against.
    definitions: AgentDefinitionRegistry,
    /// The supervisor's effective model; a definition without `model`
    /// inherits it, and `max_tokens` always aligns with it.
    supervisor_model: ModelRef,
    /// The supervisor's plugin-surface filter — its bound `agents.<name>`
    /// tool list; `None` is unconstrained. Every instance surface is
    /// intersected with it (§7's anti-escalation rule: a definition, which
    /// is data possibly shipped by a project, must never grant a child a
    /// plugin the supervisor itself does not have). The instance tool trio
    /// is exempt, exactly as on the supervisor's own surface.
    surface: Option<Vec<String>>,
}

impl SharedSpawnState {
    /// Creates the shared cell from the assembled definition table, the
    /// supervisor's model, and the supervisor's plugin-surface filter.
    pub(crate) fn new(
        definitions: AgentDefinitionRegistry,
        supervisor_model: ModelRef,
        surface: Option<Vec<String>>,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SpawnStateInner {
                definitions,
                supervisor_model,
                surface,
            })),
        }
    }

    /// Replaces only the definition table (config apply, M3-5).
    pub(crate) fn set_definitions(&self, definitions: AgentDefinitionRegistry) {
        self.lock().definitions = definitions;
    }

    /// Replaces only the supervisor model (post-build correction to the
    /// built agent's authoritative model, and the turn-start follow of an
    /// applied `SetModel`).
    pub(crate) fn set_supervisor_model(&self, supervisor_model: ModelRef) {
        self.lock().supervisor_model = supervisor_model;
    }

    /// Replaces only the supervisor's plugin-surface filter (config apply
    /// changed the bound entry's tool list).
    pub(crate) fn set_surface(&self, surface: Option<Vec<String>>) {
        self.lock().surface = surface;
    }

    /// Returns the supervisor's current plugin-surface filter (`None` is
    /// unconstrained).
    pub(crate) fn surface(&self) -> Option<Vec<String>> {
        self.lock().surface.clone()
    }

    /// Returns a clone of the definition named `name` (the `agent` tool's
    /// `type` parameter).
    pub(crate) fn definition(&self, name: &str) -> Option<AgentDefinition> {
        self.lock().definitions.get(name).cloned()
    }

    /// Enumerates every definition for the `agent` tool's description.
    pub(crate) fn describe_for_tool(&self) -> String {
        self.lock().definitions.describe_for_tool()
    }

    /// Returns the supervisor's current effective model.
    pub(crate) fn supervisor_model(&self) -> ModelRef {
        self.lock().supervisor_model.clone()
    }

    /// Locks the cell, recovering the guard from a poisoned lock (the
    /// codebase's unified poison-recovery policy). The guard is never held
    /// across an `.await`.
    fn lock(&self) -> std::sync::RwLockWriteGuard<'_, SpawnStateInner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Everything the `agent` tool needs to spawn and drive one instance, shared
/// behind an [`Arc`] between the tool handler and every drive task it starts.
///
/// One context belongs to one *spawning* agent: the session's root supervisor
/// holds the depth-`0` context (assembled by the session driver, M3-5), and
/// each spawned instance gets [`child_context`](Self::child_context) — the
/// same shared handles at depth + 1 — for its own `agent` tool, which is how
/// nesting (§5.4) and the depth cap are implemented. The client, tool
/// registry, approval overrides, interaction handler, event bus, and shared
/// spawn state are the supervisor's own handles, so a child runs on the same
/// LLM client, tool surface, approval authority, definition table, and
/// session event stream as its parent.
#[derive(Clone)]
pub(crate) struct InstanceSpawnContext {
    /// Instance table shared session-wide.
    pub(crate) registry: AgentInstanceRegistry,
    /// Swappable definition table and supervisor model, shared session-wide.
    pub(crate) shared: SharedSpawnState,
    /// LLM client shared with the supervisor (children never build their own).
    pub(crate) client: Arc<dyn LlmClient>,
    /// Tool plugins the child surface is projected from (the session's
    /// registry, `docs/dyn-agents.md` §7).
    pub(crate) tools: Arc<ToolRegistry>,
    /// Session approval tiers projected onto the child's policy.
    pub(crate) overrides: ApprovalOverrides,
    /// Root session's interaction authority every instance's paused
    /// interaction bubbles to (through an [`OriginRouter`]).
    pub(crate) interaction: Arc<IpcApproval>,
    /// Session event bus the lifecycle events are emitted on.
    pub(crate) events: EventBus,
    /// Owning session, stamped on every lifecycle event.
    pub(crate) session_id: SessionId,
    /// Worktree the child's tools resolve against (the session's cwd).
    pub(crate) worktree: WorktreeRef,
    /// Nesting depth of the *spawning* agent (`0` for the root supervisor);
    /// spawned instances get `depth + 1`.
    pub(crate) depth: u32,
}

impl InstanceSpawnContext {
    /// Returns the context for a spawned instance's own `agent` tool: the
    /// same shared handles one nesting level deeper.
    fn child_context(&self) -> Arc<Self> {
        Arc::new(Self {
            depth: self.depth + 1,
            ..self.clone()
        })
    }
}

impl std::fmt::Debug for InstanceSpawnContext {
    /// Prints structural fields while treating the client as opaque (mirrors
    /// the facade [`Agent`]'s own `Debug`).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstanceSpawnContext")
            .field("registry", &self.registry)
            .field("shared", &self.shared)
            .field("client", &"<dyn LlmClient>")
            .field("tools", &self.tools)
            .field("overrides", &self.overrides)
            .field("events", &self.events)
            .field("session_id", &self.session_id)
            .field("worktree", &self.worktree)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

/// The instance tools available on one spawning agent's tool surface: the
/// [`AGENT_TOOL_NAME`] spawn tool (M3-3) plus the [`AGENT_RESULT_TOOL_NAME`] /
/// [`AGENT_CANCEL_TOOL_NAME`] companions (M3-4), so the supervisor and every
/// child surface gain the trio together (§5.1). The session driver appends
/// them to the supervisor's surface (M3-5); the child surface reserves the
/// same extension point: [`drive_local`] appends `agent_tools` of the
/// instance's own context after its projected plugins.
pub(crate) fn agent_tools(ctx: &Arc<InstanceSpawnContext>) -> Vec<Tool> {
    vec![
        agent_tool(ctx),
        agent_result_tool(ctx),
        agent_cancel_tool(ctx),
    ]
}

/// Builds the `agent` spawn tool for one spawning agent's surface.
///
/// The handler is synchronous up to the `spawn_local`: argument and type
/// validation, the depth check, registration, and the
/// [`Event::AgentInstanceStarted`] announcement all happen before it returns
/// `{"id": ..., "status": "running"}`, so the model learns of a refused spawn
/// immediately as an ordinary tool error.
fn agent_tool(ctx: &Arc<InstanceSpawnContext>) -> Tool {
    let description = format!(
        "Spawn an agent instance to work on a task asynchronously. Returns immediately with the \
         instance id and status `running`; the instance runs concurrently. Collect its report \
         with `agent_result`, cancel it with `agent_cancel`, or watch for the completion \
         notification.\n\n{}",
        ctx.shared.describe_for_tool()
    );
    let schema = json!({
        "type": "object",
        "properties": {
            "type": {
                "type": "string",
                "description": "Agent type to spawn; defaults to `general-purpose`.",
            },
            "task": {
                "type": "string",
                "description": "Task brief; delivered to the instance as its opening user message.",
            },
            "description": {
                "type": "string",
                "description": "Optional short human-readable label for the UI and logs.",
            },
        },
        "required": ["task"],
    });
    let ctx = Arc::clone(ctx);
    Tool::function_with_schema(
        AGENT_TOOL_NAME,
        description,
        schema,
        move |_ctx: ToolContext, args: Value| {
            let ctx = Arc::clone(&ctx);
            async move { Ok::<ToolResult, Infallible>(spawn_instance(&ctx, args)) }
        },
    )
}

/// Synchronous body of the `agent` tool handler: validate, register,
/// announce, spawn the drive task, and answer `running`.
fn spawn_instance(ctx: &Arc<InstanceSpawnContext>, args: Value) -> ToolResult {
    let agent_type = match args.get("type") {
        None | Some(Value::Null) => DEFAULT_AGENT_TYPE,
        Some(Value::String(agent_type)) => agent_type.as_str(),
        Some(_) => return ToolResult::error("`type` must be a string"),
    };
    let task = match args.get("task").and_then(Value::as_str) {
        Some(task) if !task.trim().is_empty() => task.to_owned(),
        _ => {
            return ToolResult::error("`task` is required and must be a non-empty string");
        }
    };
    let description = match args.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(description)) => Some(description.clone()),
        Some(_) => return ToolResult::error("`description` must be a string"),
    };

    let Some(definition) = ctx.shared.definition(agent_type) else {
        return ToolResult::error(format!(
            "unknown agent type `{agent_type}`\n\n{}",
            ctx.shared.describe_for_tool()
        ));
    };
    if ctx.depth >= MAX_INSTANCE_DEPTH {
        return ToolResult::error(format!(
            "agent nesting depth limit ({MAX_INSTANCE_DEPTH}) reached: this agent cannot spawn \
             further instances; finish the task directly or report the blocker"
        ));
    }
    #[cfg(not(feature = "external-acp"))]
    if matches!(definition.kind, AgentKindDef::ExternalAcp { .. }) {
        return ToolResult::error(format!(
            "agent type `{agent_type}` is external (kind: acp), but this build has the \
             `external-acp` feature disabled; rebuild mag-core with the feature enabled (it is \
             on by default) to spawn external instances"
        ));
    }

    let id = ctx.registry.next_id(&definition.name);
    let depth = ctx.depth + 1;
    let instance = ctx
        .registry
        .register(Instance::new(id.clone(), definition.name.clone(), depth));
    let _ = ctx.events.emit(Event::AgentInstanceStarted {
        id: ctx.session_id,
        instance_id: id.clone(),
        agent_type: definition.name.clone(),
        description,
        depth,
    });
    tokio::task::spawn_local(drive_instance(Arc::clone(ctx), definition, instance, task));
    ToolResult::text(json!({ "id": id, "status": "running" }).to_string())
}

/// Builds the `agent_result` blocking-collection tool (§5.1, D8).
///
/// Unlike the spawn tool's synchronous handler, this one awaits: it blocks on
/// the instance's terminal state under the caller's `timeout_secs` budget and
/// selects on [`ToolContext::cancel`] so a cancelled run pre-empts the wait
/// (the `fulfill_batch_cancellable` contract of agent-lib's drive — a tool
/// must not rely on running to completion once its run is cancelled).
fn agent_result_tool(ctx: &Arc<InstanceSpawnContext>) -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Instance id returned by `agent`.",
            },
            "timeout_secs": {
                "type": "number",
                "description": "Maximum seconds to wait for the terminal state; defaults to 600. \
                                Timing out returns `running` — the instance keeps going.",
            },
        },
        "required": ["id"],
    });
    let ctx = Arc::clone(ctx);
    Tool::function_with_schema(
        AGENT_RESULT_TOOL_NAME,
        "Block until an agent instance reaches a terminal state and return its outcome: the \
         completed report, the failure error, or `cancelled`. Elapsing `timeout_secs` (default \
         600) answers `running` — not a failure, the instance keeps going; call again to keep \
         waiting, or rely on the completion notification.",
        schema,
        move |tool_ctx: ToolContext, args: Value| {
            let ctx = Arc::clone(&ctx);
            async move {
                Ok::<ToolResult, Infallible>(await_instance_result(&ctx, &tool_ctx, args).await)
            }
        },
    )
}

/// Body of the `agent_result` handler: resolve the instance, then race its
/// terminal state against the caller's timeout and the calling run's
/// cancellation.
async fn await_instance_result(
    ctx: &Arc<InstanceSpawnContext>,
    tool_ctx: &ToolContext,
    args: Value,
) -> ToolResult {
    let id = match parse_instance_id(&args) {
        Ok(id) => id.to_owned(),
        Err(result) => return result,
    };
    let timeout_secs = match args.get("timeout_secs") {
        None | Some(Value::Null) => DEFAULT_RESULT_TIMEOUT_SECS,
        Some(Value::Number(timeout)) => match timeout.as_u64() {
            Some(timeout) => timeout,
            None => {
                return ToolResult::error(
                    "`timeout_secs` must be a non-negative integer number of seconds",
                );
            }
        },
        Some(_) => {
            return ToolResult::error(
                "`timeout_secs` must be a non-negative integer number of seconds",
            );
        }
    };
    let Some(instance) = ctx.registry.get(&id) else {
        return ToolResult::error(unknown_instance_error(&ctx.registry, &id));
    };

    // The cancel-preemption shape of agent-lib's `fulfill_batch_cancellable`
    // (`agent-lib/src/agent/drive.rs`): the wait is the primary branch and a
    // fired `ToolContext.cancel` aborts it promptly instead of leaving the
    // tool parked until the timeout.
    let wait = tokio::time::timeout(Duration::from_secs(timeout_secs), await_terminal(&instance));
    let status = tokio::select! {
        biased;
        status = wait => status.ok(),
        () = tool_ctx.cancel.cancelled() => {
            return ToolResult::error(format!(
                "cancelled while waiting for agent instance `{id}`"
            ));
        }
    };
    match status {
        // A timeout is not a failure: the instance keeps running and the
        // model may collect it later (or rely on the notification).
        None => ToolResult::text(json!({ "id": id, "status": "running" }).to_string()),
        Some(status) => ToolResult::text(instance_status_json(&id, &status).to_string()),
    }
}

/// Builds the `agent_cancel` tool (§5.1). The handler is synchronous like the
/// spawn tool's: the registry transition and the cooperative cancel-handle
/// fire both happen before it answers, so the snapshot it returns is already
/// terminal.
fn agent_cancel_tool(ctx: &Arc<InstanceSpawnContext>) -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Instance id returned by `agent`.",
            },
        },
        "required": ["id"],
    });
    let ctx = Arc::clone(ctx);
    Tool::function_with_schema(
        AGENT_CANCEL_TOOL_NAME,
        "Cancel a running agent instance: its in-flight work is interrupted cooperatively and its \
         terminal state becomes `cancelled`. An already-finished instance keeps its outcome. \
         Returns the instance state after the call.",
        schema,
        move |_ctx: ToolContext, args: Value| {
            let ctx = Arc::clone(&ctx);
            async move { Ok::<ToolResult, Infallible>(cancel_instance(&ctx, args)) }
        },
    )
}

/// Synchronous body of the `agent_cancel` handler.
fn cancel_instance(ctx: &Arc<InstanceSpawnContext>, args: Value) -> ToolResult {
    let id = match parse_instance_id(&args) {
        Ok(id) => id.to_owned(),
        Err(result) => return result,
    };
    match ctx.registry.cancel(&id) {
        Some(status) => ToolResult::text(instance_status_json(&id, &status).to_string()),
        None => ToolResult::error(unknown_instance_error(&ctx.registry, &id)),
    }
}

/// Extracts the `id` argument `agent_result` and `agent_cancel` both require.
fn parse_instance_id(args: &Value) -> Result<&str, ToolResult> {
    match args.get("id").and_then(Value::as_str) {
        Some(id) if !id.trim().is_empty() => Ok(id),
        _ => Err(ToolResult::error(
            "`id` is required and must be a non-empty string",
        )),
    }
}

/// Awaits the instance's terminal status with the race-free
/// check → enable → re-check → wait pattern [`Instance::done`] mandates.
async fn await_terminal(instance: &Instance) -> InstanceStatus {
    loop {
        let notified = instance.done().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let status = instance.status();
        if status.is_terminal() {
            return status;
        }
        notified.as_mut().await;
    }
}

/// The state snapshot `agent_result` and `agent_cancel` answer with (§5.1):
/// `completed` carries the report, `failed` the error.
fn instance_status_json(id: &str, status: &InstanceStatus) -> Value {
    match status {
        InstanceStatus::Completed { report } => {
            json!({ "id": id, "status": "completed", "report": report })
        }
        InstanceStatus::Failed { error } => {
            json!({ "id": id, "status": "failed", "error": error })
        }
        InstanceStatus::Cancelled => json!({ "id": id, "status": "cancelled" }),
        InstanceStatus::Running => json!({ "id": id, "status": "running" }),
    }
}

/// The unknown-id error of `agent_result` / `agent_cancel`, attaching the
/// current instance table so the model can recover a valid id.
fn unknown_instance_error(registry: &AgentInstanceRegistry, id: &str) -> String {
    let instances = registry.list();
    if instances.is_empty() {
        return format!(
            "unknown agent instance `{id}`\n\nno agent instances have been spawned in this session"
        );
    }
    let mut error = format!("unknown agent instance `{id}`\n\nknown agent instances:");
    for instance in instances {
        let status = match instance.status() {
            InstanceStatus::Running => "running",
            InstanceStatus::Completed { .. } => "completed",
            InstanceStatus::Failed { .. } => "failed",
            InstanceStatus::Cancelled => "cancelled",
        };
        let _ = write!(
            error,
            "\n- {} ({}, {status})",
            instance.id, instance.agent_type
        );
    }
    error
}

/// Assembles the two-layer system prompt of a local instance (§4): the shared
/// skeleton, then the definition body.
fn layered_system_prompt(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        SUBAGENT_SKELETON.to_owned()
    } else {
        format!("{SUBAGENT_SKELETON}\n\n{body}")
    }
}

/// Routes one instance's paused interaction to the root session's approval
/// handler with the instance's origin attribution (`docs/dyn-agents.md` §4/§7).
///
/// The child agent keeps its own approval-policy gate; this router only
/// decides where an already-paused interaction is answered, annotating it with
/// the instance id and depth so the interface can render which instance is
/// asking. It is the instance-model counterpart of agent-lib's
/// `DelegationInteractionRouter` (`docs/CLI.md` §3.3, decision D5), including
/// the cancel wrapper: a cancelled instance resolves its parked interaction
/// conservatively instead of hanging the drive task.
struct OriginRouter {
    /// Instance id shown as the interaction origin (`delegate` on the wire).
    label: String,
    /// Nesting depth of the instance (`1` for a direct child of the root).
    depth: u32,
    /// The root session's interaction handler the request is forwarded to.
    parent: Arc<dyn InteractionHandler>,
}

impl std::fmt::Debug for OriginRouter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OriginRouter")
            .field("label", &self.label)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl InteractionHandler for OriginRouter {
    async fn fulfill(&self, request: &Interaction, ctx: &RunContext) -> RequirementResult {
        let routed = request
            .clone()
            .with_origin(InteractionOrigin::new(self.label.clone(), self.depth));
        tokio::select! {
            biased;
            _ = ctx.cancellation().cancelled() => cancelled_interaction_result(&routed),
            result = self.parent.fulfill(&routed, ctx) => result,
        }
    }
}

/// Builds an in-family interaction result for an instance interaction
/// abandoned by cancellation before the root handler answered (mirrors
/// agent-lib's `cancelled_delegation_interaction_result`, which is
/// crate-private there).
fn cancelled_interaction_result(request: &Interaction) -> RequirementResult {
    let response = match request.kind() {
        InteractionKind::Approval { call_id, .. } => {
            InteractionResponse::Approval(ApprovalResponse::new(
                request.step_id(),
                *call_id,
                ApprovalDecision::Deny,
                Some("interaction cancelled".to_owned()),
            ))
        }
        InteractionKind::Question { .. } => InteractionResponse::answer(String::new()),
        InteractionKind::Choice { .. } => InteractionResponse::Choice(0),
        InteractionKind::Permission { request } => InteractionResponse::Permission(
            PermissionResponse::cancel(request.action_id().to_owned()),
        ),
    };
    RequirementResult::Interaction(response)
}

/// Drives one spawned instance to its terminal state: builds and runs the
/// child agent, performs the terminal transition, and announces
/// [`Event::AgentInstanceFinished`].
///
/// The terminal status announced is read back from the registry *after*
/// [`AgentInstanceRegistry::complete`]: a racing
/// [`cancel`](AgentInstanceRegistry::cancel) /
/// [`cancel_all`](AgentInstanceRegistry::cancel_all) may have won the
/// first-terminal-wins race, in which case the instance is `Cancelled` even
/// when the drive itself finished or failed.
async fn drive_instance(
    ctx: Arc<InstanceSpawnContext>,
    definition: AgentDefinition,
    instance: Arc<Instance>,
    task: String,
) {
    let result = match &definition.kind {
        AgentKindDef::Local { .. } => drive_local(&ctx, &definition, &instance, &task).await,
        AgentKindDef::ExternalAcp { .. } => {
            drive_external(&ctx, &definition, &instance, &task).await
        }
    };
    let status = match result {
        Ok(report) => InstanceStatus::Completed { report },
        // A fired cancel handle means the registry already transitioned the
        // instance to `Cancelled` (the handle only fires from `cancel` /
        // `cancel_all` after their winning transition): keep that outcome
        // rather than overwriting it with the drive error.
        Err(_) if instance.cancel_handle().is_cancelled() => InstanceStatus::Cancelled,
        Err(error) => InstanceStatus::Failed { error },
    };
    // A losing transition is expected on a cancel race (and is a no-op then):
    // the notification queue entry was already pushed by the winner.
    let _ = ctx.registry.complete(&instance.id, status);

    let (status, report, error) = match instance.status() {
        InstanceStatus::Completed { report } => {
            (AgentInstanceStatusWire::Completed, Some(report), None)
        }
        InstanceStatus::Failed { error } => (AgentInstanceStatusWire::Failed, None, Some(error)),
        InstanceStatus::Cancelled => (AgentInstanceStatusWire::Cancelled, None, None),
        InstanceStatus::Running => unreachable!(
            "the instance is terminal after its drive task's transition (or a cancel won it)"
        ),
    };
    let _ = ctx.events.emit(Event::AgentInstanceFinished {
        id: ctx.session_id,
        instance_id: instance.id.clone(),
        agent_type: instance.agent_type.clone(),
        status,
        report,
        error,
    });
}

/// Builds the child facade agent for one local instance and runs it to a
/// terminal state, returning the final assistant text (the instance report,
/// §4) or the run's failure.
async fn drive_local(
    ctx: &Arc<InstanceSpawnContext>,
    definition: &AgentDefinition,
    instance: &Arc<Instance>,
    task: &str,
) -> Result<String, String> {
    let AgentKindDef::Local {
        model,
        tools,
        max_steps,
    } = &definition.kind
    else {
        // The dispatch matches on the kind, so this is unreachable.
        return Err(format!(
            "agent type `{}` is not a local definition",
            definition.name
        ));
    };

    let origin = Arc::new(OriginRouter {
        label: instance.id.clone(),
        depth: instance.depth,
        parent: ctx.interaction.clone() as Arc<dyn InteractionHandler>,
    });
    // The child's `ask_user` bubbles to the root session with the instance's
    // origin, exactly like its gated-tool approvals (§4: no direct end-user
    // interaction).
    let user_interaction = Arc::new(IpcUserInteractionBridge::new(
        origin.clone() as Arc<dyn InteractionHandler>
    ));
    // §7: the child surface is the supervisor's plugin projection narrowed
    // by the supervisor's own surface filter (the bound `agents.<name>`
    // tool list — the anti-escalation anchor, shared at every depth) and
    // intersected with the definition's allowlist (`None` inherits the
    // supervisor's surface), plus the instance tools of the child's own
    // context for nesting (§5.4).
    let allowed: Option<Vec<String>> = match (ctx.shared.surface(), tools.clone()) {
        (None, None) => None,
        (Some(bound), None) => Some(bound),
        (None, Some(definition)) => Some(definition),
        (Some(bound), Some(definition)) => Some(
            definition
                .into_iter()
                .filter(|name| bound.contains(name))
                .collect(),
        ),
    };
    let (mut surface, policy) = project_tool_plugins(
        &ctx.tools,
        allowed.as_deref(),
        ctx.overrides.default_tier(),
        user_interaction,
    );
    let policy = apply_per_tool_tiers(policy, &ctx.overrides);
    surface.extend(agent_tools(&ctx.child_context()));

    let supervisor_model = ctx.shared.supervisor_model();
    let mut builder = Agent::builder()
        .client(Arc::clone(&ctx.client))
        .model(
            model
                .clone()
                .unwrap_or_else(|| supervisor_model.model().to_owned()),
        )
        .max_tokens(supervisor_model.max_tokens().get())
        .max_steps(max_steps.unwrap_or(DEFAULT_INSTANCE_MAX_STEPS))
        .system(layered_system_prompt(&definition.body))
        .worktree(ctx.worktree.clone())
        .interaction_handler(origin as Arc<dyn InteractionHandler>)
        .approval(policy);
    for tool in surface {
        builder = builder.tool(tool);
    }
    let mut agent = builder.build().map_err(|error| error.to_string())?;
    let output = agent
        .run_full_with_cancel(task.to_owned(), instance.cancel_handle())
        .await
        .map_err(|error| error.to_string())?;
    Ok(output.reply.text().to_owned())
}

/// Launches one external ACP process for a `kind: acp` instance and drives it
/// through agent-lib's one-shot [`run_external_once`] (`docs/dyn-agents.md`
/// §6, TODO M4-1), returning the peer's final message as the instance report.
///
/// The runtime is assembled per instance with the parameters M3-5 recorded
/// from the retired static-delegate path: an [`AcpConfig`] with a 120s
/// request timeout, the definition's `env` overrides, and the session's
/// worktree as the child's working directory, behind a fresh
/// [`ExternalSessionRegistry`] (process isolation is natural — concurrent
/// instances are independent processes; the registry is dropped with this
/// drive task and `run_external_once`'s one-shot semantics reclaim the
/// process at every terminal state). The peer's ACP permission requests
/// bubble to the root session through the same [`OriginRouter`] the local
/// path uses (§6: 与 local 一致的 origin 冒泡).
///
/// Cancellation bridges the instance's registry-fired cancel handle onto the
/// one-shot call's [`CancellationToken`]: [`await_terminal`] observes the
/// registry's `cancel` / `cancel_all` transition (this drive's own completion
/// lands only after it returns, so a mid-drive terminal transition is always
/// a cancel win), fires the token, and the call abandons the session and
/// schedules its own cleanup sweep.
#[cfg(feature = "external-acp")]
async fn drive_external(
    ctx: &Arc<InstanceSpawnContext>,
    definition: &AgentDefinition,
    instance: &Arc<Instance>,
    task: &str,
) -> Result<String, String> {
    let AgentKindDef::ExternalAcp { command, env } = &definition.kind else {
        // The dispatch matches on the kind, so this is unreachable.
        return Err(format!(
            "agent type `{}` is not an external ACP definition",
            definition.name
        ));
    };
    let Some((binary, args)) = split_external_command(command) else {
        return Err(format!(
            "agent type `{}` has an empty `command` (expected argv form, binary first)",
            definition.name
        ));
    };

    let mut acp_config =
        AcpConfig::new(binary.clone(), args.clone()).with_timeout(Duration::from_secs(120));
    for (key, value) in env {
        acp_config = acp_config.with_env(key.clone(), value.clone());
    }
    let worktree = ctx.worktree.path().to_path_buf();
    acp_config = acp_config.with_working_dir(worktree.clone());
    let registry = Arc::new(ExternalSessionRegistry::with_worktree_manager(
        Arc::new(AcpAdapter::new(acp_config)),
        Arc::new(GitWorktreeManager::new().with_root(external_worktree_root())),
    ));
    let agent = ManagedExternalAgent::acp(binary, args)
        .session_handler(Arc::new(RegistryExternalSessionHandler::new(registry)))
        .worktree(worktree)
        .build()
        .map_err(|error| error.to_string())?;

    let origin = Arc::new(OriginRouter {
        label: instance.id.clone(),
        depth: instance.depth,
        parent: ctx.interaction.clone() as Arc<dyn InteractionHandler>,
    });
    let ids = FacadeIds::new();
    let cancel = CancellationToken::new();
    let drive = run_external_once(
        &definition.name,
        &agent,
        &ids,
        external_task(&definition.body, task),
        Some(origin as Arc<dyn InteractionHandler>),
        // No per-step budget maps onto a black-box external runtime
        // (agent-lib's own one-shot callers pass an unbounded budget); the
        // backstops are the ACP request timeout above and the instance's
        // cooperative cancel.
        BudgetLimits::unbounded(),
        cancel.clone(),
    );
    tokio::pin!(drive);
    let outcome = tokio::select! {
        biased;
        outcome = &mut drive => outcome,
        _ = await_terminal(instance) => {
            cancel.cancel();
            drive.await
        }
    };
    match outcome {
        Ok(outcome) if outcome.completed => Ok(outcome.summary),
        Ok(_) => Err(format!(
            "external agent `{}` ended before completing its task",
            definition.name
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// The feature-off counterpart of the external drive: the spawn handler
/// refuses external definitions synchronously, so this only keeps the
/// dispatch arm compiling and stays unreachable at runtime.
#[cfg(not(feature = "external-acp"))]
async fn drive_external(
    _ctx: &Arc<InstanceSpawnContext>,
    definition: &AgentDefinition,
    _instance: &Arc<Instance>,
    _task: &str,
) -> Result<String, String> {
    Err(format!(
        "agent type `{}` is external (kind: acp), but this build has the `external-acp` feature \
         disabled",
        definition.name
    ))
}

/// Builds an external instance's opening task (§6): the definition body is
/// the task-frame template prepended to the caller's task.
#[cfg(feature = "external-acp")]
fn external_task(body: &str, task: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        task.to_owned()
    } else {
        format!("{body}\n\n{task}")
    }
}

/// Splits an argv-form external command into binary + args; `None` when the
/// command is empty. Both real definition paths already reject an empty
/// command (the markdown frontmatter parser at parse time, the TOML
/// projection with a skip-and-warn — `docs/dyn-agents.md` §3.1, TODO M4-R),
/// so the `None` arm is unreachable defense in depth.
#[cfg(feature = "external-acp")]
fn split_external_command(command: &[String]) -> Option<(PathBuf, Vec<String>)> {
    let (binary, args) = command.split_first()?;
    Some((binary.into(), args.to_vec()))
}

/// Unique root for the per-instance ephemeral worktrees (the M3-5 recorded
/// parameter: `temp_dir/mag-external-worktrees-{pid}-{counter}`).
#[cfg(feature = "external-acp")]
fn external_worktree_root() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "mag-external-worktrees-{}-{unique}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        future::Future,
        num::NonZeroU32,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use agent_lib::{
        agent::{InteractionHandler, WorktreeRef},
        client::{ChatRequest, LlmClient},
        facade::{Agent, Approval, ModelRef, ToolContext, ToolResult},
        model::{
            content::ContentBlock,
            message::Role,
            tool::{Tool as ToolDecl, ToolStatus},
            usage::Usage,
        },
    };
    use futures::{FutureExt, StreamExt};
    use mag_config::AgentDefinitionRegistry;
    use mag_service::{
        AgentInstanceStatusWire, ApprovalDecisionWire, Event, InteractionKindWire,
        InteractionResponseWire, SessionId, StepIdWire, ToolCallIdWire,
    };
    use mag_tools::{PermissionSpec, ToolCategory, ToolPlugin, ToolRegistry, ToolRisk};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        AGENT_CANCEL_TOOL_NAME, AGENT_RESULT_TOOL_NAME, AGENT_TOOL_NAME, InstanceSpawnContext,
        MAX_INSTANCE_DEPTH, SUBAGENT_SKELETON, SharedSpawnState, agent_tools,
        layered_system_prompt,
    };
    use crate::{
        EventBus, EventStream,
        assembly::ApprovalOverrides,
        engine::approval::{AskFrontendDecider, IpcApproval},
        instances::{AgentInstanceRegistry, Instance, InstanceStatus},
        test_support::{
            FakeLlmClient, RequestRoute, StreamGate, StreamScript, text_stream_with_usage,
            tool_use_stream,
        },
    };

    /// Everything a spawn test needs: the shared context plus the handles the
    /// assertions read back.
    struct TestRig {
        ctx: Arc<InstanceSpawnContext>,
        registry: AgentInstanceRegistry,
        events: EventBus,
        ipc: Arc<IpcApproval>,
        client: Arc<FakeLlmClient>,
    }

    fn rig(
        client: &Arc<FakeLlmClient>,
        tools: ToolRegistry,
        definitions: AgentDefinitionRegistry,
    ) -> TestRig {
        rig_with_depth(client, tools, definitions, 0)
    }

    fn rig_with_depth(
        client: &Arc<FakeLlmClient>,
        tools: ToolRegistry,
        definitions: AgentDefinitionRegistry,
        depth: u32,
    ) -> TestRig {
        rig_with_surface(client, tools, definitions, depth, None)
    }

    fn rig_with_surface(
        client: &Arc<FakeLlmClient>,
        tools: ToolRegistry,
        definitions: AgentDefinitionRegistry,
        depth: u32,
        surface: Option<Vec<String>>,
    ) -> TestRig {
        let events = EventBus::new();
        let session_id = SessionId::new(Uuid::from_u128(7));
        let ipc = Arc::new(IpcApproval::new(
            session_id,
            events.clone(),
            Arc::new(AskFrontendDecider),
        ));
        let registry = AgentInstanceRegistry::new();
        let ctx = Arc::new(InstanceSpawnContext {
            registry: registry.clone(),
            shared: SharedSpawnState::new(
                definitions,
                ModelRef::new(
                    "supervisor-model",
                    NonZeroU32::new(64).expect("non-zero"),
                    None,
                    None,
                ),
                surface,
            ),
            client: client.clone() as Arc<dyn LlmClient>,
            tools: Arc::new(tools),
            overrides: ApprovalOverrides::default(),
            interaction: ipc.clone(),
            events: events.clone(),
            session_id,
            worktree: WorktreeRef::new("."),
            depth,
        });
        TestRig {
            ctx,
            registry,
            events,
            ipc,
            client: client.clone(),
        }
    }

    /// A supervisor facade agent whose only tool is `agent` (mirroring the
    /// surface the session driver assembles, M3-5).
    fn supervisor_agent(rig: &TestRig) -> Agent {
        let mut builder = Agent::builder()
            .client(rig.client.clone() as Arc<dyn LlmClient>)
            .model("supervisor-model")
            .max_tokens(64)
            .approval(Approval::auto_allow())
            .interaction_handler(rig.ipc.clone() as Arc<dyn InteractionHandler>);
        for tool in agent_tools(&rig.ctx) {
            builder = builder.tool(tool);
        }
        builder.build().expect("build supervisor agent")
    }

    /// Drives the supervisor's streaming run to its terminal state. The
    /// streaming endpoint is what the session driver uses, so the supervisor's
    /// scripts land in `stream_requests` while the child's `run_full` drive
    /// lands in `chat_requests`.
    async fn drive_supervisor(agent: &mut Agent, prompt: &str) {
        let mut stream = agent.stream(prompt.to_owned()).await.expect("stream");
        while let Some(item) = stream.next().await {
            item.expect("supervisor run event");
        }
    }

    /// Runs `test` on a `current_thread` runtime inside a [`LocalSet`],
    /// mirroring the session actor's `!Send` discipline (`crate::session`):
    /// the tool handler's `tokio::task::spawn_local` only works there.
    fn run_local<T>(test: impl Future<Output = T>) -> T {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        tokio::task::LocalSet::new().block_on(&runtime, test)
    }

    /// Awaits the instance's terminal status with the race-free
    /// check → enable → re-check → wait pattern [`Instance::done`] mandates.
    async fn wait_terminal(instance: &Arc<Instance>) -> InstanceStatus {
        loop {
            let notified = instance.done().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let status = instance.status();
            if status.is_terminal() {
                return status;
            }
            tokio::time::timeout(Duration::from_secs(5), notified.as_mut())
                .await
                .expect("instance reaches a terminal status");
        }
    }

    /// Returns every event already buffered for `subscriber` without blocking.
    fn pending_events(subscriber: &mut EventStream) -> Vec<Event> {
        let mut events = Vec::new();
        while let Some(Some(event)) = subscriber.next().now_or_never() {
            events.push(event);
        }
        events
    }

    /// Awaits the next event matching `pred` (5s backstop; a hang is a bug).
    async fn next_matching(subscriber: &mut EventStream, pred: impl Fn(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = subscriber.next().await.expect("event stream open");
                if pred(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("matching event arrives within 5s")
    }

    /// Polls `pred` until it holds (5s backstop; a hang is a bug). Used to
    /// sync the test body with runs it drives concurrently on the LocalSet.
    async fn await_until(mut pred: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !pred() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("condition holds within 5s");
    }

    /// `(status, concatenated text)` of every tool result in one request.
    fn tool_results(request: &ChatRequest) -> Vec<(ToolStatus, String)> {
        request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult {
                    content, status, ..
                } => Some((
                    *status,
                    content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Concatenates the text blocks of one message.
    fn message_text(message: &agent_lib::model::message::Message) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn usage() -> Usage {
        Usage {
            input: 2,
            output: 1,
            total: Some(3),
            ..Usage::default()
        }
    }

    /// Scripts the standard two-step supervisor turn: call `agent`, then close
    /// with a final text.
    fn supervisor_scripts(agent_args: Value) -> Vec<StreamScript> {
        vec![
            StreamScript::Complete(tool_use_stream(AGENT_TOOL_NAME, "call-1", agent_args)),
            StreamScript::Complete(text_stream_with_usage(&["supervisor final"], usage())),
        ]
    }

    /// Routes child requests by their layered system prompt and everything
    /// else (the supervisor) to the catch-all.
    fn routed_client(
        child_scripts: Vec<StreamScript>,
        supervisor_scripts: Vec<StreamScript>,
    ) -> Arc<FakeLlmClient> {
        FakeLlmClient::scripted_routes(vec![
            RequestRoute::system_contains(SUBAGENT_SKELETON, child_scripts),
            RequestRoute::any(supervisor_scripts),
        ])
    }

    /// A canned tool plugin (same pattern as the delegation tests' `StubTool`).
    #[derive(Debug)]
    struct StubTool {
        name: &'static str,
        output: &'static str,
        permission: Option<PermissionSpec>,
    }

    #[async_trait::async_trait]
    impl ToolPlugin for StubTool {
        fn name(&self) -> &str {
            self.name
        }

        fn declaration(&self) -> ToolDecl {
            ToolDecl {
                name: self.name.to_owned(),
                description: format!("stub {} tool", self.name),
                input_schema: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
            ToolResult::text(self.output)
        }

        fn permission(&self) -> Option<PermissionSpec> {
            self.permission
        }
    }

    /// A stub tool that parks inside `invoke` until the test opens the gate
    /// (the "stub tool" use case [`StreamGate`] declares), giving the test
    /// deterministic control over when a child's in-flight run completes.
    #[derive(Debug)]
    struct GatedStubTool {
        gate: Arc<StreamGate>,
    }

    #[async_trait::async_trait]
    impl ToolPlugin for GatedStubTool {
        fn name(&self) -> &str {
            "gated_stub"
        }

        fn declaration(&self) -> ToolDecl {
            ToolDecl {
                name: self.name().to_owned(),
                description: "gated stub tool".to_owned(),
                input_schema: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn invoke(&self, _ctx: ToolContext, _args: Value) -> ToolResult {
            self.gate.wait().await;
            ToolResult::text("gated stub output")
        }

        fn permission(&self) -> Option<PermissionSpec> {
            None
        }
    }

    /// A registry with a gated `shell` and an auto-allowed `read_file`.
    fn gated_registry() -> ToolRegistry {
        ToolRegistry::new()
            .register(Arc::new(StubTool {
                name: "shell",
                output: "shell output",
                permission: Some(PermissionSpec::new(ToolCategory::Shell, ToolRisk::Medium)),
            }))
            .register(Arc::new(StubTool {
                name: "read_file",
                output: "file contents",
                permission: None,
            }))
    }

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// delegation tests' `TempConfigDir`).
    struct TempAgentsDir(PathBuf);

    impl TempAgentsDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mag-agents-{}-{nanos}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp agents dir");
            Self(path)
        }

        fn write(&self, file_name: &str, content: &str) {
            fs::write(self.0.join(file_name), content).expect("write agent definition");
        }

        /// The builtin table with this directory merged in as the user source.
        fn definitions(&self) -> AgentDefinitionRegistry {
            AgentDefinitionRegistry::merge(
                AgentDefinitionRegistry::builtin(),
                AgentDefinitionRegistry::load_user_dir(&self.0).expect("load temp agents dir"),
            )
        }
    }

    impl Drop for TempAgentsDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn spawn_returns_running_immediately_and_completes_with_report() {
        run_local(async {
            let client = routed_client(
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["report: found the answer"],
                    usage(),
                ))],
                supervisor_scripts(json!({
                    "type": "general-purpose",
                    "task": "solve the puzzle",
                    "description": "puzzle work",
                })),
            );
            let rig = rig(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            // The spawn tool answered immediately with the running envelope,
            // fed back to the supervisor's second request.
            let stream_requests = client.stream_requests();
            assert_eq!(stream_requests.len(), 2);
            let results = tool_results(&stream_requests[1]);
            assert!(
                results
                    .iter()
                    .any(|(status, text)| *status == ToolStatus::Ok
                        && text == r#"{"id":"general-purpose-1","status":"running"}"#),
                "the `agent` tool result is the running envelope: {results:?}"
            );

            let instance = rig
                .registry
                .get("general-purpose-1")
                .expect("instance registered");
            assert_eq!(instance.agent_type, "general-purpose");
            assert_eq!(instance.depth, 1);
            assert_eq!(
                wait_terminal(&instance).await,
                InstanceStatus::Completed {
                    report: "report: found the answer".to_owned(),
                }
            );

            // Lifecycle events arrived in order: Started, then Finished.
            let events = pending_events(&mut subscriber);
            let started_at = events
                .iter()
                .position(|event| matches!(event, Event::AgentInstanceStarted { .. }))
                .expect("started event");
            let finished_at = events
                .iter()
                .position(|event| matches!(event, Event::AgentInstanceFinished { .. }))
                .expect("finished event");
            assert!(started_at < finished_at);
            let Event::AgentInstanceStarted {
                id,
                instance_id,
                agent_type,
                description,
                depth,
            } = &events[started_at]
            else {
                unreachable!()
            };
            assert_eq!(*id, rig.ctx.session_id);
            assert_eq!(instance_id, "general-purpose-1");
            assert_eq!(agent_type, "general-purpose");
            assert_eq!(description.as_deref(), Some("puzzle work"));
            assert_eq!(*depth, 1);
            assert_eq!(
                events[finished_at],
                Event::AgentInstanceFinished {
                    id: rig.ctx.session_id,
                    instance_id: "general-purpose-1".to_owned(),
                    agent_type: "general-purpose".to_owned(),
                    status: AgentInstanceStatusWire::Completed,
                    report: Some("report: found the answer".to_owned()),
                    error: None,
                }
            );

            // The child's single (non-streaming) request: layered system
            // prompt, the task as opening user message, the supervisor's
            // model and max_tokens.
            let chat_requests = client.chat_requests();
            assert_eq!(chat_requests.len(), 1);
            let child = &chat_requests[0];
            let system = child.system.as_deref().expect("child system prompt");
            assert!(system.starts_with(SUBAGENT_SKELETON));
            assert!(
                system.contains("Handle the task end to end"),
                "the general-purpose body is the second layer: {system}"
            );
            assert_eq!(child.messages.len(), 1);
            assert_eq!(child.messages[0].role, Role::User);
            assert_eq!(message_text(&child.messages[0]), "solve the puzzle");
            assert_eq!(child.model, "supervisor-model");
            assert_eq!(child.max_tokens, 64);
        });
    }

    #[test]
    fn unknown_agent_type_errors_with_available_list() {
        run_local(async {
            let client = routed_client(
                Vec::new(),
                supervisor_scripts(json!({ "type": "ghost", "task": "haunt" })),
            );
            let rig = rig(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(Duration::from_secs(5), drive_supervisor(&mut agent, "go"))
                .await
                .expect("supervisor turn completes");

            let results = tool_results(&client.stream_requests()[1]);
            let (status, error) = results
                .iter()
                .find(|(_, text)| text.contains("unknown agent type"))
                .expect("unknown-type tool error");
            assert_eq!(*status, ToolStatus::Error);
            assert!(error.contains("`ghost`"), "names the bad type: {error}");
            assert!(
                error.contains("general-purpose") && error.contains("explorer"),
                "attaches the available list: {error}"
            );
            assert!(rig.registry.list().is_empty(), "no instance registered");
            assert!(
                pending_events(&mut subscriber)
                    .iter()
                    .all(|event| !matches!(
                        event,
                        Event::AgentInstanceStarted { .. } | Event::AgentInstanceFinished { .. }
                    )),
                "a refused spawn emits no lifecycle events"
            );
        });
    }

    #[test]
    fn missing_or_invalid_arguments_error() {
        run_local(async {
            let client = routed_client(
                Vec::new(),
                supervisor_scripts(json!({ "type": "general-purpose" })),
            );
            let rig = rig(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(Duration::from_secs(5), drive_supervisor(&mut agent, "go"))
                .await
                .expect("supervisor turn completes");

            let results = tool_results(&client.stream_requests()[1]);
            let (status, error) = results
                .iter()
                .find(|(_, text)| text.contains("`task`"))
                .expect("missing-task tool error");
            assert_eq!(*status, ToolStatus::Error);
            assert!(error.contains("required"), "explains the contract: {error}");
            assert!(rig.registry.list().is_empty());
        });
    }

    #[test]
    fn depth_limit_errors_synchronously() {
        run_local(async {
            let client = routed_client(
                Vec::new(),
                supervisor_scripts(json!({ "task": "go deeper" })),
            );
            // A spawner already at the cap must not derive a deeper instance.
            let rig = rig_with_depth(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
                MAX_INSTANCE_DEPTH,
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(Duration::from_secs(5), drive_supervisor(&mut agent, "go"))
                .await
                .expect("supervisor turn completes");

            let results = tool_results(&client.stream_requests()[1]);
            let (status, error) = results
                .iter()
                .find(|(_, text)| text.contains("depth limit"))
                .expect("depth-limit tool error");
            assert_eq!(*status, ToolStatus::Error);
            assert!(
                error.contains(&MAX_INSTANCE_DEPTH.to_string()),
                "names the limit: {error}"
            );
            assert!(rig.registry.list().is_empty());
            assert!(pending_events(&mut subscriber).is_empty());
        });
    }

    #[test]
    fn child_approval_bubbles_to_root_with_origin_and_resumes() {
        run_local(async {
            let client = routed_client(
                vec![
                    StreamScript::Complete(tool_use_stream(
                        "shell",
                        "child-shell-1",
                        json!({ "cmd": "ls" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(
                        &["report after approval"],
                        usage(),
                    )),
                ],
                supervisor_scripts(json!({ "task": "run the shell" })),
            );
            // The gated `shell` lands on the child's policy as `ask`.
            let rig = rig(
                &client,
                gated_registry(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");
            assert!(
                matches!(
                    rig.registry.get("general-purpose-1").map(|i| i.status()),
                    Some(InstanceStatus::Running)
                ),
                "the child parks on its approval while the supervisor's turn is over"
            );

            // The child's gated tool pause pops on the root session's event
            // stream with the instance's origin attribution.
            let event = next_matching(&mut subscriber, |event| {
                matches!(event, Event::InteractionRequested { .. })
            })
            .await;
            let Event::InteractionRequested {
                request_id,
                origin,
                kind,
                ..
            } = event
            else {
                unreachable!()
            };
            assert_eq!(origin.delegate.as_deref(), Some("general-purpose-1"));
            assert_eq!(origin.depth, 1);
            assert!(!origin.is_root());
            assert!(matches!(kind, InteractionKindWire::Approval { .. }));

            rig.ipc
                .respond(
                    request_id,
                    InteractionResponseWire::Approval {
                        step_id: StepIdWire::new(Uuid::nil()),
                        call_id: ToolCallIdWire::new(Uuid::nil()),
                        decision: ApprovalDecisionWire::Approve,
                        message: None,
                    },
                )
                .expect("respond approve");

            let instance = rig.registry.get("general-purpose-1").expect("instance");
            assert_eq!(
                wait_terminal(&instance).await,
                InstanceStatus::Completed {
                    report: "report after approval".to_owned(),
                }
            );

            // The approved tool genuinely ran: its result is folded into the
            // child's follow-up request.
            let chat_requests = client.chat_requests();
            assert_eq!(chat_requests.len(), 2);
            let results = tool_results(&chat_requests[1]);
            assert!(
                results
                    .iter()
                    .any(|(status, text)| *status == ToolStatus::Ok && text == "shell output"),
                "the gated tool executed after approval: {results:?}"
            );

            let event = next_matching(&mut subscriber, |event| {
                matches!(event, Event::AgentInstanceFinished { .. })
            })
            .await;
            assert_eq!(
                event,
                Event::AgentInstanceFinished {
                    id: rig.ctx.session_id,
                    instance_id: "general-purpose-1".to_owned(),
                    agent_type: "general-purpose".to_owned(),
                    status: AgentInstanceStatusWire::Completed,
                    report: Some("report after approval".to_owned()),
                    error: None,
                }
            );
        });
    }

    #[test]
    fn cancel_parked_instance_marks_it_cancelled() {
        run_local(async {
            let client = routed_client(
                vec![StreamScript::Complete(tool_use_stream(
                    "shell",
                    "child-shell-1",
                    json!({ "cmd": "ls" }),
                ))],
                supervisor_scripts(json!({ "task": "run the shell" })),
            );
            let rig = rig(
                &client,
                gated_registry(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            // The child parks on its approval; cancel the instance instead of
            // answering. The origin router's cancel wrapper resolves the
            // parked interaction conservatively and the drive ends cancelled.
            next_matching(&mut subscriber, |event| {
                matches!(event, Event::InteractionRequested { .. })
            })
            .await;
            assert_eq!(
                rig.registry.cancel("general-purpose-1"),
                Some(InstanceStatus::Cancelled)
            );

            let instance = rig.registry.get("general-purpose-1").expect("instance");
            assert!(instance.cancel_handle().is_cancelled());
            assert_eq!(wait_terminal(&instance).await, InstanceStatus::Cancelled);

            let event = next_matching(&mut subscriber, |event| {
                matches!(event, Event::AgentInstanceFinished { .. })
            })
            .await;
            assert_eq!(
                event,
                Event::AgentInstanceFinished {
                    id: rig.ctx.session_id,
                    instance_id: "general-purpose-1".to_owned(),
                    agent_type: "general-purpose".to_owned(),
                    status: AgentInstanceStatusWire::Cancelled,
                    report: None,
                    error: None,
                }
            );
            // The winning transition pushed exactly one notification; the
            // drive task's losing `complete` added none.
            assert_eq!(
                rig.registry.drain_notifications(),
                [crate::instances::InstanceNotification {
                    id: "general-purpose-1".to_owned(),
                    text: "agent instance general-purpose-1 cancelled".to_owned(),
                }]
            );
        });
    }

    #[test]
    fn child_run_failure_marks_instance_failed() {
        run_local(async {
            // The child route matches but has no scripts left: the fake
            // client errors the child's first request.
            let client = routed_client(Vec::new(), supervisor_scripts(json!({ "task": "doomed" })));
            let rig = rig(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            let instance = rig
                .registry
                .get("general-purpose-1")
                .expect("instance registered");
            let status = wait_terminal(&instance).await;
            let InstanceStatus::Failed { error } = status else {
                panic!("expected Failed, got {status:?}");
            };
            assert!(!error.is_empty(), "the failure carries a diagnostic");

            let event = next_matching(&mut subscriber, |event| {
                matches!(event, Event::AgentInstanceFinished { .. })
            })
            .await;
            let Event::AgentInstanceFinished {
                status,
                report,
                error: wire_error,
                ..
            } = event
            else {
                unreachable!()
            };
            assert_eq!(status, AgentInstanceStatusWire::Failed);
            assert_eq!(report, None);
            assert_eq!(wire_error, Some(error));
        });
    }

    #[test]
    fn agent_result_blocks_until_the_instance_completes() {
        run_local(async {
            let gate = StreamGate::new();
            let client = routed_client(
                vec![
                    // The child parks inside the gated stub tool until the test
                    // opens the gate, then closes with its report.
                    StreamScript::Complete(tool_use_stream(
                        "gated_stub",
                        "child-call-1",
                        json!({}),
                    )),
                    StreamScript::Complete(text_stream_with_usage(
                        &["report: gate-opened findings"],
                        usage(),
                    )),
                ],
                vec![
                    StreamScript::Complete(tool_use_stream(
                        AGENT_TOOL_NAME,
                        "call-1",
                        json!({ "task": "inspect the vault" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_RESULT_TOOL_NAME,
                        "call-2",
                        json!({ "id": "general-purpose-1" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["supervisor final"], usage())),
                ],
            );
            let rig = rig(
                &client,
                ToolRegistry::new().register(Arc::new(GatedStubTool {
                    gate: Arc::clone(&gate),
                })),
                AgentDefinitionRegistry::builtin(),
            );
            let agent = supervisor_agent(&rig);
            // Drive the supervisor concurrently so the test can control the
            // child's completion timing while `agent_result` is blocked.
            let drive = tokio::task::spawn_local(async move {
                let mut agent = agent;
                drive_supervisor(&mut agent, "please delegate").await;
            });

            // The supervisor's second request carries the `agent_result`
            // call; the child's first request is the gated stub tool call.
            await_until(|| client.stream_requests().len() == 2).await;
            await_until(|| client.chat_requests().len() == 1).await;
            assert_eq!(
                rig.registry
                    .get("general-purpose-1")
                    .map(|instance| instance.status()),
                Some(InstanceStatus::Running),
                "the child is parked inside the gated stub tool"
            );
            // A genuinely blocked `agent_result` does not let the supervisor
            // advance while the instance runs.
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(
                client.stream_requests().len(),
                2,
                "agent_result keeps blocking while the instance runs"
            );

            gate.open();
            tokio::time::timeout(Duration::from_secs(5), drive)
                .await
                .expect("supervisor turn completes")
                .expect("supervisor drive task");

            // Each request carries the full history; the new tool result of
            // the step is the last one.
            let (status, text) = tool_results(&client.stream_requests()[2])
                .last()
                .expect("agent_result tool result")
                .clone();
            assert_eq!(status, ToolStatus::Ok);
            assert_eq!(
                serde_json::from_str::<Value>(&text).expect("json tool result"),
                json!({
                    "id": "general-purpose-1",
                    "status": "completed",
                    "report": "report: gate-opened findings",
                })
            );
        });
    }

    #[test]
    fn agent_result_timeout_returns_running_without_failing() {
        run_local(async {
            let gate = StreamGate::new();
            let client = routed_client(
                // The child parks inside the gated stub tool and the test
                // never opens the gate: the instance is still running when
                // `agent_result` times out.
                vec![
                    StreamScript::Complete(tool_use_stream(
                        "gated_stub",
                        "child-call-1",
                        json!({}),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["unreached"], usage())),
                ],
                vec![
                    StreamScript::Complete(tool_use_stream(
                        AGENT_TOOL_NAME,
                        "call-1",
                        json!({ "task": "long work" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_RESULT_TOOL_NAME,
                        "call-2",
                        json!({ "id": "general-purpose-1", "timeout_secs": 0 }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["supervisor final"], usage())),
                ],
            );
            let rig = rig(
                &client,
                ToolRegistry::new().register(Arc::new(GatedStubTool {
                    gate: Arc::clone(&gate),
                })),
                AgentDefinitionRegistry::builtin(),
            );
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            let (status, text) = tool_results(&client.stream_requests()[2])
                .last()
                .expect("agent_result tool result")
                .clone();
            assert_eq!(status, ToolStatus::Ok, "a timeout is not a failure");
            assert_eq!(
                serde_json::from_str::<Value>(&text).expect("json tool result"),
                json!({ "id": "general-purpose-1", "status": "running" })
            );
            // The timeout did not transition the instance.
            assert_eq!(
                rig.registry
                    .get("general-purpose-1")
                    .map(|instance| instance.status()),
                Some(InstanceStatus::Running)
            );
        });
    }

    #[test]
    fn agent_cancel_then_result_returns_cancelled() {
        run_local(async {
            let client = routed_client(
                // The child parks on its gated `shell` approval; the cancel
                // resolves the parked interaction conservatively through the
                // origin router (no answer needed from the test).
                vec![StreamScript::Complete(tool_use_stream(
                    "shell",
                    "child-shell-1",
                    json!({ "cmd": "ls" }),
                ))],
                vec![
                    StreamScript::Complete(tool_use_stream(
                        AGENT_TOOL_NAME,
                        "call-1",
                        json!({ "task": "run the shell" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_CANCEL_TOOL_NAME,
                        "call-2",
                        json!({ "id": "general-purpose-1" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_RESULT_TOOL_NAME,
                        "call-3",
                        json!({ "id": "general-purpose-1" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["supervisor final"], usage())),
                ],
            );
            let rig = rig(
                &client,
                gated_registry(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut subscriber = rig.events.subscribe();
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 4);
            // Each request carries the full history; the new tool result of
            // the step is the last one.
            for request in [&requests[2], &requests[3]] {
                let (status, text) = tool_results(request)
                    .last()
                    .expect("instance tool result")
                    .clone();
                assert_eq!(status, ToolStatus::Ok);
                assert_eq!(
                    serde_json::from_str::<Value>(&text).expect("json tool result"),
                    json!({ "id": "general-purpose-1", "status": "cancelled" }),
                    "agent_cancel and agent_result both answer the terminal snapshot"
                );
            }

            let instance = rig
                .registry
                .get("general-purpose-1")
                .expect("instance registered");
            assert_eq!(instance.status(), InstanceStatus::Cancelled);
            assert!(instance.cancel_handle().is_cancelled());

            // The cancelled drive unwinds promptly (the router's cancel
            // wrapper resolves the parked interaction) and announces the
            // terminal state.
            let event = next_matching(&mut subscriber, |event| {
                matches!(event, Event::AgentInstanceFinished { .. })
            })
            .await;
            assert_eq!(
                event,
                Event::AgentInstanceFinished {
                    id: rig.ctx.session_id,
                    instance_id: "general-purpose-1".to_owned(),
                    agent_type: "general-purpose".to_owned(),
                    status: AgentInstanceStatusWire::Cancelled,
                    report: None,
                    error: None,
                }
            );
        });
    }

    #[test]
    fn unknown_instance_id_errors_with_instance_list() {
        run_local(async {
            let client = routed_client(
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["report"],
                    usage(),
                ))],
                vec![
                    // The registry is still empty: the error says so.
                    StreamScript::Complete(tool_use_stream(
                        AGENT_RESULT_TOOL_NAME,
                        "call-1",
                        json!({ "id": "ghost-1" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_TOOL_NAME,
                        "call-2",
                        json!({ "task": "t" }),
                    )),
                    // After the spawn the table is attached to the error.
                    StreamScript::Complete(tool_use_stream(
                        AGENT_RESULT_TOOL_NAME,
                        "call-3",
                        json!({ "id": "ghost-1" }),
                    )),
                    StreamScript::Complete(tool_use_stream(
                        AGENT_CANCEL_TOOL_NAME,
                        "call-4",
                        json!({ "id": "ghost-2" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["supervisor final"], usage())),
                ],
            );
            let rig = rig(
                &client,
                ToolRegistry::with_builtins(),
                AgentDefinitionRegistry::builtin(),
            );
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(
                Duration::from_secs(5),
                drive_supervisor(&mut agent, "please delegate"),
            )
            .await
            .expect("supervisor turn completes");

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 5);

            // Each request carries the full history; the new tool result of
            // the step is the last one.
            let (status, error) = tool_results(&requests[1])
                .last()
                .expect("agent_result unknown-id error")
                .clone();
            assert_eq!(status, ToolStatus::Error);
            assert!(error.contains("`ghost-1`"), "names the bad id: {error}");
            assert!(
                error.contains("no agent instances have been spawned"),
                "empty-table hint: {error}"
            );

            let (status, error) = tool_results(&requests[3])
                .last()
                .expect("agent_result unknown-id error")
                .clone();
            assert_eq!(status, ToolStatus::Error);
            assert!(
                error.contains("known agent instances:") && error.contains("general-purpose-1"),
                "attaches the instance table: {error}"
            );

            let (status, error) = tool_results(&requests[4])
                .last()
                .expect("agent_cancel unknown-id error")
                .clone();
            assert_eq!(status, ToolStatus::Error);
            assert!(
                error.contains("`ghost-2`") && error.contains("general-purpose-1"),
                "names the bad id and attaches the table: {error}"
            );

            // The failed lookups touched no instance.
            assert_eq!(rig.registry.list().len(), 1);
        });
    }

    /// Spawns one `agent_type` instance and returns the tool names the child
    /// advertised on its first LLM request (sorted). `surface` is the
    /// supervisor's plugin-surface filter (`None` = unconstrained).
    fn child_tool_names(
        tools: ToolRegistry,
        definitions: AgentDefinitionRegistry,
        agent_type: &str,
        surface: Option<Vec<String>>,
    ) -> Vec<String> {
        run_local(async move {
            let client = routed_client(
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["report"],
                    usage(),
                ))],
                supervisor_scripts(json!({ "type": agent_type, "task": "t" })),
            );
            let rig = rig_with_surface(&client, tools, definitions, 0, surface);
            let mut agent = supervisor_agent(&rig);

            tokio::time::timeout(Duration::from_secs(5), drive_supervisor(&mut agent, "go"))
                .await
                .expect("supervisor turn completes");
            let instance = rig
                .registry
                .get(&format!("{agent_type}-1"))
                .expect("instance registered");
            wait_terminal(&instance).await;

            let chat_requests = client.chat_requests();
            assert_eq!(chat_requests.len(), 1);
            let mut names: Vec<String> = chat_requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect();
            names.sort();
            names
        })
    }

    #[test]
    fn child_inherits_full_surface_when_tools_unset() {
        let names = child_tool_names(
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
            "general-purpose",
            None,
        );
        assert_eq!(
            names,
            [
                "agent",
                "agent_cancel",
                "agent_result",
                "ask_user",
                "grep",
                "list_dir",
                "read_file",
                "shell"
            ]
        );
    }

    #[test]
    fn child_surface_intersects_definition_tools() {
        let names = child_tool_names(
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
            "explorer",
            None,
        );
        // `tools = [read_file, list_dir, grep]` narrows the supervisor's
        // surface; the instance tool trio is always appended (§5.1/§5.4).
        assert_eq!(
            names,
            [
                "agent",
                "agent_cancel",
                "agent_result",
                "grep",
                "list_dir",
                "read_file"
            ]
        );
    }

    #[test]
    fn child_surface_skips_unknown_definition_tools() {
        let dir = TempAgentsDir::new();
        dir.write(
            "reader.md",
            "---\nname: reader\ndescription: reads files only\ntools: read_file, ghost\n---\nRead things carefully.\n",
        );
        let names = child_tool_names(
            ToolRegistry::with_builtins(),
            dir.definitions(),
            "reader",
            None,
        );
        assert_eq!(
            names,
            ["agent", "agent_cancel", "agent_result", "read_file"]
        );
    }

    /// §7's anti-escalation rule: a child never widens past the supervisor's
    /// own (binding-narrowed) surface — a definition with no `tools` inherits
    /// the narrowed surface, and an explicit `tools` allowlist is intersected
    /// with it. The instance tool trio is exempt (§5.1).
    #[test]
    fn child_surface_is_bounded_by_the_supervisor_surface() {
        let bound = || Some(vec!["read_file".to_owned(), "shell".to_owned()]);
        // The definition sets no `tools`: it inherits the supervisor's
        // narrowed surface, not the full registry (`ask_user`, `grep`,
        // `list_dir` are all out).
        let names = child_tool_names(
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
            "general-purpose",
            bound(),
        );
        assert_eq!(
            names,
            [
                "agent",
                "agent_cancel",
                "agent_result",
                "read_file",
                "shell"
            ]
        );

        // An explicit `tools` allowlist is intersected with the supervisor's
        // surface: `explorer` names `grep`/`list_dir`, which the supervisor
        // itself does not have, so they drop out.
        let names = child_tool_names(
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
            "explorer",
            bound(),
        );
        assert_eq!(
            names,
            ["agent", "agent_cancel", "agent_result", "read_file"]
        );
    }

    #[test]
    fn agent_tools_expose_spawn_result_and_cancel() {
        let rig = rig(
            &FakeLlmClient::scripted(Vec::new()),
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
        );
        let tools = agent_tools(&rig.ctx);
        assert_eq!(tools.len(), 3);

        let agent = tools
            .iter()
            .find(|tool| tool.name() == AGENT_TOOL_NAME)
            .expect("agent tool");
        assert!(agent.description().contains("Available agent types:"));
        assert!(agent.description().contains("general-purpose"));
        assert!(agent.description().contains("explorer"));
        assert_eq!(agent.input_schema()["required"], json!(["task"]));

        let result = tools
            .iter()
            .find(|tool| tool.name() == AGENT_RESULT_TOOL_NAME)
            .expect("agent_result tool");
        assert_eq!(result.input_schema()["required"], json!(["id"]));
        assert!(
            result.input_schema()["properties"]["timeout_secs"].is_object(),
            "the timeout parameter is declared"
        );

        let cancel = tools
            .iter()
            .find(|tool| tool.name() == AGENT_CANCEL_TOOL_NAME)
            .expect("agent_cancel tool");
        assert_eq!(cancel.input_schema()["required"], json!(["id"]));
    }

    #[test]
    fn layered_prompt_combines_skeleton_and_body() {
        assert_eq!(layered_system_prompt(""), SUBAGENT_SKELETON);
        assert_eq!(layered_system_prompt("  \n"), SUBAGENT_SKELETON);
        let layered = layered_system_prompt("Body text.");
        assert!(layered.starts_with(SUBAGENT_SKELETON));
        assert!(layered.ends_with("Body text."));
        assert_eq!(
            layered.len(),
            SUBAGENT_SKELETON.len() + 2 + "Body text.".len()
        );
    }

    /// External ACP instance tests (TODO M4-1, `docs/dyn-agents.md` §6): a
    /// `kind: acp` definition spawns one process per instance through the
    /// same `agent` tool, and the one-shot drive reclaims it at every
    /// terminal state. The fake-acp.sh 范式 follows the retired M3-5
    /// `engine.rs:4457` infrastructure, extended with `hang` (cancel path)
    /// and `permission` (origin bubbling) modes.
    #[cfg(all(unix, feature = "external-acp"))]
    mod external_acp {
        use std::path::Path;

        use mag_service::PermissionDecisionWire;

        use super::*;

        /// Writes the fake ACP agent script: a line-oriented JSON-RPC peer
        /// logging every received frame to `$1` (the log path), answering
        /// `initialize` / `session/new` / `session/prompt` in `$2` mode
        /// (`success`, `crash_prompt`, `hang`, `permission`), and exiting on
        /// `session/cancel` with a `SESSION_CANCELLED` marker. `$3` is the
        /// session id to advertise. Returns `(script, log)`.
        fn fake_acp_script(dir: &TempAgentsDir) -> (PathBuf, PathBuf) {
            use std::os::unix::fs::PermissionsExt;

            let script = dir.0.join("fake-acp.sh");
            fs::write(
                &script,
                r#"#!/bin/sh
set -eu
MAG_FAKE_ACP_LOG="$1"
mode="$2"
session="$3"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$MAG_FAKE_ACP_LOG"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"%s"}}\n' "$session"
      ;;
    *'"method":"session/prompt"'*)
      if [ "$mode" = "crash_prompt" ]; then exit 9; fi
      if [ "$mode" = "hang" ]; then continue; fi
      if [ "$mode" = "permission" ]; then
        printf '{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{"sessionId":"%s","toolCall":{"toolCallId":"call-1","title":"write src/x.rs"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}}\n' "$session"
        while IFS= read -r answer; do
          printf '%s\n' "$answer" >> "$MAG_FAKE_ACP_LOG"
          case "$answer" in
            *'"id":100'*) break ;;
          esac
        done
      fi
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"external summary"}}}}\n' "$session"
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
      ;;
    *'"method":"session/cancel"'*)
      printf '%s\n' 'SESSION_CANCELLED' >> "$MAG_FAKE_ACP_LOG"
      exit 0
      ;;
  esac
done
"#,
            )
            .expect("write fake ACP script");
            let mut permissions = fs::metadata(&script)
                .expect("fake ACP script metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).expect("chmod fake ACP script");
            (script, dir.0.join("fake-acp.log"))
        }

        /// Writes the `kind: acp` `peer` definition whose command runs the
        /// fake script with the log path and mode as arguments.
        fn write_peer_definition(dir: &TempAgentsDir, script: &Path, log: &Path, mode: &str) {
            dir.write(
                "peer.md",
                &format!(
                    "---\nname: peer\ndescription: fake ACP peer for tests\nkind: acp\ncommand: \
                     [{}, {}, {mode}, mag-fake-acp-session]\n---\nTask frame template.\n",
                    script.display(),
                    log.display(),
                ),
            );
        }

        /// Polls the fake's log until `needle` appears (5s backstop via
        /// [`await_until`]; a hang is a bug).
        async fn await_log_line(log: &Path, needle: &'static str) {
            await_until(|| {
                fs::read_to_string(log)
                    .map(|text| text.contains(needle))
                    .unwrap_or(false)
            })
            .await;
        }

        /// The one-shot reclamation observable: the terminal sweep's
        /// best-effort `session/cancel` reaches the fake, which logs it
        /// (raw frame and/or the `SESSION_CANCELLED` marker) and exits.
        async fn await_process_reclaimed(log: &Path) {
            await_until(|| {
                fs::read_to_string(log)
                    .map(|text| {
                        text.contains(r#""method":"session/cancel""#)
                            || text.contains("SESSION_CANCELLED")
                    })
                    .unwrap_or(false)
            })
            .await;
        }

        #[test]
        fn external_instance_completes_and_reclaims_the_process() {
            run_local(async {
                let dir = TempAgentsDir::new();
                let (script, log) = fake_acp_script(&dir);
                write_peer_definition(&dir, &script, &log, "success");
                let client = FakeLlmClient::scripted_streams(supervisor_scripts(json!({
                    "type": "peer",
                    "task": "inspect the vault",
                    "description": "acp work",
                })));
                let rig = rig(&client, ToolRegistry::with_builtins(), dir.definitions());
                let mut subscriber = rig.events.subscribe();
                let mut agent = supervisor_agent(&rig);

                tokio::time::timeout(
                    Duration::from_secs(5),
                    drive_supervisor(&mut agent, "please delegate"),
                )
                .await
                .expect("supervisor turn completes");

                let instance = rig.registry.get("peer-1").expect("instance registered");
                assert_eq!(
                    wait_terminal(&instance).await,
                    InstanceStatus::Completed {
                        report: "external summary".to_owned(),
                    },
                    "the peer's final message is the instance report"
                );

                // Lifecycle events in order, with the same wire shape as the
                // local path.
                let events = pending_events(&mut subscriber);
                let started_at = events
                    .iter()
                    .position(|event| matches!(event, Event::AgentInstanceStarted { .. }))
                    .expect("started event");
                let finished_at = events
                    .iter()
                    .position(|event| matches!(event, Event::AgentInstanceFinished { .. }))
                    .expect("finished event");
                assert!(started_at < finished_at);
                let Event::AgentInstanceStarted {
                    id,
                    instance_id,
                    agent_type,
                    description,
                    depth,
                } = &events[started_at]
                else {
                    unreachable!()
                };
                assert_eq!(*id, rig.ctx.session_id);
                assert_eq!(instance_id, "peer-1");
                assert_eq!(agent_type, "peer");
                assert_eq!(description.as_deref(), Some("acp work"));
                assert_eq!(*depth, 1);
                assert_eq!(
                    events[finished_at],
                    Event::AgentInstanceFinished {
                        id: rig.ctx.session_id,
                        instance_id: "peer-1".to_owned(),
                        agent_type: "peer".to_owned(),
                        status: AgentInstanceStatusWire::Completed,
                        report: Some("external summary".to_owned()),
                        error: None,
                    }
                );

                // The process was driven through the ACP handshake; the
                // opening prompt carries the definition body as the
                // task-frame template joined to the caller's task (§6).
                let log_text = fs::read_to_string(&log).expect("fake ACP log");
                assert!(
                    log_text.contains(r#""method":"initialize""#)
                        && log_text.contains(r#""method":"session/new""#)
                        && log_text.contains(r#""method":"session/prompt""#),
                    "the ACP process was driven: {log_text}"
                );
                assert!(
                    log_text.contains("Task frame template.\\n\\ninspect the vault"),
                    "body template joined to the task: {log_text}"
                );

                await_process_reclaimed(&log).await;
            });
        }

        #[test]
        fn external_instance_cancel_abandons_and_reclaims_the_process() {
            run_local(async {
                let dir = TempAgentsDir::new();
                let (script, log) = fake_acp_script(&dir);
                write_peer_definition(&dir, &script, &log, "hang");
                let client = FakeLlmClient::scripted_streams(supervisor_scripts(json!({
                    "type": "peer",
                    "task": "hang on the prompt",
                })));
                let rig = rig(&client, ToolRegistry::with_builtins(), dir.definitions());
                let mut subscriber = rig.events.subscribe();
                let mut agent = supervisor_agent(&rig);

                tokio::time::timeout(
                    Duration::from_secs(5),
                    drive_supervisor(&mut agent, "please delegate"),
                )
                .await
                .expect("supervisor turn completes");

                let instance = rig.registry.get("peer-1").expect("instance registered");
                // The fake received the prompt and is hanging on it.
                await_log_line(&log, r#""method":"session/prompt""#).await;
                assert_eq!(instance.status(), InstanceStatus::Running);

                assert_eq!(
                    rig.registry.cancel("peer-1"),
                    Some(InstanceStatus::Cancelled)
                );
                assert!(instance.cancel_handle().is_cancelled());
                assert_eq!(wait_terminal(&instance).await, InstanceStatus::Cancelled);

                let event = next_matching(&mut subscriber, |event| {
                    matches!(event, Event::AgentInstanceFinished { .. })
                })
                .await;
                assert_eq!(
                    event,
                    Event::AgentInstanceFinished {
                        id: rig.ctx.session_id,
                        instance_id: "peer-1".to_owned(),
                        agent_type: "peer".to_owned(),
                        status: AgentInstanceStatusWire::Cancelled,
                        report: None,
                        error: None,
                    }
                );

                await_process_reclaimed(&log).await;
            });
        }

        #[test]
        fn external_permission_request_bubbles_to_root_with_origin() {
            run_local(async {
                let dir = TempAgentsDir::new();
                let (script, log) = fake_acp_script(&dir);
                write_peer_definition(&dir, &script, &log, "permission");
                let client = FakeLlmClient::scripted_streams(supervisor_scripts(json!({
                    "type": "peer",
                    "task": "write the file",
                })));
                let rig = rig(&client, ToolRegistry::with_builtins(), dir.definitions());
                let mut subscriber = rig.events.subscribe();
                let mut agent = supervisor_agent(&rig);

                tokio::time::timeout(
                    Duration::from_secs(5),
                    drive_supervisor(&mut agent, "please delegate"),
                )
                .await
                .expect("supervisor turn completes");

                // The peer's ACP `session/request_permission` pops on the root
                // session's event stream with the instance's origin.
                let event = next_matching(&mut subscriber, |event| {
                    matches!(event, Event::InteractionRequested { .. })
                })
                .await;
                let Event::InteractionRequested {
                    request_id,
                    origin,
                    kind,
                    ..
                } = event
                else {
                    unreachable!()
                };
                assert_eq!(origin.delegate.as_deref(), Some("peer-1"));
                assert_eq!(origin.depth, 1);
                assert!(!origin.is_root());
                let InteractionKindWire::Permission { action_id, .. } = kind else {
                    panic!("expected a Permission interaction, got {kind:?}");
                };

                rig.ipc
                    .respond(
                        request_id,
                        InteractionResponseWire::Permission {
                            action_id,
                            decision: PermissionDecisionWire::Approve,
                        },
                    )
                    .expect("respond approve");

                let instance = rig.registry.get("peer-1").expect("instance registered");
                assert_eq!(
                    wait_terminal(&instance).await,
                    InstanceStatus::Completed {
                        report: "external summary".to_owned(),
                    },
                    "the approved peer resumed and completed"
                );
                // The fake received the host's answer before finishing.
                let log_text = fs::read_to_string(&log).expect("fake ACP log");
                assert!(
                    log_text.contains(r#""id":100"#),
                    "the host's permission answer reached the peer: {log_text}"
                );

                await_process_reclaimed(&log).await;
            });
        }

        #[test]
        fn external_process_crash_marks_the_instance_failed() {
            run_local(async {
                let dir = TempAgentsDir::new();
                let (script, log) = fake_acp_script(&dir);
                write_peer_definition(&dir, &script, &log, "crash_prompt");
                let client = FakeLlmClient::scripted_streams(supervisor_scripts(json!({
                    "type": "peer",
                    "task": "doomed",
                })));
                let rig = rig(&client, ToolRegistry::with_builtins(), dir.definitions());
                let mut subscriber = rig.events.subscribe();
                let mut agent = supervisor_agent(&rig);

                tokio::time::timeout(
                    Duration::from_secs(5),
                    drive_supervisor(&mut agent, "please delegate"),
                )
                .await
                .expect("supervisor turn completes");

                let instance = rig.registry.get("peer-1").expect("instance registered");
                let status = wait_terminal(&instance).await;
                let InstanceStatus::Failed { error } = status else {
                    panic!("expected Failed, got {status:?}");
                };
                assert!(!error.is_empty(), "the failure carries a diagnostic");

                let event = next_matching(&mut subscriber, |event| {
                    matches!(event, Event::AgentInstanceFinished { .. })
                })
                .await;
                let Event::AgentInstanceFinished {
                    status,
                    report,
                    error: wire_error,
                    ..
                } = event
                else {
                    unreachable!()
                };
                assert_eq!(status, AgentInstanceStatusWire::Failed);
                assert_eq!(report, None);
                assert_eq!(wire_error, Some(error));

                // The drive failing at all proves the crashed process is
                // gone: a live-but-mute peer would leave the drive parked on
                // its prompt read and `wait_terminal` would time out.
            });
        }
    }
}
