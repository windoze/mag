//! Single-turn driver wiring mag sessions to the facade [`Agent`].
//!
//! Since agent-lib Milestone 7 exposed the host injection surface, mag no longer
//! assembles its own [`HandlerScope`](agent_lib::agent::HandlerScope) /
//! [`drain`](agent_lib::agent::drain) loop. A session owns one facade
//! [`Agent`], and each turn is driven by consuming
//! [`Agent::stream`](agent_lib::facade::Agent::stream): every incremental
//! [`RunEvent`](agent_lib::facade::RunEvent) is projected through the official
//! [`RunEvent::to_wire`](agent_lib::facade::RunEvent::to_wire) bridge and mapped
//! into a mag [`Event`].
//!
//! The agent is assembled with the session's tool surface and approval gate
//! (`docs/DESIGN.md` §3.2/§3.3): every [`ToolPlugin`] from the [`ToolRegistry`]
//! is projected into a facade [`Tool`] via
//! [`Tool::function_with_schema`](agent_lib::facade::Tool::function_with_schema)
//! (the facade injects the run-scoped [`ToolContext`] per call), and each tool
//! that declares [`permission`](ToolPlugin::permission) is gated behind
//! [`ApprovalPolicy::ask_tool`] so it pauses through the injected
//! [`IpcApproval`]. The `ask_user` plugin is permission-free but still uses the
//! same `IpcApproval` instance directly through a host bridge to emit
//! `Question` / `Choice` interactions (`docs/CLI.md` §5 P6).

use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock, PoisonError,
    atomic::{AtomicU64, Ordering},
};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_lib::{
    agent::{
        ApprovalDecision, BudgetLimits, Interaction, InteractionHandler, InteractionResponse,
        PivotMessage, PivotSource, RequirementResult, RunContext, StepId, TraceNodeId, WorktreeRef,
    },
    client::LlmClient,
    conversation::MessageId,
    facade::{
        Agent, AgentRunStream, AgentSnapshot, Approval, ApprovalPolicy, CancelHandle,
        DelegationMessage as FacadeDelegationMessage, DelegationTrace as FacadeDelegationTrace,
        FacadeError, ModelRef, ReconfigRequest, Tool, ToolContext, ToolResult, ToolSetId,
        ToolSetRef, ToolTrace as FacadeToolTrace, UsageSummary, WireRunEvent, WireRunOutput,
    },
    model::{
        content::ContentBlock,
        message::{Message, Role},
    },
};
use mag_config::{
    AgentDefinitionRegistry, ApprovalPolicyKind, ConfigSnapshot, default_user_agents_dir,
    project_agents_dir,
};
use mag_service::{
    DelegationMessageWire, DelegationStatusWire, DelegationTrace, Event, RunErrorKind,
    RunId as WireRunId, RunOutput, SessionBudget, SessionConfig, SessionId, ToolCallIdWire,
    ToolStatusWire, ToolTrace, UsageInfo,
};
use mag_tools::{
    ToolInvocation, ToolPlugin, ToolRegistry, UserInteractionBridge, UserInteractionError,
    UserInteractionRequest, UserInteractionResponse,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    EventBus,
    assembly::{ApprovalOverrides, SessionBinding},
    engine::approval::IpcApproval,
    instances::{
        AgentInstanceRegistry, InstanceNotification,
        spawn::{InstanceSpawnContext, SharedSpawnState, agent_tools},
    },
    persistence::Persistence,
    turn_complete::{TurnCompleteHub, TurnCompletion, TurnSummary},
};

const DEFAULT_MAX_TOKENS: u32 = 512;
const DEFAULT_MAX_STEPS: u32 = 8;

/// Shared pivot queue bridging one in-flight run and its session actor
/// (`docs/CLI.md` §3.2, decision D1).
///
/// This is the pivot counterpart of the [`CancelHandle`] side-channel: the
/// session actor pushes user pivot messages into the queue while
/// [`SessionDriver::run_turn`] — the only owner of the run's mutable
/// [`AgentRunStream`] — drains it after every stream poll and tries
/// [`AgentRunStream::interject`]. The lock is synchronous, never held across
/// an `.await`, and poisoning is recovered rather than propagated, mirroring
/// the codebase's unified poison-recovery policy.
#[derive(Clone, Debug, Default)]
pub(crate) struct PivotQueue {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl PivotQueue {
    /// Creates an empty pivot queue for one run.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Queues one pivot message (FIFO order is preserved for draining).
    pub(crate) fn push(&self, text: String) {
        self.lock().push_back(text);
    }

    /// Pops the oldest queued pivot, if any.
    fn pop_front(&self) -> Option<String> {
        self.lock().pop_front()
    }

    /// Returns a pivot to the front of the queue after a retryable rejection.
    fn push_front(&self, text: String) {
        self.lock().push_front(text);
    }

    /// Removes and returns every queued pivot, preserving FIFO order.
    pub(crate) fn drain(&self) -> Vec<String> {
        self.lock().drain(..).collect()
    }

    /// Locks the queue, recovering the guard from a poisoned lock.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One session's stateful facade [`Agent`] plus a run-id source.
///
/// The facade [`Agent`] holds the session's conversation, so reusing one driver
/// across turns accumulates history without mag reassembling any state.
#[derive(Debug)]
pub(crate) struct SessionDriver {
    agent: Agent,
    /// Executable tool surface the agent was built with; consulted again at
    /// config-apply time to project a filtered [`ReplaceToolSet`] declaration
    /// list (`docs/CLI.md` §4.4).
    tools: Arc<ToolRegistry>,
    /// Turn-complete hook point: notified once after every run terminal
    /// (`docs/CLI.md` §4.5).
    turn_complete: TurnCompleteHub,
    /// The `agents.<name>` entry this session is bound to; `apply_config`
    /// reconfigurations read that entry (`docs/CLI.md` §4.4).
    agent_name: String,
    /// Config-controlled system prompt target, queued as a mutable overlay at
    /// the next turn start so it can be replaced or cleared by `apply_config`.
    system_prompt: Option<String>,
    /// The root supervisor's instance spawn context (depth `0`,
    /// `docs/dyn-agents.md` §5): the session's instance registry, the shared
    /// spawn state (definition table + supervisor model + session default
    /// subagent toolset), and every handle the `agent` tool trio captured
    /// when it was appended to the tool surface. Held for the cancel cascade
    /// and the `apply_config` definition/toolset rebuild.
    spawn_ctx: Arc<InstanceSpawnContext>,
    /// The session's configured working directory, retained so `apply_config`
    /// can re-load the project-level definition directory (M3-5).
    cwd: Option<PathBuf>,
    /// Instance notifications drained from the registry but not yet landed on
    /// a pivot window (M3-6): retried at every stream poll while the run is in
    /// flight, and — unlike user pivots — never dropped at the run's end; the
    /// leftovers are prefixed onto the next turn's user input.
    pending_notifications: VecDeque<InstanceNotification>,
    run_counter: AtomicU64,
    /// Mints fresh tool-set identities for `apply_config` reconfigurations.
    tool_set_counter: AtomicU64,
}

impl SessionDriver {
    /// Builds a fresh facade [`Agent`] for the supplied session configuration.
    ///
    /// The `tools` registry is projected onto the agent through
    /// [`tool_surface`]: `binding` narrows the surface to the bound
    /// `agents.<name>` entry's enabled tool list when it sets one
    /// (`docs/CLI.md` §4.4: new sessions use the current DO graph), each
    /// remaining plugin declaring a [`permission`](ToolPlugin::permission) is
    /// gated behind [`ApprovalPolicy::ask_tool`] so it pauses through
    /// `approval`, and `overrides` applies the configured `[approval]` /
    /// `[tools.<name>].approval` tiers on top. The shared [`IpcApproval`] is
    /// injected as the interaction handler and stays the sole authority
    /// answering a paused tool call (`docs/DESIGN.md` §3.3).
    ///
    /// The bound entry's `model` / `system_prompt` override the wire
    /// [`SessionConfig`] values when set; the wire values are the fallback
    /// (session ↔ agent binding, see [`Engine::from_config`](crate::Engine::from_config)).
    ///
    /// When [`config.cwd`](SessionConfig::cwd) is set, that path becomes the
    /// agent's [`WorktreeRef`] so the built-in tools resolve relative to the
    /// interface-supplied session root (for ACP, the client's `cwd`,
    /// `docs/ACP.md` §3.2/§6); a `None` cwd keeps the facade default `"."`.
    ///
    /// The surface also carries the `agent` / `agent_result` / `agent_cancel`
    /// instance tools (`docs/dyn-agents.md` §5.1, M3-5), built over the
    /// session's root [`InstanceSpawnContext`]: the session's instance
    /// registry, the four-layer definition table (builtin → user dir →
    /// project dir at the session cwd → the binding's TOML layer), the shared
    /// LLM client, and the supervisor's model. Spawning is asynchronous, so a
    /// delegated run drives on the same handles as the supervisor itself.
    ///
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while assembling the agent (for
    /// example an invalid model/provider configuration).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: &SessionConfig,
        client: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        approval: Arc<IpcApproval>,
        turn_complete: TurnCompleteHub,
        binding: &SessionBinding,
        overrides: &ApprovalOverrides,
        session_id: SessionId,
        events: EventBus,
    ) -> Result<Self, FacadeError> {
        let user_interaction = Arc::new(IpcUserInteractionBridge::new(
            approval.clone() as Arc<dyn InteractionHandler>
        )) as Arc<dyn UserInteractionBridge>;
        let model = binding
            .model()
            .map(str::to_owned)
            .unwrap_or_else(|| config.model.clone());
        let spawn_ctx = Arc::new(root_spawn_context(
            config,
            client.clone(),
            tools.clone(),
            approval.clone(),
            binding,
            overrides,
            session_id,
            events,
            ModelRef::new(
                model.clone(),
                NonZeroU32::new(DEFAULT_MAX_TOKENS).expect("non-zero default"),
                None,
                None,
            ),
        ));
        let (facade_tools, policy) =
            tool_surface(&tools, binding, overrides, user_interaction, &spawn_ctx);
        let mut builder = Agent::builder()
            .client(client)
            .model(model)
            .max_tokens(DEFAULT_MAX_TOKENS)
            .max_steps(DEFAULT_MAX_STEPS)
            .interaction_handler(approval as Arc<dyn InteractionHandler>);
        if let Some(cwd) = &config.cwd {
            builder = builder.worktree(WorktreeRef::new(cwd.clone()));
        }
        if let Some(budget) = &config.budget {
            builder = builder.budget(budget_limits(budget));
        }
        for tool in facade_tools {
            builder = builder.tool(tool);
        }
        let agent = builder.approval(policy).build()?;
        // Correct the spawn-context model to the built agent's authoritative
        // one (identical by construction here, authoritative on restore).
        spawn_ctx
            .shared
            .set_supervisor_model(agent.state().current_model().clone());

        Ok(Self {
            agent,
            tools,
            turn_complete,
            agent_name: binding.agent_name().to_owned(),
            system_prompt: binding.system_prompt().map(str::to_owned),
            spawn_ctx,
            cwd: config.cwd.clone(),
            pending_notifications: VecDeque::new(),
            run_counter: AtomicU64::new(1),
            tool_set_counter: AtomicU64::new(1),
        })
    }

    /// Rebuilds a session's facade [`Agent`] from a persisted [`AgentSnapshot`].
    ///
    /// A snapshot is data-only (`docs/DESIGN.md` §3.6): the LLM `client`, the
    /// executable `tools`, the [`ApprovalPolicy`], and the [`IpcApproval`]
    /// interaction handler are all runtime handles the snapshot deliberately
    /// omits, so they are re-injected here exactly as [`new`](Self::new) supplies
    /// them for a fresh agent. Re-injecting the shared [`IpcApproval`] keeps a
    /// restored session on the cross-process approval path (`PLAN.md` R-B); the
    /// restored [`AgentState`](agent_lib::agent) carries the conversation, model,
    /// and loop policy, so a resumed run continues exactly where the snapshot left
    /// off. The gated-tool policy is re-derived from the same `tools` registry so
    /// a restored session pauses on the same tools it did before.
    ///
    /// The session's configured per-run `budget` (if any) is re-applied exactly
    /// as [`new`](Self::new) applies it: a snapshot is data-only and deliberately
    /// omits the budget limits, so a restored session enforces the same
    /// [`SessionBudget`] it was created with.
    ///
    /// The `agent` tool trio is re-injected exactly as in [`new`](Self::new):
    /// instances are ephemeral and never persisted (`docs/dyn-agents.md`
    /// §5.2), so a restored session starts with a fresh instance registry and
    /// a freshly merged definition table. A snapshot written before the static
    /// delegation path was retired (M3-5) may still carry persisted delegate
    /// recipes and their synthesized `ask_<name>` declarations; restore never
    /// re-registers any delegate, and
    /// [`AgentRestoreBuilder::prune_unregistered_delegates`](agent_lib::facade::AgentRestoreBuilder::prune_unregistered_delegates)
    /// is kept precisely so those legacy entries are dropped — silently —
    /// instead of being resurrected under agent-lib's default merge semantics
    /// with an approval-free fallback policy (with zero re-registrations the
    /// prune is a pure one-way sweep of the old roster; agent-lib's surface
    /// offers no other way to keep a legacy snapshot both restorable and
    /// delegate-free).
    ///
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while rebuilding the agent (for example
    /// a snapshot whose state cannot be deserialized).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn restore(
        config: &SessionConfig,
        client: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        approval: Arc<IpcApproval>,
        snapshot: AgentSnapshot,
        budget: Option<&SessionBudget>,
        turn_complete: TurnCompleteHub,
        binding: &SessionBinding,
        overrides: &ApprovalOverrides,
        session_id: SessionId,
        events: EventBus,
    ) -> Result<Self, FacadeError> {
        let user_interaction = Arc::new(IpcUserInteractionBridge::new(
            approval.clone() as Arc<dyn InteractionHandler>
        )) as Arc<dyn UserInteractionBridge>;
        let spawn_ctx = Arc::new(root_spawn_context(
            config,
            client.clone(),
            tools.clone(),
            approval.clone(),
            binding,
            overrides,
            session_id,
            events,
            ModelRef::new(
                binding
                    .model()
                    .map(str::to_owned)
                    .unwrap_or_else(|| config.model.clone()),
                NonZeroU32::new(DEFAULT_MAX_TOKENS).expect("non-zero default"),
                None,
                None,
            ),
        ));
        let (facade_tools, policy) =
            tool_surface(&tools, binding, overrides, user_interaction, &spawn_ctx);
        let mut builder = Agent::restore()
            .snapshot(snapshot)
            .client(client)
            .interaction_handler(approval as Arc<dyn InteractionHandler>);
        if let Some(budget) = budget {
            builder = builder.budget(budget_limits(budget));
        }
        for tool in facade_tools {
            builder = builder.tool(tool);
        }
        // Restore never re-registers a delegate (the static delegation path is
        // retired, M3-5): with an empty re-registered set the prune drops
        // every persisted legacy delegate recipe together with its
        // synthesized `ask_<name>` declaration, so an old snapshot restores
        // cleanly and its delegates silently disappear.
        let agent = builder
            .prune_unregistered_delegates()
            .approval(policy)
            .build()?;
        // The restored snapshot's model is the authoritative supervisor model
        // children inherit.
        spawn_ctx
            .shared
            .set_supervisor_model(agent.state().current_model().clone());

        Ok(Self {
            agent,
            tools,
            turn_complete,
            agent_name: binding.agent_name().to_owned(),
            system_prompt: binding.system_prompt().map(str::to_owned),
            spawn_ctx,
            cwd: config.cwd.clone(),
            pending_notifications: VecDeque::new(),
            run_counter: AtomicU64::new(1),
            tool_set_counter: AtomicU64::new(1),
        })
    }

    /// Drives one user message through the facade [`Agent`] under a
    /// [`CancelHandle`], emitting the run's streamed and terminal events.
    ///
    /// The caller (the session actor) mints the run identity and emits
    /// [`RunStarted`](Event::RunStarted) before invoking this; `run_turn` streams
    /// the turn: each text delta becomes an [`Event::TextDelta`] and the run ends
    /// with exactly one terminal event:
    ///
    /// - [`Event::RunFinished`] when the facade stream reaches its terminal
    ///   `Done`.
    /// - [`Event::RunError`] when the facade stream yields a failure, or ends
    ///   without a terminal `Done`. The [`RunErrorKind`] classifies the failure:
    ///   agent-lib's structured `LoopLimitExceeded` / `BudgetExhausted` variants
    ///   map onto their wire counterparts so transports can pick a precise stop
    ///   reason without string-matching.
    /// - [`Event::RunError`] with [`RunErrorKind::Cancelled`] when `cancel` fires
    ///   while the turn is in flight.
    ///
    /// Cancellation is cooperative through agent-lib's [`CancelHandle`]: the
    /// facade observes the token between fulfillment batches (bounded cancel
    /// latency), abandons the in-flight turn, and ends the stream with a
    /// cancellation error, which this driver reports as `"run cancelled"`.
    /// agent-lib leaves the cancelled machine parked on `Idle` with committed
    /// history intact, so the driver stays reusable for the next turn.
    ///
    /// A run that completes cleanly reaches a **committed consistency point**, the
    /// only point at which the agent can be snapshotted (`docs/DESIGN.md` §3.6).
    /// The committed [`AgentSnapshot`] is persisted to `store` *before*
    /// [`RunFinished`](Event::RunFinished) is emitted, so any observer that sees
    /// the terminal event can rely on the durable snapshot already being written.
    /// A cancelled or failed turn is not snapshotted: its committed history is
    /// unchanged from the prior committed point, whose snapshot (if any) is
    /// already durable.
    ///
    /// Pivot messages queued through `pivots` are drained after every stream
    /// poll — exactly when the run may be parked on a step boundary with the
    /// facade's pivot window open — and injected through
    /// [`AgentRunStream::interject`] (`docs/CLI.md` §3.2). An accepted pivot is
    /// announced as [`Event::PivotApplied`] and enters the conversation like a
    /// user message, so the committed snapshot persists it exactly as a
    /// `send_message` turn would. Pivots still queued when the run ends —
    /// finished, failed, or cancelled — are each announced as
    /// [`Event::PivotDropped`] with the run's terminal reason *before* the
    /// terminal event is emitted, and the outcome is returned so the session
    /// actor can drop any pivot that raced the run's end with the same reason.
    ///
    /// Agent-instance completion notifications share the drain but not the
    /// drop (M3-6, `docs/dyn-agents.md` §5.2): after every poll the driver
    /// also pulls the session registry's terminal notifications and injects
    /// each through [`AgentRunStream::interject_pivot`] as a host-sourced
    /// pivot (`PivotSource::Host { label: "agent:<id>" }`). A notification
    /// that never reaches an open pivot window before the run ends is *not*
    /// dropped — it stays buffered in the driver and is folded into the next
    /// turn's user input as a `[agent 实例通知]` prefix block, so a terminal
    /// instance is announced even when the run offered no window (a
    /// pure-text turn has none) or no run was in flight at all.
    ///
    /// Right after the terminal event is emitted — with the run's mutable
    /// stream borrow released and the facade agent at rest — the turn-complete
    /// hook fires exactly once (`docs/CLI.md` §4.5): every registered
    /// [`TurnCompleteListener`](crate::TurnCompleteListener) observes the
    /// session id and the run's completion kind, with listener failures
    /// isolated from the driver.
    pub(crate) async fn run_turn(
        &mut self,
        session_id: SessionId,
        text: String,
        events: &EventBus,
        cancel: &CancelHandle,
        pivots: &PivotQueue,
        store: &Persistence,
    ) -> TurnOutcome {
        // Cloned up front so the early-error path below can fire the
        // turn-complete hook without touching the mutably borrowed agent.
        let turn_complete = self.turn_complete.clone();
        self.queue_system_prompt_overlay(session_id);
        // Instance notifications left over from an idle gap (or unlanded
        // from the previous run) ride into the conversation as a prefix
        // block on this turn's user input (M3-6).
        let (text, drained_notifications) = self.take_pending_notifications(text);
        let mut stream = match self.agent.stream_with_cancel(text, cancel.clone()).await {
            Ok(stream) => stream,
            Err(error) => {
                // The turn never started, so the prefixed notifications were
                // not committed either: keep them for the next turn.
                self.pending_notifications.extend(drained_notifications);
                let outcome = if cancel.is_cancelled() {
                    TurnOutcome::Cancelled
                } else {
                    TurnOutcome::Failed {
                        kind: error_kind(&error),
                        message: error.to_string(),
                    }
                };
                // Same cancel cascade as the in-flight path below.
                if matches!(outcome, TurnOutcome::Cancelled) {
                    self.spawn_ctx.registry.cancel_all();
                }
                drop_pivots(session_id, pivots, events, outcome.pivot_drop_reason());
                let terminal = match &outcome {
                    TurnOutcome::Cancelled => Event::RunError {
                        id: session_id,
                        message: "run cancelled".to_owned(),
                        kind: RunErrorKind::Cancelled,
                    },
                    TurnOutcome::Failed { kind, message } => Event::RunError {
                        id: session_id,
                        message: message.clone(),
                        kind: *kind,
                    },
                    TurnOutcome::Completed => unreachable!("early stream error cannot complete"),
                };
                let _ = events.emit(terminal);
                turn_complete.notify(&TurnSummary::new(
                    session_id,
                    TurnCompletion::from(&outcome),
                ));
                return outcome;
            }
        };

        let mut final_output: Option<RunOutput> = None;
        let outcome = loop {
            match stream.next().await {
                Some(Ok(event)) => {
                    if let Some(mag_event) =
                        map_wire_event(session_id, event.to_wire(), &mut final_output)
                    {
                        let _ = events.emit(mag_event);
                    }
                    // After every poll the run may be parked on a step boundary
                    // with the facade's pivot window open: try landing the
                    // queued pivots and instance notifications before the next
                    // poll drives past it.
                    drain_pivots(
                        session_id,
                        &mut stream,
                        pivots,
                        &self.spawn_ctx.registry,
                        &mut self.pending_notifications,
                        events,
                    );
                }
                // A cancelled run surfaces from the facade as a stream error;
                // report it through the dedicated cancellation outcome rather
                // than leaking agent-lib's internal cursor detail.
                Some(Err(_)) if cancel.is_cancelled() => break TurnOutcome::Cancelled,
                Some(Err(error)) => {
                    break TurnOutcome::Failed {
                        kind: error_kind(&error),
                        message: error.to_string(),
                    };
                }
                None if cancel.is_cancelled() => break TurnOutcome::Cancelled,
                None => {
                    if final_output.is_some() {
                        break TurnOutcome::Completed;
                    }
                    break TurnOutcome::Failed {
                        kind: RunErrorKind::Other,
                        message: "agent stream ended without a terminal `Done` event".to_owned(),
                    };
                }
            }
        };

        // Release the mutable agent borrow before emitting the terminal event; a
        // cancelled or failed turn is abandoned (committed history is left
        // intact) so the next `run_turn` on this driver can proceed.
        drop(stream);

        // Cancel cascade (`docs/dyn-agents.md` §5.2): a cancelled supervisor
        // run cancels every still-running instance it spawned; a completed or
        // failed run leaves its instances alone (spawning is asynchronous, so
        // an instance may legitimately outlive the turn that started it).
        if matches!(outcome, TurnOutcome::Cancelled) {
            self.spawn_ctx.registry.cancel_all();
        }

        // Any pivot still queued at the end of the run never reached a step
        // boundary: report it dropped with the run's terminal reason.
        // Buffered instance notifications are deliberately *not* dropped —
        // they outlive the run in `pending_notifications` and are prefixed
        // onto the next turn's user input (M3-6).
        drop_pivots(session_id, pivots, events, outcome.pivot_drop_reason());

        let terminal = match &outcome {
            TurnOutcome::Completed => {
                let output = final_output.expect("completed turn has terminal output");
                // Persist the committed snapshot before announcing completion.
                self.persist_committed_snapshot(session_id, store);
                Event::RunFinished {
                    id: session_id,
                    output,
                }
            }
            TurnOutcome::Failed { kind, message } => Event::RunError {
                id: session_id,
                message: message.clone(),
                kind: *kind,
            },
            TurnOutcome::Cancelled => Event::RunError {
                id: session_id,
                message: "run cancelled".to_owned(),
                kind: RunErrorKind::Cancelled,
            },
        };
        let _ = events.emit(terminal);
        self.notify_turn_complete(session_id, TurnCompletion::from(&outcome));
        outcome
    }

    /// Emits the turn-complete hook for this run terminal (`docs/CLI.md` §4.5).
    ///
    /// Called after the terminal event went out and the run's mutable stream
    /// borrow was released: the facade agent is at rest (committed, failed, or
    /// cancelled), which is exactly the committed consistency point §4.5 hangs
    /// the notification on. Listener failures are isolated inside the hub.
    fn notify_turn_complete(&self, session_id: SessionId, completion: TurnCompletion) {
        self.turn_complete
            .notify(&TurnSummary::new(session_id, completion));
    }

    /// Drains the instance notifications no pivot channel could carry (M3-6)
    /// and folds them into `text` as a `[agent 实例通知]` prefix block.
    ///
    /// Returns the folded input together with the drained notifications so
    /// the early-error path of [`run_turn`](Self::run_turn) can restore them
    /// when the turn never started (an unstarted turn commits nothing, so an
    /// already-drained notification must not be lost with the folded text).
    fn take_pending_notifications(&mut self, text: String) -> (String, Vec<InstanceNotification>) {
        let mut drained: Vec<InstanceNotification> = self.pending_notifications.drain(..).collect();
        drained.extend(self.spawn_ctx.registry.drain_notifications());
        if drained.is_empty() {
            return (text, drained);
        }
        let mut prefixed = String::from(INSTANCE_NOTIFICATION_PREFIX_HEADER);
        for notification in &drained {
            prefixed.push_str("- ");
            prefixed.push_str(&notification.text);
            prefixed.push('\n');
        }
        prefixed.push('\n');
        prefixed.push_str(&text);
        (prefixed, drained)
    }

    /// Applies the runtime configuration snapshot to this session's agent at a
    /// turn boundary (`docs/CLI.md` §4.4, decision D2).
    ///
    /// The caller (the session actor) only invokes this while the agent is at
    /// rest — right after a run terminal, or immediately when the session has
    /// no in-progress run — satisfying agent-lib's reconfigure admission rule
    /// (Idle/between-runs only).
    ///
    /// Field mapping from the snapshot's **bound** `agents.<name>` entry
    /// (`self.agent_name`, resolved at session creation):
    ///
    /// - `model` → [`ReconfigRequest::SetModel`] (only when it actually
    ///   changed; `max_tokens`/`temperature` keep their current values since
    ///   the config schema carries no LLM sampling parameters yet). The spawn
    ///   context's shared supervisor model follows once the queued request
    ///   lands at the next turn start, so instances spawned from that turn on
    ///   inherit the new model.
    /// - `tools` → [`ReconfigRequest::ReplaceToolSet`] with the declarations
    ///   of the enabled entries, projected from this driver's executable
    ///   [`ToolRegistry`], plus the `agent` / `agent_result` / `agent_cancel`
    ///   instance-tool declarations (`docs/dyn-agents.md` §5.1: the instance
    ///   surface survives any surface narrowing, exactly as the delegation
    ///   declarations did on the retired static path). A config tool name
    ///   with no registered plugin is skipped with a warn log. An agent entry
    ///   with no tool list imposes no constraint and leaves the current
    ///   surface untouched; an explicit `tools = []` clears the plugin
    ///   surface while the instance tools remain. The bound entry's tool list
    ///   constrains only the supervisor's own surface — it never narrows a
    ///   spawned child's surface (`docs/dyn-agents.md` §7).
    /// - `system_prompt` → this driver's config-controlled overlay target. The
    ///   target is queued as [`ReconfigRequest::SetSystemPromptOverlay`] right
    ///   before the next turn starts instead of being stored in the agent's
    ///   immutable base prompt, so apply can replace or clear it without
    ///   appending to stale text.
    ///
    /// The definition table is rebuilt on every apply (M3-5): the TOML layer
    /// is re-projected from the applied snapshot and re-merged over the
    /// builtin/user/project layers (the two directory layers are re-read, so
    /// edited definition files are picked up too). The session's default
    /// subagent toolset (`[session].default_subagent_tools`,
    /// `docs/dyn-agents.md` §7) is re-read alongside it. Definitions and the
    /// default toolset only affect *later* spawns; running instances keep
    /// driving with the context they captured.
    ///
    /// Out of scope on the current agent-lib reconfigure surface (documented
    /// for M3-R): the approval policy is baked into the agent at build time
    /// and has no reconfigure variant, so `tools.*.approval` / `approval.*`
    /// changes take effect on the next session (re)build rather than
    /// mid-session; per-run `budget` is likewise build-time only (and
    /// `session` defaults only affect new sessions per decision D2).
    pub(crate) fn apply_config(&mut self, session_id: SessionId, snapshot: &ConfigSnapshot) {
        if let Some(agent_config) = snapshot.agent(&self.agent_name) {
            self.system_prompt = agent_config.system_prompt().map(str::to_owned);
        }
        let requests = self.reconfig_requests(session_id, snapshot);
        // Mirror the queued `SetModel` (if any) into the shared spawn state:
        // the facade drains its reconfig queue at the next turn boundary, and
        // the model this mirrors is built from the same values as the request,
        // so instances spawned from the next run on inherit exactly the
        // supervisor's new effective model. (Facade admission of `SetModel`
        // only rejects blank models / non-finite temperatures / provider-extras
        // mismatches, none of which this driver can produce.)
        if let Some(model) = requests.iter().find_map(|request| match request {
            ReconfigRequest::SetModel { model } => Some(model.clone()),
            _ => None,
        }) {
            self.spawn_ctx.shared.set_supervisor_model(model);
        }
        // Re-read the session's default subagent toolset from the applied
        // snapshot (same rebuild semantics as the definition table below):
        // instances spawned after the apply whose definition sets no `tools`
        // fall back to the new list (`docs/dyn-agents.md` §7).
        self.spawn_ctx.shared.set_default_tools(
            snapshot
                .session_defaults()
                .default_subagent_tools()
                .map(<[String]>::to_vec),
        );
        self.apply_reconfig_items(session_id, requests);
        // Rebuild the definition table in the shared spawn state
        // (`docs/dyn-agents.md` §3.2, M3-5): the TOML layer is re-projected
        // from the applied snapshot and re-merged, so the next spawn at any
        // depth sees the new table. (The supervisor model half follows at the
        // next turn start, when an applied `SetModel` actually lands.)
        self.spawn_ctx
            .shared
            .set_definitions(assemble_agent_definitions(
                self.cwd.as_deref(),
                AgentDefinitionRegistry::from_toml_snapshot(snapshot, &self.agent_name),
            ));
    }

    /// Applies each reconfigure request independently.
    ///
    /// Per-item failure isolation (`docs/CLI.md` §4.4, TODO M3-5): a rejected
    /// item — an immutable variant such as a skill request (facade reports
    /// [`FacadeError::Config`]), or a payload failing admission — is logged at
    /// warn level and skipped, keeping the agent's previous value; the
    /// remaining items are still applied and the session's main flow is never
    /// interrupted.
    pub(crate) fn apply_reconfig_items(
        &mut self,
        session_id: SessionId,
        requests: Vec<ReconfigRequest>,
    ) {
        for request in requests {
            let family = reconfig_family(&request);
            if let Err(error) = self.agent.reconfigure(request) {
                tracing::warn!(
                    session_id = %session_id,
                    request = family,
                    error = %error,
                    "config apply: reconfigure item rejected; keeping the previous value"
                );
            }
        }
    }

    /// Queues the config-controlled system prompt overlay for the next turn
    /// start when it differs from the agent's current overlay.
    fn queue_system_prompt_overlay(&mut self, session_id: SessionId) {
        if self.agent.state().system_prompt_overlay() == self.system_prompt.as_deref() {
            return;
        }
        if let Err(error) = self
            .agent
            .reconfigure(ReconfigRequest::SetSystemPromptOverlay {
                system_prompt: self.system_prompt.clone(),
                expected_version: self.agent.state().system_prompt_overlay_version(),
            })
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "config apply: system prompt overlay rejected; keeping the previous value"
            );
        }
    }

    /// Builds the reconfigure batch projecting the snapshot's entry for this
    /// session's bound agent (`self.agent_name`) onto this session's agent.
    fn reconfig_requests(
        &mut self,
        session_id: SessionId,
        snapshot: &ConfigSnapshot,
    ) -> Vec<ReconfigRequest> {
        let Some(agent_config) = snapshot.agent(&self.agent_name) else {
            return Vec::new();
        };
        let mut requests = Vec::new();

        if let Some(model) = agent_config.model() {
            let current = self.agent.state().current_model();
            if current.model() != model {
                requests.push(ReconfigRequest::SetModel {
                    model: ModelRef::new(
                        model.to_owned(),
                        current.max_tokens(),
                        current.temperature(),
                        None,
                    ),
                });
            }
        }

        let wanted: Option<Vec<&str>> = agent_config.tools_list().map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.is_enabled())
                .map(|tool| tool.name())
                .collect()
        });
        if let Some(wanted) = wanted {
            let mut declarations = Vec::new();
            for name in wanted {
                match self
                    .tools
                    .plugins()
                    .iter()
                    .find(|plugin| plugin.name() == name)
                {
                    Some(plugin) => declarations.push(plugin.declaration()),
                    None => tracing::warn!(
                        session_id = %session_id,
                        tool = name,
                        "config apply: tool not present in the tool registry; skipped"
                    ),
                }
            }
            // The instance tools are facade-level (no registry plugin backs
            // them), so the projection names them explicitly: a narrowed
            // `tools` list must never strip the session's instance surface
            // (`docs/dyn-agents.md` §5.1), and rebuilding the trio here also
            // refreshes the `agent` description with the definition table
            // this apply just swapped in.
            declarations.extend(agent_tools(&self.spawn_ctx).iter().map(Tool::declaration));
            let current_names: BTreeSet<&str> = self
                .agent
                .state()
                .current_tool_set()
                .tools()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            let wanted_names: BTreeSet<&str> =
                declarations.iter().map(|tool| tool.name.as_str()).collect();
            if current_names != wanted_names {
                let id = self.next_tool_set_id();
                requests.push(ReconfigRequest::ReplaceToolSet {
                    tool_set: ToolSetRef::new(id, declarations),
                });
            }
        }

        requests
    }

    /// Mints a fresh identity for a replacement tool set.
    fn next_tool_set_id(&self) -> ToolSetId {
        let value = self.tool_set_counter.fetch_add(1, Ordering::Relaxed);
        ToolSetId::new(Uuid::from_u128(u128::from(value)))
    }

    /// Captures the agent's committed [`AgentSnapshot`] and writes it to `store`.
    ///
    /// Persistence is best-effort: a snapshot or store failure is swallowed so it
    /// never turns an otherwise successful run into an error. At a committed point
    /// [`Agent::snapshot`](agent_lib::facade::Agent::snapshot) is expected to
    /// succeed; if it cannot, the previous committed snapshot (if any) stays the
    /// latest durable state.
    fn persist_committed_snapshot(&self, session_id: SessionId, store: &Persistence) {
        if let Ok(snapshot) = self.agent.snapshot() {
            let _ = store.save_snapshot(session_id, &snapshot);
        }
    }

    /// Mints the next envelope run identity for this session.
    ///
    /// The facade owns the drive's internal ids and does not surface a run id on
    /// the event stream, so mag mints its own monotonic id purely to tag the
    /// [`RunStarted`](Event::RunStarted) / cancellation envelope.
    pub(crate) fn next_run_id(&self) -> WireRunId {
        let value = self.run_counter.fetch_add(1, Ordering::Relaxed);
        WireRunId::new(Uuid::from_u128(u128::from(value)))
    }
}

impl Drop for SessionDriver {
    /// Session end cancel cascade (`docs/dyn-agents.md` §5.2): instances are
    /// ephemeral and never outlive their session, so every still-running
    /// instance is cancelled when the driver drops. Already-terminal
    /// instances keep their outcomes.
    fn drop(&mut self) {
        self.spawn_ctx.registry.cancel_all();
    }
}

/// Terminal outcome of one [`SessionDriver::run_turn`] drive loop.
pub(crate) enum TurnOutcome {
    /// The facade stream reached its terminal `Done`.
    Completed,
    /// The facade stream yielded a failure carrying this message.
    Failed {
        /// Wire classification mapped from the [`FacadeError`] variant.
        kind: RunErrorKind,
        /// Human-readable failure message.
        message: String,
    },
    /// The run's [`CancelHandle`] fired while the turn was in flight.
    Cancelled,
}

impl TurnOutcome {
    /// Reason string stamped on [`Event::PivotDropped`] when a queued pivot
    /// outlives its run (`docs/CLI.md` §3.2).
    pub(crate) fn pivot_drop_reason(&self) -> &'static str {
        match self {
            Self::Completed => PIVOT_DROP_RUN_FINISHED,
            Self::Failed { .. } => PIVOT_DROP_RUN_FAILED,
            Self::Cancelled => PIVOT_DROP_RUN_CANCELLED,
        }
    }
}

impl From<&TurnOutcome> for TurnCompletion {
    /// Maps the driver's terminal outcome onto the turn-complete hook's
    /// completion kind (`docs/CLI.md` §4.5).
    fn from(outcome: &TurnOutcome) -> Self {
        match outcome {
            TurnOutcome::Completed => Self::Committed,
            TurnOutcome::Failed { .. } => Self::Failed,
            TurnOutcome::Cancelled => Self::Cancelled,
        }
    }
}

/// Short static label identifying a reconfigure request variant, stamped on
/// warn logs when an item is rejected at admission.
fn reconfig_family(request: &ReconfigRequest) -> &'static str {
    match request {
        ReconfigRequest::ActivateSkill { .. } => "activate_skill",
        ReconfigRequest::DeactivateSkill { .. } => "deactivate_skill",
        ReconfigRequest::ReplaceActiveSkills { .. } => "replace_active_skills",
        ReconfigRequest::SetSystemPromptOverlay { .. } => "set_system_prompt_overlay",
        ReconfigRequest::ReplaceToolSet { .. } => "replace_tool_set",
        ReconfigRequest::PatchToolSet { .. } => "patch_tool_set",
        ReconfigRequest::SetModel { .. } => "set_model",
        ReconfigRequest::SetLoopPolicy { .. } => "set_loop_policy",
    }
}

/// [`Event::PivotDropped`] reason for a run that finished normally.
const PIVOT_DROP_RUN_FINISHED: &str = "run finished before the pivot could be applied";
/// [`Event::PivotDropped`] reason for a run that failed.
const PIVOT_DROP_RUN_FAILED: &str = "run failed before the pivot could be applied";
/// [`Event::PivotDropped`] reason for a run that was cancelled.
const PIVOT_DROP_RUN_CANCELLED: &str = "run cancelled before the pivot could be applied";

/// Header of the block [`SessionDriver::take_pending_notifications`] prefixes
/// onto the next turn's user input, carrying the instance notifications no
/// pivot channel could deliver (M3-6).
const INSTANCE_NOTIFICATION_PREFIX_HEADER: &str = "[agent 实例通知]\n";

/// Drains the run's pivot queue and the session's instance-notification
/// queue, attempting [`AgentRunStream::interject`] on each queued user pivot
/// and [`AgentRunStream::interject_pivot`] on each instance notification
/// (`docs/CLI.md` §3.2, `docs/dyn-agents.md` §5.2).
///
/// Called after every stream poll, which is exactly when the run may be
/// parked on a step boundary with the facade's pivot window open. A pivot
/// accepted by the facade is announced as [`Event::PivotApplied`].
/// [`FacadeError::InvalidState`] means the window is not (or no longer) open —
/// a side-effect-free, retry-safe rejection — so the item returns to the
/// front of its queue and is retried after the next poll. Any other error is
/// permanent: the item leaves the queue as [`Event::PivotDropped`].
///
/// Instance notifications still buffered when the run ends are *not*
/// dropped, unlike user pivots: they stay in `pending_notifications` and are
/// prefixed onto the next turn's user input (M3-6).
fn drain_pivots(
    session_id: SessionId,
    stream: &mut AgentRunStream<'_>,
    pivots: &PivotQueue,
    registry: &AgentInstanceRegistry,
    pending_notifications: &mut VecDeque<InstanceNotification>,
    events: &EventBus,
) {
    // Pull freshly terminal instances into the retry buffer first; the
    // registry queue is the only producer, and the buffer keeps FIFO order
    // across polls.
    pending_notifications.extend(registry.drain_notifications());
    while let Some(text) = pivots.pop_front() {
        match stream.interject(text.as_str()) {
            Ok(()) => {
                let _ = events.emit(Event::PivotApplied { id: session_id });
            }
            Err(FacadeError::InvalidState(_)) => {
                pivots.push_front(text);
                break;
            }
            Err(error) => {
                let _ = events.emit(Event::PivotDropped {
                    id: session_id,
                    reason: format!("pivot rejected: {error}"),
                });
            }
        }
    }
    while let Some(notification) = pending_notifications.pop_front() {
        match stream.interject_pivot(instance_notification_pivot(&notification)) {
            Ok(()) => {
                let _ = events.emit(Event::PivotApplied { id: session_id });
            }
            Err(FacadeError::InvalidState(_)) => {
                pending_notifications.push_front(notification);
                break;
            }
            Err(error) => {
                let _ = events.emit(Event::PivotDropped {
                    id: session_id,
                    reason: format!("instance notification pivot rejected: {error}"),
                });
            }
        }
    }
}

/// Builds the host-sourced pivot message carrying one instance completion
/// notification into the supervisor's conversation (M3-6): the notification
/// enters history as a user message attributed to
/// `PivotSource::Host { label: "agent:<id>" }`.
fn instance_notification_pivot(notification: &InstanceNotification) -> PivotMessage {
    PivotMessage::new(
        host_pivot_message_id(),
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: notification.text.clone(),
                extra: Default::default(),
            }],
        },
        PivotSource::Host {
            label: format!("agent:{}", notification.id),
        },
    )
    .expect("a notification pivot is a user-role message")
}

/// Mints the conversation message id for a host-sourced notification pivot.
///
/// The facade's own id source is internal and unreachable from mag, so the
/// id is externally allocated — exactly the use case [`PivotMessage`] exposes
/// for host pivots. The high half carries a process-unique tag (nanoseconds
/// since the epoch), placing the id outside the facade's small-counter space
/// exactly like a UUIDv7: it can never collide with a facade-minted id, and a
/// restored agent's `continuing_after` re-seed scan ignores it (agent-lib
/// `facade/ids.rs`). The low half is a process-wide counter for uniqueness
/// within the process.
fn host_pivot_message_id() -> MessageId {
    /// Nanoseconds-since-epoch tag keeping ids unique across process runs.
    static PROCESS_TAG: OnceLock<u64> = OnceLock::new();
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let tag = PROCESS_TAG.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(1)
    });
    let ordinal = COUNTER.fetch_add(1, Ordering::Relaxed);
    MessageId::new(Uuid::from_u128(
        (u128::from(*tag) << 64) | u128::from(ordinal),
    ))
}

/// Reports every pivot still queued for a finished run as
/// [`Event::PivotDropped`] with `reason`, preserving queue order.
fn drop_pivots(
    session_id: SessionId,
    pivots: &PivotQueue,
    events: &EventBus,
    reason: &'static str,
) {
    for _ in pivots.drain() {
        let _ = events.emit(Event::PivotDropped {
            id: session_id,
            reason: reason.to_owned(),
        });
    }
}

/// Maps a [`FacadeError`] onto its wire [`RunErrorKind`] classification.
///
/// Only the structured terminal variants a transport can act on get a dedicated
/// kind; everything else stays [`RunErrorKind::Other`] with the message
/// carrying the details.
fn error_kind(error: &FacadeError) -> RunErrorKind {
    match error {
        FacadeError::LoopLimitExceeded => RunErrorKind::LoopLimitExceeded,
        FacadeError::BudgetExhausted => RunErrorKind::BudgetExhausted,
        _ => RunErrorKind::Other,
    }
}

/// Projects a wire [`SessionBudget`] onto agent-lib's [`BudgetLimits`].
fn budget_limits(budget: &SessionBudget) -> BudgetLimits {
    BudgetLimits::new(
        budget.max_steps,
        budget.max_tokens,
        budget.max_cost_micros,
        budget.max_wall_time_secs.map(Duration::from_secs),
    )
}

/// Assembles the session's root (depth `0`) [`InstanceSpawnContext`]
/// (`docs/dyn-agents.md` §5, M3-5): a fresh instance registry, the four-layer
/// definition table merged from the binding's TOML layer, the session's
/// default subagent toolset (§7), and the supervisor's own client / tools /
/// approval / event handles the spawned instances share. `supervisor_model`
/// seeds the shared cell and is corrected to the built agent's authoritative
/// model right after construction.
#[allow(clippy::too_many_arguments)]
fn root_spawn_context(
    config: &SessionConfig,
    client: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
    approval: Arc<IpcApproval>,
    binding: &SessionBinding,
    overrides: &ApprovalOverrides,
    session_id: SessionId,
    events: EventBus,
    supervisor_model: ModelRef,
) -> InstanceSpawnContext {
    InstanceSpawnContext {
        registry: AgentInstanceRegistry::new(),
        shared: SharedSpawnState::new(
            assemble_agent_definitions(config.cwd.as_deref(), binding.agent_definitions().clone()),
            supervisor_model,
            binding.default_subagent_tools().map(<[String]>::to_vec),
        ),
        client,
        tools,
        overrides: overrides.clone(),
        interaction: approval,
        events,
        session_id,
        worktree: config.cwd.as_ref().map_or_else(
            || WorktreeRef::new("."),
            |cwd| WorktreeRef::new(cwd.clone()),
        ),
        depth: 0,
    }
}

/// Merges the session's agent-definition table from its four sources
/// (`docs/dyn-agents.md` §3.2): builtin, the user-level directory
/// ([`default_user_agents_dir`]), the project-level directory
/// ([`project_agents_dir`], only when the session has a cwd), and the TOML
/// layer `toml` (resolved by the session binding at spawn, rebuilt from the
/// applied snapshot at config-apply time). Directory loads are tolerant: a
/// missing directory is empty and an unreadable one is warned about and
/// skipped, so a broken setup never takes a session down.
fn assemble_agent_definitions(
    cwd: Option<&Path>,
    toml: AgentDefinitionRegistry,
) -> AgentDefinitionRegistry {
    let user = AgentDefinitionRegistry::load_user_dir(&default_user_agents_dir()).unwrap_or_else(
        |error| {
            tracing::warn!(%error, "user agent definition directory unreadable; layer skipped");
            AgentDefinitionRegistry::default()
        },
    );
    let project = cwd.map_or_else(AgentDefinitionRegistry::default, |cwd| {
        AgentDefinitionRegistry::load_project_dir(&project_agents_dir(cwd)).unwrap_or_else(
            |error| {
                tracing::warn!(%error, "project agent definition directory unreadable; layer skipped");
                AgentDefinitionRegistry::default()
            },
        )
    });
    AgentDefinitionRegistry::merge(
        AgentDefinitionRegistry::merge(
            AgentDefinitionRegistry::merge(AgentDefinitionRegistry::builtin(), user),
            project,
        ),
        toml,
    )
}

/// Maps one projected [`WireRunEvent`] into a mag [`Event`].
///
/// A terminal [`WireRunEvent::Done`] is folded into `final_output` and produces
/// no streamed event (the caller emits [`RunFinished`](Event::RunFinished) once
/// the stream drains). Tool lifecycle events project into mag's
/// [`ToolStarted`](Event::ToolStarted) / [`ToolFinished`](Event::ToolFinished).
///
/// Delegation lifecycle events (`docs/CLI.md` §5 P7) project into the matching
/// [`DelegationStarted`](Event::DelegationStarted) /
/// [`DelegationFinished`](Event::DelegationFinished) /
/// [`DelegationFailed`](Event::DelegationFailed) wire events through
/// [`delegation_trace_from_wire`], and a delegate's intermediate message into
/// [`DelegationMessage`](Event::DelegationMessage). A delegation call is
/// bracketed by these events only — the facade deliberately emits no
/// `ToolStarted`/`ToolFinished` pair for an `ask_<name>` call.
///
/// The facade's [`ApprovalRequested`](WireRunEvent::ApprovalRequested) is
/// intentionally dropped: it is a fire-and-forget notification, and mag's
/// canonical pause event is the [`InteractionRequested`](Event::InteractionRequested)
/// that [`IpcApproval`] emits independently on the pause point (`docs/DESIGN.md`
/// §3.4). `DelegationProgress` has no wire counterpart, and escalation,
/// artifact, and raw variants are not produced on the local-subagent path, so
/// they are ignored here.
fn map_wire_event(
    session_id: SessionId,
    event: WireRunEvent,
    final_output: &mut Option<RunOutput>,
) -> Option<Event> {
    match event {
        WireRunEvent::TextDelta(text) => Some(Event::TextDelta {
            id: session_id,
            text,
        }),
        WireRunEvent::ToolStarted(trace) => Some(Event::ToolStarted {
            id: session_id,
            trace: tool_trace_from_wire(&trace, ToolStatusWire::Started),
        }),
        WireRunEvent::ToolFinished(trace) => Some(Event::ToolFinished {
            id: session_id,
            trace: tool_trace_from_wire(&trace, ToolStatusWire::Finished),
        }),
        WireRunEvent::DelegationStarted(trace) => Some(Event::DelegationStarted {
            id: session_id,
            trace: delegation_trace_from_wire(&trace, DelegationStatusWire::Started),
        }),
        WireRunEvent::DelegationFinished(trace) => Some(Event::DelegationFinished {
            id: session_id,
            trace: delegation_trace_from_wire(&trace, DelegationStatusWire::Finished),
        }),
        WireRunEvent::DelegationFailed(trace) => Some(Event::DelegationFailed {
            id: session_id,
            trace: delegation_trace_from_wire(&trace, DelegationStatusWire::Failed),
        }),
        WireRunEvent::DelegationMessage(message) => Some(Event::DelegationMessage {
            id: session_id,
            message: delegation_message_from_wire(&message),
        }),
        WireRunEvent::Done(output) => {
            *final_output = Some(run_output_from_wire(&output));
            None
        }
        // `ApprovalRequested` is covered by `IpcApproval`'s `InteractionRequested`
        // (see the function docs); every remaining variant is out of scope here.
        _ => None,
    }
}

/// Projects a [`ToolRegistry`] into the facade tool surface and its approval
/// policy shared by [`SessionDriver::new`] and [`SessionDriver::restore`].
///
/// Assembly order:
///
/// 1. The plugins are projected through [`project_tool_plugins`] (shared with
///    the instance spawn path, `docs/dyn-agents.md` §7): the whole-agent
///    default tier comes from `overrides` (the `[approval]` section; `allow`
///    matches agent-lib's own default), the bound `agents.<name>` entry's
///    enabled tool list narrows the projection when set, and each projected
///    plugin declaring a [`permission`](ToolPlugin::permission) is gated behind
///    [`ApprovalPolicy::ask_tool`] so it pauses through the injected
///    [`IpcApproval`](crate::engine::approval::IpcApproval), while a
///    permission-free plugin stays on the policy default tier (`docs/DESIGN.md`
///    §3.2/§3.3). This includes read-only tools and `ask_user`, whose own
///    interaction is emitted by its handler rather than by the approval policy.
///    Every projected plugin receives the session's user-interaction bridge;
///    `ask_user` consumes it to emit `Question` / `Choice` through the same
///    `IpcApproval` path (`docs/CLI.md` §5 P6 / D6), while other tools ignore
///    it.
/// 2. The `agent` / `agent_result` / `agent_cancel` instance tools
///    (`docs/dyn-agents.md` §5.1, M3-5) are appended over the session's root
///    spawn context. They stay on the policy default tier: spawning is
///    asynchronous and immediately reversible through `agent_cancel`, so no
///    derived gate is added (design §7).
/// 3. `overrides`' per-tool tiers (`[tools.<name>].approval`) replace the
///    derived tier for their tool — including `[tools.agent]` /
///    `[tools.agent_result]` / `[tools.agent_cancel]` entries, so a
///    configured `deny` refuses spawns outright and an `ask` gates them
///    through the root session's `IpcApproval` (M3-5).
///
/// Tier mapping: `ask` → [`ApprovalPolicy::ask_tool`], `allow` →
/// [`ApprovalPolicy::allow_tool`], `deny` → [`ApprovalPolicy::deny_tool`].
/// Note that with mag's always-injected interaction handler, a `deny` tier
/// still pauses through the interface (agent-lib resolves the pause through
/// the injected handler rather than the policy's headless fallback) — the tier
/// records the *policy intent*; the interface remains the deciding authority.
fn tool_surface(
    tools: &ToolRegistry,
    binding: &SessionBinding,
    overrides: &ApprovalOverrides,
    user_interaction: Arc<dyn UserInteractionBridge>,
    spawn_ctx: &Arc<InstanceSpawnContext>,
) -> (Vec<Tool>, ApprovalPolicy) {
    let (mut facade_tools, policy) = project_tool_plugins(
        tools,
        binding.tools(),
        overrides.default_tier(),
        user_interaction,
    );
    facade_tools.extend(agent_tools(spawn_ctx));
    let policy = apply_per_tool_tiers(policy, overrides);
    (facade_tools, policy)
}

/// Projects registry plugins into facade tools and their base approval policy,
/// shared by the supervisor's [`tool_surface`] and the dynamic-instance spawn
/// path (`docs/dyn-agents.md` §7, TODO M3-3).
///
/// - `allowed` narrows the projection to the named plugins (the supervisor's
///   bound `agents.<name>` tool list, or an agent definition's `tools`
///   allowlist); `None` projects every registered plugin. A named tool with no
///   registered plugin is warned about and skipped.
/// - The policy starts from the `default_tier` whole-agent tier and gates each
///   projected plugin declaring a [`permission`](ToolPlugin::permission)
///   behind `ask`.
///
/// Per-tool `[tools.<name>]` tiers are deliberately **not** applied here:
/// callers layer them on after any further derived tiers (the supervisor
/// appends the instance tools first so an explicit `[tools.agent]`-style
/// entry keeps the final say; the instance path applies them directly).
pub(crate) fn project_tool_plugins(
    tools: &ToolRegistry,
    allowed: Option<&[String]>,
    default_tier: ApprovalPolicyKind,
    user_interaction: Arc<dyn UserInteractionBridge>,
) -> (Vec<Tool>, ApprovalPolicy) {
    let mut policy = base_policy(default_tier);
    let mut facade_tools = Vec::new();
    for plugin in tools.plugins() {
        if let Some(allowed) = allowed
            && !allowed.iter().any(|name| name == plugin.name())
        {
            continue;
        }
        if plugin.permission().is_some() {
            policy = policy.ask_tool(plugin.name());
        }
        facade_tools.push(facade_tool(
            Arc::clone(plugin),
            Arc::clone(&user_interaction),
        ));
    }
    if let Some(allowed) = allowed {
        for name in allowed {
            if !tools.plugins().iter().any(|p| p.name() == name) {
                tracing::warn!(
                    tool = name.as_str(),
                    "tool surface names a tool not present in the tool registry; skipped"
                );
            }
        }
    }
    (facade_tools, policy)
}

/// Applies the configured `[tools.<name>].approval` tiers on top of `policy`
/// (shared by the main agent's [`tool_surface`] and the dynamic-instance
/// spawn path of `docs/dyn-agents.md` §7).
pub(crate) fn apply_per_tool_tiers(
    mut policy: ApprovalPolicy,
    overrides: &ApprovalOverrides,
) -> ApprovalPolicy {
    for (name, tier) in overrides.per_tool() {
        policy = match tier {
            ApprovalPolicyKind::Ask => policy.ask_tool(name.clone()),
            ApprovalPolicyKind::Allow => policy.allow_tool(name.clone()),
            ApprovalPolicyKind::Deny => policy.deny_tool(name.clone()),
        };
    }
    policy
}

/// Builds the whole-agent default [`ApprovalPolicy`] for a configured tier.
///
/// The `ask` tier carries a deny-by-default synchronous handler: with mag's
/// injected [`IpcApproval`](crate::engine::approval::IpcApproval) every pause
/// is answered by the interface, so the handler only matters as agent-lib's
/// headless fallback, where denying is the conservative choice.
fn base_policy(tier: ApprovalPolicyKind) -> ApprovalPolicy {
    match tier {
        ApprovalPolicyKind::Allow => ApprovalPolicy::default(),
        ApprovalPolicyKind::Deny => ApprovalPolicy::new(Approval::auto_deny()),
        ApprovalPolicyKind::Ask => ApprovalPolicy::new(Approval::ask(|_| ApprovalDecision::Deny)),
    }
}

/// User-interaction bridge consumed by the `ask_user` tool (`docs/CLI.md` §5 P6).
///
/// The bridge deliberately reuses the session's interaction-answer path: the
/// supervisor's bridge wraps its [`IpcApproval`] instance (the same pending
/// map, `RequestId` minting, origin handling, and `respond_interaction`
/// wake-up path answer approvals, permissions, questions, and choices), while
/// a spawned agent instance's bridge wraps its origin router so the child's
/// `ask_user` bubbles to the root session with the instance's attribution
/// (`docs/dyn-agents.md` §4, M3-3). A tool call supplies only agent-lib's
/// [`ToolContext`], so this bridge reconstructs the minimal [`RunContext`]
/// needed by the [`InteractionHandler`] using the tool call's run id,
/// cancellation token, and a trace root derived from the tool call id.
pub(crate) struct IpcUserInteractionBridge {
    handler: Arc<dyn InteractionHandler>,
}

impl IpcUserInteractionBridge {
    /// Creates a bridge over the handler that answers the interaction.
    pub(crate) fn new(handler: Arc<dyn InteractionHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait::async_trait]
impl UserInteractionBridge for IpcUserInteractionBridge {
    async fn ask_user(
        &self,
        ctx: ToolContext,
        request: UserInteractionRequest,
    ) -> Result<UserInteractionResponse, UserInteractionError> {
        let interaction = match &request.options {
            Some(options) => Interaction::choice(
                StepId::new(*ctx.tool_call_id.as_uuid()),
                request.question.clone(),
                options.clone(),
            ),
            None => Interaction::question(
                StepId::new(*ctx.tool_call_id.as_uuid()),
                request.question.clone(),
            ),
        };
        let run_context = RunContext::new_root_with_cancellation(
            ctx.run_id,
            BudgetLimits::unbounded(),
            TraceNodeId::new(format!("ask_user:{}", ctx.tool_call_id)),
            ctx.cancel,
        );

        match self.handler.fulfill(&interaction, &run_context).await {
            RequirementResult::Interaction(InteractionResponse::Answer(text)) => {
                if request.options.is_some() {
                    Err(UserInteractionError::new(
                        "answer response received for a choice prompt",
                    ))
                } else {
                    Ok(UserInteractionResponse::Answer(text))
                }
            }
            RequirementResult::Interaction(InteractionResponse::Choice(index)) => {
                if request.options.is_some() {
                    Ok(UserInteractionResponse::Choice(index))
                } else {
                    Err(UserInteractionError::new(
                        "choice response received for a free-form question",
                    ))
                }
            }
            RequirementResult::Interaction(other) => Err(UserInteractionError::new(format!(
                "unexpected interaction response family `{}`",
                other.tag()
            ))),
            _ => Err(UserInteractionError::new(
                "unexpected requirement result while asking user",
            )),
        }
    }
}

/// Projects one facade [`Tool`] from a [`ToolPlugin`].
///
/// The plugin's [`declaration`](ToolPlugin::declaration) supplies the model-facing
/// name, description, and JSON input schema; the executor forwards the run-scoped
/// [`ToolContext`] and raw JSON arguments to
/// [`ToolPlugin::invoke_with_context`](ToolPlugin::invoke_with_context), adding
/// the current session's user-interaction bridge for `ask_user` (`docs/CLI.md`
/// §5 P6). The plugin already encodes success and recoverable failure in its
/// [`ToolResult`] status, so the executor is infallible from the facade's point
/// of view.
fn facade_tool(
    plugin: Arc<dyn ToolPlugin>,
    user_interaction: Arc<dyn UserInteractionBridge>,
) -> Tool {
    let declaration = plugin.declaration();
    Tool::function_with_schema(
        declaration.name,
        declaration.description,
        declaration.input_schema,
        move |ctx: ToolContext, args: Value| {
            let plugin = Arc::clone(&plugin);
            let user_interaction = Arc::clone(&user_interaction);
            async move {
                let invocation = ToolInvocation::new(ctx).with_user_interaction(user_interaction);
                Ok::<ToolResult, Infallible>(plugin.invoke_with_context(invocation, args).await)
            }
        },
    )
}

/// Projects a facade [`ToolTrace`](FacadeToolTrace) into the wire
/// [`ToolTrace`], stamping the lifecycle `status`.
///
/// The facade trace only carries the tool name and stringified framework call
/// id; the richer input/output/message fields are populated by later milestones.
fn tool_trace_from_wire(trace: &FacadeToolTrace, status: ToolStatusWire) -> ToolTrace {
    ToolTrace {
        run_id: None,
        call_id: ToolCallIdWire::parse_str(&trace.call_id)
            .unwrap_or_else(|_| ToolCallIdWire::new(Uuid::nil())),
        name: trace.name.clone(),
        input: None,
        output: None,
        status,
        message: None,
    }
}

/// Projects a facade delegation trace into the wire [`DelegationTrace`]
/// (`docs/CLI.md` §5 P7).
///
/// agent-lib's facade trace currently carries only the delegate name, the
/// terminal status, and the child's token usage — the delegated task input and
/// the output/failure reason are not exposed on the facade event surface, so
/// the wire's optional `task`/`output`/`message` fields stay `None` here
/// (they remain part of the frozen contract for producers that can populate
/// them, e.g. the M4-2 external path if agent-lib grows the surface; recorded
/// in the M4-1 completion notes). `run_id` is `None` for the same reason as
/// in [`tool_trace_from_wire`]: the facade does not surface a run id on its
/// event stream.
fn delegation_trace_from_wire(
    trace: &FacadeDelegationTrace,
    status: DelegationStatusWire,
) -> DelegationTrace {
    DelegationTrace {
        run_id: None,
        delegate: trace.delegate.clone(),
        status,
        task: None,
        output: None,
        message: None,
        usage: Some(usage_info_from_usage(&trace.usage)),
    }
}

/// Projects a facade delegation message into the wire
/// [`DelegationMessageWire`].
fn delegation_message_from_wire(message: &FacadeDelegationMessage) -> DelegationMessageWire {
    DelegationMessageWire {
        run_id: None,
        delegate: message.delegate.clone(),
        text: message.message.clone(),
    }
}

/// Projects the facade's terminal [`WireRunOutput`] into a mag [`RunOutput`].
fn run_output_from_wire(output: &WireRunOutput) -> RunOutput {
    RunOutput {
        text: output.reply.text().to_owned(),
        usage: Some(usage_from_summary(&output.usage)),
    }
}

/// Flattens a facade [`UsageSummary`] into the provider-neutral [`UsageInfo`].
fn usage_from_summary(summary: &UsageSummary) -> UsageInfo {
    let usage = summary.total();
    usage_info_from_usage(&usage)
}

/// Flattens agent-lib usage into the service-level usage DTO.
fn usage_info_from_usage(usage: &agent_lib::model::usage::Usage) -> UsageInfo {
    UsageInfo {
        input_tokens: u64::from(usage.input),
        output_tokens: u64::from(usage.output),
        total_tokens: u64::from(usage.total.unwrap_or_else(|| usage.total_computed())),
    }
}

#[cfg(test)]
mod tests {
    use agent_lib::facade::{
        ApprovalRequest, DelegationMessage as FacadeDelegationMessage,
        DelegationTrace as FacadeDelegationTrace, ToolTrace as FacadeToolTrace,
    };
    use agent_lib::{
        client::Response,
        facade::{RunEvent, RunOutput as FacadeRunOutput, WireRunEvent},
        model::{
            content::ContentBlock,
            message::{Message, Role},
            normalized::{Normalized, StopReason},
            usage::Usage,
        },
    };
    use mag_service::{
        DelegationMessageWire, DelegationStatusWire, DelegationTrace, Event, SessionId,
        ToolCallIdWire, ToolStatusWire, ToolTrace, UsageInfo,
    };
    use serde_json::Map;
    use uuid::Uuid;

    use crate::test_support::{
        FakeLlmClient, RequestRoute, StreamScript, text_stream_with_usage, tool_use_stream,
    };

    use super::map_wire_event;

    fn session_id() -> SessionId {
        SessionId::new(Uuid::from_u128(1))
    }

    fn round_trip(wire: &WireRunEvent) {
        let json = serde_json::to_string(wire).expect("serialize wire event");
        let back: WireRunEvent = serde_json::from_str(&json).expect("deserialize wire event");
        assert_eq!(&back, wire);
    }

    #[test]
    fn text_delta_maps_and_round_trips() {
        let wire = RunEvent::TextDelta("hel".to_owned()).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::TextDelta {
                id: session_id(),
                text: "hel".to_owned(),
            })
        );
        assert!(final_output.is_none());
    }

    #[test]
    fn done_folds_into_run_output_and_round_trips() {
        let response = Response {
            message: Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "hello".to_owned(),
                    extra: Map::new(),
                }],
            },
            usage: Usage {
                input: 7,
                output: 2,
                total: Some(9),
                ..Usage::default()
            },
            stop_reason: Normalized::from_mapped(StopReason::EndTurn, "end_turn"),
            extra: Map::new(),
        };
        let wire = RunEvent::Done(Box::new(FacadeRunOutput::from(response))).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert!(mapped.is_none());
        let output = final_output.expect("terminal output folded");
        assert_eq!(output.text, "hello");
        assert_eq!(
            output.usage,
            Some(UsageInfo {
                input_tokens: 7,
                output_tokens: 2,
                total_tokens: 9,
            })
        );
    }

    /// Builds a facade [`ToolTrace`](FacadeToolTrace) via serde, since the type
    /// is `#[non_exhaustive]` and has no public struct constructor.
    fn facade_trace(name: &str, call: Uuid) -> FacadeToolTrace {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "call_id": call.to_string(),
        }))
        .expect("deserialize facade tool trace")
    }

    #[test]
    fn tool_started_maps_and_round_trips() {
        let call = Uuid::from_u128(0xabcd);
        let wire = RunEvent::ToolStarted(facade_trace("shell", call)).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::ToolStarted {
                id: session_id(),
                trace: ToolTrace {
                    run_id: None,
                    call_id: ToolCallIdWire::new(call),
                    name: "shell".to_owned(),
                    input: None,
                    output: None,
                    status: ToolStatusWire::Started,
                    message: None,
                },
            })
        );
        assert!(final_output.is_none());
    }

    #[test]
    fn tool_finished_maps_and_round_trips() {
        let call = Uuid::from_u128(0x1234);
        let wire = RunEvent::ToolFinished(facade_trace("read_file", call)).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        let Some(Event::ToolFinished { id, trace }) = mapped else {
            panic!("expected tool_finished, got {mapped:?}");
        };
        assert_eq!(id, session_id());
        assert_eq!(trace.call_id, ToolCallIdWire::new(call));
        assert_eq!(trace.name, "read_file");
        assert_eq!(trace.status, ToolStatusWire::Finished);
    }

    #[test]
    fn approval_requested_is_dropped() {
        // The canonical pause event is `IpcApproval`'s `InteractionRequested`;
        // the facade's fire-and-forget `ApprovalRequested` must not map to a mag
        // event (`docs/DESIGN.md` §3.4).
        let wire = RunEvent::ApprovalRequested(ApprovalRequest::for_tool("shell")).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        assert!(map_wire_event(session_id(), wire, &mut final_output).is_none());
        assert!(final_output.is_none());
    }

    /// Builds a facade delegation trace via serde, since the type is
    /// `#[non_exhaustive]` and has no public struct constructor.
    fn facade_delegation_trace(status: &str) -> FacadeDelegationTrace {
        serde_json::from_value(serde_json::json!({
            "delegate": "researcher",
            "status": status,
            "usage": { "input": 3, "output": 2, "total": 5 },
        }))
        .expect("deserialize facade delegation trace")
    }

    /// The expected wire projection of a facade delegation trace.
    fn wire_delegation_trace(status: DelegationStatusWire) -> DelegationTrace {
        DelegationTrace {
            run_id: None,
            delegate: "researcher".to_owned(),
            status,
            task: None,
            output: None,
            message: None,
            usage: Some(UsageInfo {
                input_tokens: 3,
                output_tokens: 2,
                total_tokens: 5,
            }),
        }
    }

    #[test]
    fn delegation_started_maps_and_round_trips() {
        let wire = RunEvent::DelegationStarted(facade_delegation_trace("completed")).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::DelegationStarted {
                id: session_id(),
                trace: wire_delegation_trace(DelegationStatusWire::Started),
            })
        );
        assert!(final_output.is_none());
    }

    #[test]
    fn delegation_finished_maps_and_round_trips() {
        let wire = RunEvent::DelegationFinished(facade_delegation_trace("completed")).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::DelegationFinished {
                id: session_id(),
                trace: wire_delegation_trace(DelegationStatusWire::Finished),
            })
        );
    }

    #[test]
    fn delegation_failed_maps_and_round_trips() {
        let wire = RunEvent::DelegationFailed(facade_delegation_trace("failed")).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::DelegationFailed {
                id: session_id(),
                trace: wire_delegation_trace(DelegationStatusWire::Failed),
            })
        );
    }

    #[test]
    fn delegation_message_maps_and_round_trips() {
        let message: FacadeDelegationMessage = serde_json::from_value(serde_json::json!({
            "delegate": "researcher",
            "message": "still searching",
        }))
        .expect("deserialize facade delegation message");
        let wire = RunEvent::DelegationMessage(message).to_wire();
        round_trip(&wire);

        let mut final_output = None;
        let mapped = map_wire_event(session_id(), wire, &mut final_output);

        assert_eq!(
            mapped,
            Some(Event::DelegationMessage {
                id: session_id(),
                message: DelegationMessageWire {
                    run_id: None,
                    delegate: "researcher".to_owned(),
                    text: "still searching".to_owned(),
                },
            })
        );
    }

    /// Builds a driver for `cwd` and returns the worktree path recorded in its
    /// committed snapshot, proving `SessionConfig.cwd` reached the facade agent's
    /// [`WorktreeRef`](agent_lib::agent::WorktreeRef).
    fn worktree_for_cwd(cwd: Option<std::path::PathBuf>) -> serde_json::Value {
        let driver = driver_with_cwd(FakeLlmClient::scripted(Vec::new()), cwd);

        let snapshot = driver.agent.snapshot().expect("committed snapshot");
        let json = serde_json::to_value(&snapshot).expect("serialize snapshot");
        json["agent_state"]["spec"]["worktree"].clone()
    }

    /// Builds a driver over the built-in tool registry with a `fake-model`
    /// session config, ready for config-apply tests.
    fn driver_for_test(client: std::sync::Arc<FakeLlmClient>) -> super::SessionDriver {
        driver_with_cwd(client, None)
    }

    /// Builds a driver whose session binding is resolved from `snapshot`
    /// over the full built-in registry.
    fn driver_with_binding(
        client: std::sync::Arc<FakeLlmClient>,
        snapshot: &mag_config::ConfigSnapshot,
    ) -> super::SessionDriver {
        driver_with_snapshot_tools(client, snapshot, mag_tools::ToolRegistry::with_builtins())
    }

    /// Builds a driver whose session binding and tool registry are both
    /// assembled from `snapshot` (the production `Engine::from_config`
    /// wiring: `[tools.<name>] enabled = false` entries leave the registry).
    fn driver_with_filtered_registry(
        client: std::sync::Arc<FakeLlmClient>,
        snapshot: &mag_config::ConfigSnapshot,
    ) -> super::SessionDriver {
        driver_with_snapshot_tools(
            client,
            snapshot,
            crate::assembly::assemble_tool_registry(snapshot),
        )
    }

    /// Builds a driver whose session binding is resolved from `snapshot`
    /// over the supplied tool registry.
    fn driver_with_snapshot_tools(
        client: std::sync::Arc<FakeLlmClient>,
        snapshot: &mag_config::ConfigSnapshot,
        tools: mag_tools::ToolRegistry,
    ) -> super::SessionDriver {
        use crate::EventBus;
        use crate::assembly::{ApprovalOverrides, SessionBinding};
        use crate::engine::approval::{AskFrontendDecider, IpcApproval};
        use crate::turn_complete::TurnCompleteHub;

        let config = mag_service::SessionConfig {
            provider: "fake".to_owned(),
            model: "fake-model".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: mag_service::RoutingMode::ModelRouted,
            budget: None,
        };
        let events = EventBus::new();
        let approval = std::sync::Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            std::sync::Arc::new(AskFrontendDecider),
        ));
        super::SessionDriver::new(
            &config,
            client,
            std::sync::Arc::new(tools),
            approval,
            TurnCompleteHub::default(),
            &SessionBinding::resolve(&config, Some(snapshot)),
            &ApprovalOverrides::default(),
            session_id(),
            events,
        )
        .expect("build session driver")
    }

    fn driver_with_cwd(
        client: std::sync::Arc<FakeLlmClient>,
        cwd: Option<std::path::PathBuf>,
    ) -> super::SessionDriver {
        use crate::EventBus;
        use crate::assembly::{ApprovalOverrides, SessionBinding};
        use crate::engine::approval::{AskFrontendDecider, IpcApproval};
        use crate::turn_complete::TurnCompleteHub;

        let config = mag_service::SessionConfig {
            provider: "fake".to_owned(),
            model: "fake-model".to_owned(),
            tool_profile: None,
            cwd,
            routing: mag_service::RoutingMode::ModelRouted,
            budget: None,
        };
        let events = EventBus::new();
        let approval = std::sync::Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            std::sync::Arc::new(AskFrontendDecider),
        ));
        super::SessionDriver::new(
            &config,
            client,
            std::sync::Arc::new(mag_tools::ToolRegistry::with_builtins()),
            approval,
            TurnCompleteHub::default(),
            &SessionBinding::resolve(&config, None),
            &ApprovalOverrides::default(),
            session_id(),
            events,
        )
        .expect("build session driver")
    }

    #[test]
    fn new_carries_config_cwd_into_agent_worktree() {
        let worktree = worktree_for_cwd(Some(std::path::PathBuf::from("/work/session-root")));
        assert_eq!(worktree, serde_json::json!("/work/session-root"));
    }

    #[test]
    fn new_without_cwd_keeps_default_worktree() {
        let worktree = worktree_for_cwd(None);
        assert_eq!(worktree, serde_json::json!("."));
    }

    /// M3-5 main wiring: a freshly built session advertises the `agent` tool
    /// trio on its surface, and the `agent` description enumerates the builtin
    /// definitions (the merged table's lowest layer).
    #[test]
    fn new_exposes_the_agent_instance_tools() {
        let driver = driver_for_test(FakeLlmClient::scripted(Vec::new()));

        let tools = driver.agent.state().current_tool_set().tools();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        for tool in ["agent", "agent_result", "agent_cancel"] {
            assert!(names.contains(&tool), "{tool} on the surface: {names:?}");
        }
        let agent = tools
            .iter()
            .find(|tool| tool.name == "agent")
            .expect("the agent tool is advertised");
        for expected in ["general-purpose", "explorer"] {
            assert!(
                agent.description.contains(expected),
                "the agent description enumerates `{expected}`: {}",
                agent.description
            );
        }
    }

    /// M3-5 config apply: the TOML definition layer is rebuilt from the
    /// applied snapshot and re-merged, so a definition added by the apply
    /// becomes spawnable for later spawns; the shared supervisor model follows
    /// an applied `SetModel` once it lands at the next turn start.
    #[test]
    fn apply_config_rebuilds_the_definition_table_for_later_spawns() {
        driver_test_runtime().block_on(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            assert!(
                driver.spawn_ctx.shared.definition("researcher").is_none(),
                "no researcher definition before the apply"
            );
            let dto = ConfigDto::parse_str(
                r#"
[agents.default]
model = "model-b"

[agents.researcher]
role = "Researches topics."
"#,
            )
            .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");

            driver.apply_config(session_id(), &snapshot);

            let definition = driver
                .spawn_ctx
                .shared
                .definition("researcher")
                .expect("researcher definition after the apply");
            assert_eq!(definition.description, "Researches topics.");
            assert_eq!(
                driver.spawn_ctx.shared.supervisor_model().model(),
                "model-b",
                "the shared supervisor model mirrors the queued SetModel"
            );

            // The queued SetModel lands at the next turn start with exactly
            // the mirrored value.
            drive_one_turn(&mut driver).await;
            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].model, "model-b");
        });
    }

    /// §7 (post-F-R): the session's default subagent toolset
    /// (`[session].default_subagent_tools`) seeds the shared spawn state at
    /// build and is re-read from the applied snapshot at config apply. The
    /// bound entry's own tool list plays no role in it — the M3-R surface
    /// filter is gone, so the supervisor's surface never constrains a child.
    #[test]
    fn default_subagent_tools_seed_at_build_and_follow_config_apply() {
        use mag_config::{ConfigDto, ConfigSnapshot};

        // An unconfigured session has no default toolset.
        let driver = driver_for_test(FakeLlmClient::scripted(Vec::new()));
        assert_eq!(driver.spawn_ctx.shared.default_tools(), None);

        // `[session].default_subagent_tools` seeds the spawn state at build.
        let dto = ConfigDto::parse_str(
            r#"
[session]
default_subagent_tools = ["read_file", "shell"]
"#,
        )
        .expect("config parses");
        let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");
        let mut driver = driver_with_binding(FakeLlmClient::scripted(Vec::new()), &snapshot);
        assert_eq!(
            driver.spawn_ctx.shared.default_tools(),
            Some(vec!["read_file".to_owned(), "shell".to_owned()])
        );

        // A config apply re-reads the `[session]` value (the bound entry's
        // own tool list is irrelevant to it).
        let dto = ConfigDto::parse_str(
            r#"
[agents.default]
tools = ["shell"]

[session]
default_subagent_tools = ["grep"]
"#,
        )
        .expect("config parses");
        let snapshot = ConfigSnapshot::resolve(&dto, 2).expect("config resolves");
        driver.apply_config(session_id(), &snapshot);
        assert_eq!(
            driver.spawn_ctx.shared.default_tools(),
            Some(vec!["grep".to_owned()]),
            "the applied `[session]` value, not the bound tool list"
        );

        // An applied snapshot without the key clears the toolset (the same
        // wholesale rebuild semantics as the definition table).
        let dto = ConfigDto::parse_str(
            r#"
[agents.default]
model = "model-b"
"#,
        )
        .expect("config parses");
        let snapshot = ConfigSnapshot::resolve(&dto, 3).expect("config resolves");
        driver.apply_config(session_id(), &snapshot);
        assert_eq!(driver.spawn_ctx.shared.default_tools(), None);
    }

    /// §7 (post-F-R semantic reversal of the M3-R anti-escalation rule): the
    /// supervisor's bound tool list constrains only the supervisor itself. A
    /// spawned child's surface is a property of the agent definition — a
    /// definition without `tools` gets the full session registry, and an
    /// explicit `tools` list is applied exactly, even where it names plugins
    /// the supervisor itself does not have.
    #[test]
    fn child_surface_ignores_the_supervisor_bound_tool_list() {
        use crate::instances::spawn::SUBAGENT_SKELETON;

        driver_local(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            // The supervisor binds to a one-tool surface.
            let dto = ConfigDto::parse_str("[agents.default]\ntools = [\"read_file\"]\n")
                .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");
            let client = FakeLlmClient::scripted_routes(vec![
                RequestRoute::system_contains(
                    SUBAGENT_SKELETON,
                    vec![
                        StreamScript::Complete(text_stream_with_usage(&["report"], usage(1, 1))),
                        StreamScript::Complete(text_stream_with_usage(&["report"], usage(1, 1))),
                    ],
                ),
                RequestRoute::any(vec![
                    StreamScript::Complete(tool_use_stream(
                        "agent",
                        "call-1",
                        serde_json::json!({ "type": "general-purpose", "task": "t" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["done"], usage(1, 1))),
                    StreamScript::Complete(tool_use_stream(
                        "agent",
                        "call-2",
                        serde_json::json!({ "type": "explorer", "task": "t" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["done"], usage(1, 1))),
                ]),
            ]);
            let mut driver = driver_with_binding(client.clone(), &snapshot);

            // Sanity: the supervisor's own surface really is narrowed to
            // `read_file` plus the instance tool trio.
            let names: Vec<&str> = driver
                .agent
                .state()
                .current_tool_set()
                .tools()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(
                names,
                ["read_file", "agent", "agent_result", "agent_cancel"]
            );

            // A definition without `tools` spawns a child on the full
            // session registry, not on the supervisor's narrowed surface.
            drive_one_turn(&mut driver).await;
            let instance = driver
                .spawn_ctx
                .registry
                .get("general-purpose-1")
                .expect("instance registered");
            await_instance_terminal(&instance).await;

            // An explicit `tools` list is applied exactly: `explorer` gets
            // `grep`/`list_dir`, which the supervisor itself does not have.
            drive_one_turn(&mut driver).await;
            let instance = driver
                .spawn_ctx
                .registry
                .get("explorer-1")
                .expect("instance registered");
            await_instance_terminal(&instance).await;

            let chat_requests = client.chat_requests();
            assert_eq!(chat_requests.len(), 2);
            let mut first: Vec<&str> = chat_requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            first.sort_unstable();
            assert_eq!(
                first,
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
            let mut second: Vec<&str> = chat_requests[1]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            second.sort_unstable();
            assert_eq!(
                second,
                [
                    "agent",
                    "agent_cancel",
                    "agent_result",
                    "grep",
                    "list_dir",
                    "read_file"
                ]
            );
        });
    }

    /// §7's session-level hard boundary: a `[tools.<name>] enabled = false`
    /// tool never enters the session registry, so a child surface naming it
    /// (here through a TOML definition's `tools` list) drops it.
    #[test]
    fn child_surface_drops_tools_disabled_at_session_level() {
        use crate::instances::spawn::SUBAGENT_SKELETON;

        driver_local(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            let dto = ConfigDto::parse_str(
                r#"
[agents.default]

[agents.reviewer]
role = "Reviews changes."
tools = ["read_file", "shell"]

[tools.shell]
enabled = false
"#,
            )
            .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");
            let client = FakeLlmClient::scripted_routes(vec![
                RequestRoute::system_contains(
                    SUBAGENT_SKELETON,
                    vec![StreamScript::Complete(text_stream_with_usage(
                        &["report"],
                        usage(1, 1),
                    ))],
                ),
                RequestRoute::any(vec![
                    StreamScript::Complete(tool_use_stream(
                        "agent",
                        "call-1",
                        serde_json::json!({ "type": "reviewer", "task": "t" }),
                    )),
                    StreamScript::Complete(text_stream_with_usage(&["done"], usage(1, 1))),
                ]),
            ]);
            let mut driver = driver_with_filtered_registry(client.clone(), &snapshot);

            drive_one_turn(&mut driver).await;
            let instance = driver
                .spawn_ctx
                .registry
                .get("reviewer-1")
                .expect("instance registered");
            await_instance_terminal(&instance).await;

            let chat_requests = client.chat_requests();
            assert_eq!(chat_requests.len(), 1);
            let mut names: Vec<&str> = chat_requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            names.sort_unstable();
            assert_eq!(
                names,
                ["agent", "agent_cancel", "agent_result", "read_file"],
                "`shell` is disabled session-wide and drops out of the child surface"
            );
        });
    }

    /// M3-5 restore compatibility: a snapshot persisted before the static
    /// delegation path was retired carries delegate recipes and their
    /// synthesized `ask_<name>` declarations. Restore must neither fail nor
    /// resurrect them: the legacy delegate is pruned and its declaration
    /// leaves the surface, while the instance tools are re-injected.
    #[test]
    fn restore_sweeps_legacy_delegates_from_an_old_snapshot() {
        let driver = driver_for_test(FakeLlmClient::scripted(Vec::new()));
        let snapshot = driver.agent.snapshot().expect("committed snapshot");
        let snapshot = legacy_snapshot_with_delegate(snapshot);

        let restored = restored_driver(snapshot);

        let names: Vec<&str> = restored
            .agent
            .state()
            .current_tool_set()
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        for tool in ["agent", "agent_result", "agent_cancel"] {
            assert!(
                names.contains(&tool),
                "{tool} on the restored surface: {names:?}"
            );
        }
        assert!(
            !names.contains(&"ask_legacy"),
            "the legacy delegate start tool is pruned, not resurrected: {names:?}"
        );
        assert!(
            restored.agent.subagents().is_empty(),
            "the legacy delegate itself is pruned from the roster"
        );
    }

    /// Injects a legacy local delegate recipe and its synthesized
    /// `ask_legacy` declaration into a snapshot, mimicking what a pre-M3-5
    /// session persisted (the declaration lived in the initial tool set — the
    /// serialized record omits `current_tool_set` while it matches the
    /// initial set; the recipe lived in the delegate roster). Typed shapes
    /// are reused from the snapshot itself so the fixture tracks agent-lib's
    /// serialized form.
    fn legacy_snapshot_with_delegate(
        snapshot: agent_lib::facade::AgentSnapshot,
    ) -> agent_lib::facade::AgentSnapshot {
        let mut json = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let spec = json["agent_state"]["spec"].clone();
        let tool_set_id = json["agent_state"]["spec"]["initial_tools"]["id"].clone();
        let ask_decl = serde_json::json!({
            "name": "ask_legacy",
            "description": "Start the legacy delegate.",
            "input_schema": {
                "type": "object",
                "properties": { "task": { "type": "string" } },
            },
        });
        json.pointer_mut("/agent_state/spec/initial_tools/tools")
            .and_then(serde_json::Value::as_array_mut)
            .expect("snapshot carries the initial tool set")
            .push(ask_decl);
        json["delegates"]
            .as_array_mut()
            .expect("delegate roster")
            .push(serde_json::json!({
                "name": "legacy",
                "description": "legacy delegate",
                "spec": spec,
                "tools": { "id": tool_set_id, "tools": [] },
                "inherit_model": true,
            }));
        serde_json::from_value(json).expect("deserialize legacy snapshot")
    }

    /// Builds a driver restored from `snapshot`, mirroring the session actor's
    /// resume path.
    fn restored_driver(snapshot: agent_lib::facade::AgentSnapshot) -> super::SessionDriver {
        use crate::EventBus;
        use crate::assembly::{ApprovalOverrides, SessionBinding};
        use crate::engine::approval::{AskFrontendDecider, IpcApproval};
        use crate::turn_complete::TurnCompleteHub;

        let config = mag_service::SessionConfig {
            provider: "fake".to_owned(),
            model: "fake-model".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: mag_service::RoutingMode::ModelRouted,
            budget: None,
        };
        let events = EventBus::new();
        let approval = std::sync::Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            std::sync::Arc::new(AskFrontendDecider),
        ));
        super::SessionDriver::restore(
            &config,
            FakeLlmClient::scripted(Vec::new()),
            std::sync::Arc::new(mag_tools::ToolRegistry::with_builtins()),
            approval,
            snapshot,
            config.budget.as_ref(),
            TurnCompleteHub::default(),
            &SessionBinding::resolve(&config, None),
            &ApprovalOverrides::default(),
            session_id(),
            events,
        )
        .expect("restore session driver")
    }

    /// M3-5 session-end cancel cascade: dropping the driver cancels every
    /// still-running instance (instances never outlive their session).
    #[test]
    fn drop_cancels_all_running_instances() {
        use crate::instances::{Instance, InstanceStatus};

        let driver = driver_for_test(FakeLlmClient::scripted(Vec::new()));
        let registry = driver.spawn_ctx.registry.clone();
        let instance = registry.register(Instance::new(
            "general-purpose-1".to_owned(),
            "general-purpose".to_owned(),
            1,
        ));
        let cancel_handle = instance.cancel_handle();

        drop(driver);

        assert_eq!(instance.status(), InstanceStatus::Cancelled);
        assert!(cancel_handle.is_cancelled());
    }

    /// TODO M3-5 (d): a rejected reconfigure item (here an immutable skill
    /// variant, which the facade rejects with `FacadeError::Config`) is warned
    /// and skipped without interrupting the remaining items or the agent.
    #[test]
    fn reconfig_items_apply_independently_with_rejections_skipped() {
        driver_test_runtime().block_on(async {
            use agent_lib::agent::SkillId;
            use agent_lib::facade::{ModelRef, ReconfigRequest};

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            let max_tokens = driver.agent.state().current_model().max_tokens();

            let requests = vec![
                ReconfigRequest::ActivateSkill {
                    skill_id: SkillId::new(Uuid::from_u128(42)),
                },
                ReconfigRequest::SetModel {
                    model: ModelRef::new("model-b", max_tokens, None, None),
                },
            ];
            driver.apply_reconfig_items(session_id(), requests);

            // The skill request was rejected (facade: no skill registry), yet
            // the later model item still landed and renders into the next
            // turn's request.
            drive_one_turn(&mut driver).await;
            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].model, "model-b");
        });
    }

    /// `apply_config` projects the snapshot's `agents.default` entry onto the
    /// agent: a changed model becomes `SetModel`, the configured system prompt
    /// becomes `SetSystemPromptOverlay`, and the enabled tool list becomes a
    /// filtered `ReplaceToolSet` (`docs/CLI.md` §4.4). Queued reconfigurations
    /// apply at the next turn start (agent-lib turn-boundary semantics), so the
    /// effect is observed on the next run's request.
    #[test]
    fn apply_config_projects_model_and_tool_subset_from_snapshot() {
        driver_test_runtime().block_on(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            let dto = ConfigDto::parse_str(
                r#"
[agents.default]
model = "model-b"
system_prompt = "apply system"
tools = ["read_file", "shell"]
"#,
            )
            .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");

            driver.apply_config(session_id(), &snapshot);
            drive_one_turn(&mut driver).await;

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].model, "model-b");
            assert_eq!(requests[0].system.as_deref(), Some("apply system"));
            let names: Vec<&str> = requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec![
                    "read_file",
                    "shell",
                    "agent",
                    "agent_result",
                    "agent_cancel"
                ],
                "the narrowed plugins plus the instance tools (M3-5)"
            );
        });
    }

    /// A disabled tool is filtered out, and a config tool name with no
    /// registered plugin is skipped (warn log) instead of poisoning the whole
    /// replacement set.
    #[test]
    fn apply_config_skips_disabled_and_unknown_tools() {
        driver_test_runtime().block_on(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            let dto = ConfigDto::parse_str(
                r#"
[agents.default]
tools = ["read_file", "shell", "ghost"]

[tools.shell]
enabled = false
"#,
            )
            .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");

            driver.apply_config(session_id(), &snapshot);
            drive_one_turn(&mut driver).await;

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            let names: Vec<&str> = requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec!["read_file", "agent", "agent_result", "agent_cancel"],
                "disabled and unknown tools skipped; the instance tools remain (M3-5)"
            );
        });
    }

    /// A snapshot without an `agents.default` entry (or without relevant
    /// fields) changes nothing.
    #[test]
    fn apply_config_without_agent_entry_is_a_noop() {
        driver_test_runtime().block_on(async {
            use mag_config::ConfigSnapshot;

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            let snapshot = ConfigSnapshot::resolve(&mag_config::ConfigDto::default(), 1)
                .expect("default config resolves");

            driver.apply_config(session_id(), &snapshot);
            drive_one_turn(&mut driver).await;

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].model, "fake-model");
            let names: Vec<&str> = requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec![
                    "read_file",
                    "list_dir",
                    "grep",
                    "shell",
                    "ask_user",
                    "agent",
                    "agent_result",
                    "agent_cancel",
                ]
            );
        });
    }

    /// An explicit `tools = []` on the bound entry clears the session's
    /// plugin surface: the replacement set carries only the instance tools
    /// (`agent` / `agent_result` / `agent_cancel`, `docs/dyn-agents.md` §5.1),
    /// which always survive a surface narrowing, unlike an absent `tools` key
    /// which leaves the surface untouched.
    #[test]
    fn apply_config_clears_the_surface_on_an_explicit_empty_tool_list() {
        driver_test_runtime().block_on(async {
            use mag_config::{ConfigDto, ConfigSnapshot};

            let client =
                FakeLlmClient::scripted(vec![text_stream_with_usage(&["ok"], usage(1, 1))]);
            let mut driver = driver_for_test(client.clone());
            let dto = ConfigDto::parse_str(
                r#"
[agents.default]
tools = []
"#,
            )
            .expect("config parses");
            let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("config resolves");

            driver.apply_config(session_id(), &snapshot);
            drive_one_turn(&mut driver).await;

            let requests = client.stream_requests();
            assert_eq!(requests.len(), 1);
            let names: Vec<&str> = requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec!["agent", "agent_result", "agent_cancel"],
                "an explicit empty list exposes no plugins, only the instance tools: {names:?}"
            );
        });
    }

    /// Single-threaded runtime for driver tests: the facade run stream is not
    /// `Send`, mirroring the session actor's `current_thread` discipline.
    fn driver_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build driver test runtime")
    }

    /// Runs `test` on the driver-test runtime inside a
    /// [`tokio::task::LocalSet`], so the `agent` tool handler's
    /// `tokio::task::spawn_local` works (mirrors the session actor's `!Send`
    /// discipline, `crate::session`).
    fn driver_local<T>(test: impl std::future::Future<Output = T>) -> T {
        let runtime = driver_test_runtime();
        tokio::task::LocalSet::new().block_on(&runtime, test)
    }

    /// Polls the instance until it reaches a terminal status (5s backstop; a
    /// hang is a bug).
    async fn await_instance_terminal(instance: &std::sync::Arc<crate::instances::Instance>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !instance.status().is_terminal() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("instance reaches a terminal status");
    }

    /// Drives one turn through the driver's own `run_turn` and asserts it
    /// completes, so config-apply tests observe the queued reconfiguration the
    /// way production turns do.
    async fn drive_one_turn(driver: &mut super::SessionDriver) {
        use agent_lib::facade::CancelHandle;

        use crate::EventBus;
        use crate::persistence::Persistence;

        let events = EventBus::new();
        let cancel = CancelHandle::new();
        let pivots = super::PivotQueue::new();
        let store = Persistence::in_memory().expect("in-memory store");
        let outcome = driver
            .run_turn(
                session_id(),
                "hi".to_owned(),
                &events,
                &cancel,
                &pivots,
                &store,
            )
            .await;
        assert!(
            matches!(outcome, super::TurnOutcome::Completed),
            "the post-apply turn must complete"
        );
    }

    fn usage(input: u32, output: u32) -> agent_lib::model::usage::Usage {
        agent_lib::model::usage::Usage {
            input,
            output,
            total: Some(input + output),
            ..agent_lib::model::usage::Usage::default()
        }
    }
}
