//! The `agent` spawn tool and the local-instance drive task
//! (`docs/dyn-agents.md` §4/§5, TODO M3-3).
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
//! The drive task assembles the child facade agent from the definition: the
//! layered system prompt ([`SUBAGENT_SKELETON`] + definition body, §4), the
//! supervisor's tool surface filtered by the definition's `tools` allowlist
//! plus the `agent` tool itself for nesting (§5.4/§7), the supervisor's
//! approval-policy projection, and an [`OriginRouter`] that bubbles every
//! paused interaction to the root session's [`IpcApproval`] with the
//! instance's attribution (§4/§7). The run's final assistant text becomes the
//! instance report (the report contract of §4); the terminal transition goes
//! through [`AgentInstanceRegistry::complete`] and is announced as
//! [`Event::AgentInstanceFinished`].

use std::{convert::Infallible, sync::Arc};

use agent_lib::{
    agent::{
        ApprovalDecision, ApprovalResponse, Interaction, InteractionHandler, InteractionKind,
        InteractionOrigin, InteractionResponse, PermissionResponse, RequirementResult, RunContext,
        WorktreeRef,
    },
    client::LlmClient,
    facade::{Agent, ModelRef, Tool, ToolContext, ToolResult},
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

/// Name of the spawn tool (§5.1, D5); `agent_result` / `agent_cancel` land in
/// M3-4 and extend [`agent_tools`].
pub(crate) const AGENT_TOOL_NAME: &str = "agent";

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

/// Everything the `agent` tool needs to spawn and drive one instance, shared
/// behind an [`Arc`] between the tool handler and every drive task it starts.
///
/// One context belongs to one *spawning* agent: the session's root supervisor
/// holds the depth-`0` context (assembled by the session driver in M3-5), and
/// each spawned instance gets [`child_context`](Self::child_context) — the
/// same shared handles at depth + 1 — for its own `agent` tool, which is how
/// nesting (§5.4) and the depth cap are implemented. The client, tool
/// registry, approval overrides, interaction handler, and event bus are the
/// supervisor's own handles, so a child runs on the same LLM client, tool
/// surface, approval authority, and session event stream as its parent.
#[derive(Clone)]
pub(crate) struct InstanceSpawnContext {
    /// Instance table shared session-wide.
    pub(crate) registry: AgentInstanceRegistry,
    /// Merged definition table the `type` parameter resolves against.
    pub(crate) definitions: AgentDefinitionRegistry,
    /// LLM client shared with the supervisor (children never build their own).
    pub(crate) client: Arc<dyn LlmClient>,
    /// The supervisor's effective model; a definition without `model`
    /// inherits it, and `max_tokens` always aligns with it.
    pub(crate) supervisor_model: ModelRef,
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
            .field("definitions", &self.definitions)
            .field("client", &"<dyn LlmClient>")
            .field("supervisor_model", &self.supervisor_model)
            .field("tools", &self.tools)
            .field("overrides", &self.overrides)
            .field("events", &self.events)
            .field("session_id", &self.session_id)
            .field("worktree", &self.worktree)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

/// The instance tools available on one spawning agent's tool surface.
///
/// Currently just the [`AGENT_TOOL_NAME`] spawn tool (M3-3); M3-4 appends
/// `agent_result` and `agent_cancel` here so the supervisor and every child
/// surface gain them together (§5.1). The child surface reserves the same
/// extension point: [`drive_local`] appends `agent_tools` of the instance's
/// own context after its projected plugins.
// Wired into the session tool surface in M3-5; until then only tests and the
// child-surface assembly below consume this.
#[allow(dead_code)]
pub(crate) fn agent_tools(ctx: &Arc<InstanceSpawnContext>) -> Vec<Tool> {
    vec![agent_tool(ctx)]
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
        ctx.definitions.describe_for_tool()
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

    let Some(definition) = ctx.definitions.get(agent_type) else {
        return ToolResult::error(format!(
            "unknown agent type `{agent_type}`\n\n{}",
            ctx.definitions.describe_for_tool()
        ));
    };
    if ctx.depth >= MAX_INSTANCE_DEPTH {
        return ToolResult::error(format!(
            "agent nesting depth limit ({MAX_INSTANCE_DEPTH}) reached: this agent cannot spawn \
             further instances; finish the task directly or report the blocker"
        ));
    }
    if !matches!(definition.kind, AgentKindDef::Local { .. }) {
        return ToolResult::error(format!(
            "agent type `{agent_type}` is external (kind: acp); external instances are not \
             supported yet (they land in M4)"
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
    tokio::task::spawn_local(drive_instance(
        Arc::clone(ctx),
        definition.clone(),
        instance,
        task,
    ));
    ToolResult::text(json!({ "id": id, "status": "running" }).to_string())
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
    let result = drive_local(&ctx, &definition, &instance, &task).await;
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
        // The handler filters external definitions synchronously, so this is
        // unreachable until M4 routes them to their own drive path.
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
    // §7: the child surface is the supervisor's projection intersected with
    // the definition's allowlist (`None` inherits everything), plus the
    // instance tools of the child's own context for nesting (§5.4).
    let (mut surface, policy) = project_tool_plugins(
        &ctx.tools,
        tools.as_deref(),
        ctx.overrides.default_tier(),
        user_interaction,
    );
    let policy = apply_per_tool_tiers(policy, &ctx.overrides);
    surface.extend(agent_tools(&ctx.child_context()));

    let mut builder = Agent::builder()
        .client(Arc::clone(&ctx.client))
        .model(
            model
                .clone()
                .unwrap_or_else(|| ctx.supervisor_model.model().to_owned()),
        )
        .max_tokens(ctx.supervisor_model.max_tokens().get())
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
        AGENT_TOOL_NAME, InstanceSpawnContext, MAX_INSTANCE_DEPTH, SUBAGENT_SKELETON, agent_tools,
        layered_system_prompt,
    };
    use crate::{
        EventBus, EventStream,
        assembly::ApprovalOverrides,
        engine::approval::{AskFrontendDecider, IpcApproval},
        instances::{AgentInstanceRegistry, Instance, InstanceStatus},
        test_support::{
            FakeLlmClient, RequestRoute, StreamScript, text_stream_with_usage, tool_use_stream,
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
            definitions,
            client: client.clone() as Arc<dyn LlmClient>,
            supervisor_model: ModelRef::new(
                "supervisor-model",
                NonZeroU32::new(64).expect("non-zero"),
                None,
                None,
            ),
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

    /// A supervisor facade agent whose only tool is `agent` (the surface the
    /// session driver will assemble in M3-5).
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
                ["agent instance general-purpose-1 cancelled".to_owned()]
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

    /// Spawns one `agent_type` instance and returns the tool names the child
    /// advertised on its first LLM request (sorted).
    fn child_tool_names(
        tools: ToolRegistry,
        definitions: AgentDefinitionRegistry,
        agent_type: &str,
    ) -> Vec<String> {
        run_local(async move {
            let client = routed_client(
                vec![StreamScript::Complete(text_stream_with_usage(
                    &["report"],
                    usage(),
                ))],
                supervisor_scripts(json!({ "type": agent_type, "task": "t" })),
            );
            let rig = rig(&client, tools, definitions);
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
        );
        assert_eq!(
            names,
            [
                "agent",
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
        );
        // `tools = [read_file, list_dir, grep]` narrows the supervisor's
        // surface; the `agent` tool itself is always appended (§5.4).
        assert_eq!(names, ["agent", "grep", "list_dir", "read_file"]);
    }

    #[test]
    fn child_surface_skips_unknown_definition_tools() {
        let dir = TempAgentsDir::new();
        dir.write(
            "reader.md",
            "---\nname: reader\ndescription: reads files only\ntools: read_file, ghost\n---\nRead things carefully.\n",
        );
        let names = child_tool_names(ToolRegistry::with_builtins(), dir.definitions(), "reader");
        assert_eq!(names, ["agent", "read_file"]);
    }

    #[test]
    fn agent_tool_description_enumerates_definitions_and_requires_task() {
        let rig = rig(
            &FakeLlmClient::scripted(Vec::new()),
            ToolRegistry::with_builtins(),
            AgentDefinitionRegistry::builtin(),
        );
        let tools = agent_tools(&rig.ctx);
        assert_eq!(tools.len(), 1, "M3-4 appends agent_result/agent_cancel");
        let tool = &tools[0];
        assert_eq!(tool.name(), AGENT_TOOL_NAME);
        assert!(tool.description().contains("Available agent types:"));
        assert!(tool.description().contains("general-purpose"));
        assert!(tool.description().contains("explorer"));
        assert_eq!(tool.input_schema()["required"], json!(["task"]));
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
}
