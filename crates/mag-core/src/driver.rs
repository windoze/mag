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
//! [`IpcApproval`]; tools without a permission stay auto-allowed and never
//! interrupt the run.

use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

use std::time::Duration;

use agent_lib::{
    agent::{BudgetLimits, InteractionHandler, WorktreeRef},
    client::LlmClient,
    facade::{
        Agent, AgentRunStream, AgentSnapshot, ApprovalPolicy, CancelHandle, FacadeError, ModelRef,
        ReconfigRequest, Tool, ToolContext, ToolResult, ToolSetId, ToolSetRef,
        ToolTrace as FacadeToolTrace, UsageSummary, WireRunEvent, WireRunOutput,
    },
};
use mag_config::ConfigSnapshot;
use mag_service::{
    Event, RunErrorKind, RunId as WireRunId, RunOutput, SessionBudget, SessionConfig, SessionId,
    ToolCallIdWire, ToolStatusWire, ToolTrace, UsageInfo,
};
use mag_tools::{ToolPlugin, ToolRegistry};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    EventBus,
    engine::approval::IpcApproval,
    persistence::Persistence,
    turn_complete::{TurnCompleteHub, TurnCompletion, TurnSummary},
};

const DEFAULT_MAX_TOKENS: u32 = 512;
const DEFAULT_MAX_STEPS: u32 = 8;

/// Name of the `agents.<name>` entry a session reconfigures from on
/// `apply_config` (`docs/CLI.md` §4.4).
///
/// Provisional binding: sessions do not yet carry an agent name (they are
/// created from a wire [`SessionConfig`]), so the runtime config apply reads
/// the well-known `default` agent entry — the same entry the §4.2 example
/// config defines. The session↔agent binding becomes explicit when
/// `Engine::from_config` lands (M3-6).
const DEFAULT_AGENT_NAME: &str = "default";

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
    run_counter: AtomicU64,
    /// Mints fresh tool-set identities for `apply_config` reconfigurations.
    tool_set_counter: AtomicU64,
}

impl SessionDriver {
    /// Builds a fresh facade [`Agent`] for the supplied session configuration.
    ///
    /// The `tools` registry is projected onto the agent: each plugin becomes a
    /// facade [`Tool`], and any plugin declaring a
    /// [`permission`](ToolPlugin::permission) is gated behind
    /// [`ApprovalPolicy::ask_tool`] so it pauses through `approval`. The shared
    /// [`IpcApproval`] is injected as the interaction handler and stays the sole
    /// authority answering a paused tool call (`docs/DESIGN.md` §3.3).
    ///
    /// When [`config.cwd`](SessionConfig::cwd) is set, that path becomes the
    /// agent's [`WorktreeRef`] so the built-in tools resolve relative to the
    /// interface-supplied session root (for ACP, the client's `cwd`,
    /// `docs/ACP.md` §3.2/§6); a `None` cwd keeps the facade default `"."`.
    ///
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while assembling the agent (for
    /// example an invalid model/provider configuration).
    pub(crate) fn new(
        config: &SessionConfig,
        client: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        approval: Arc<IpcApproval>,
        turn_complete: TurnCompleteHub,
    ) -> Result<Self, FacadeError> {
        let (facade_tools, policy) = tool_surface(&tools);
        let mut builder = Agent::builder()
            .client(client)
            .model(config.model.clone())
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

        Ok(Self {
            agent,
            tools,
            turn_complete,
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
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while rebuilding the agent (for example
    /// a snapshot whose state cannot be deserialized).
    pub(crate) fn restore(
        client: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        approval: Arc<IpcApproval>,
        snapshot: AgentSnapshot,
        budget: Option<&SessionBudget>,
        turn_complete: TurnCompleteHub,
    ) -> Result<Self, FacadeError> {
        let (facade_tools, policy) = tool_surface(&tools);
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
        let agent = builder.approval(policy).build()?;

        Ok(Self {
            agent,
            tools,
            turn_complete,
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
        let mut stream = match self.agent.stream_with_cancel(text, cancel.clone()).await {
            Ok(stream) => stream,
            Err(error) => {
                drop_pivots(session_id, pivots, events, PIVOT_DROP_RUN_FAILED);
                let _ = events.emit(Event::RunError {
                    id: session_id,
                    message: error.to_string(),
                    kind: error_kind(&error),
                });
                turn_complete.notify(&TurnSummary::new(session_id, TurnCompletion::Failed));
                return TurnOutcome::Failed {
                    kind: error_kind(&error),
                    message: error.to_string(),
                };
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
                    // queued pivots before the next poll drives past it.
                    drain_pivots(session_id, &mut stream, pivots, events);
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
                None => break TurnOutcome::Completed,
            }
        };

        // Release the mutable agent borrow before emitting the terminal event; a
        // cancelled or failed turn is abandoned (committed history is left
        // intact) so the next `run_turn` on this driver can proceed.
        drop(stream);

        // Any pivot still queued at the end of the run never reached a step
        // boundary: report it dropped with the run's terminal reason.
        drop_pivots(session_id, pivots, events, outcome.pivot_drop_reason());

        let terminal = match &outcome {
            TurnOutcome::Completed => match final_output {
                Some(output) => {
                    // Persist the committed snapshot before announcing completion.
                    self.persist_committed_snapshot(session_id, store);
                    Event::RunFinished {
                        id: session_id,
                        output,
                    }
                }
                None => Event::RunError {
                    id: session_id,
                    message: "agent stream ended without a terminal `Done` event".to_owned(),
                    kind: RunErrorKind::Other,
                },
            },
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

    /// Applies the runtime configuration snapshot to this session's agent at a
    /// turn boundary (`docs/CLI.md` §4.4, decision D2).
    ///
    /// The caller (the session actor) only invokes this while the agent is at
    /// rest — right after a run terminal, or immediately when the session has
    /// no in-progress run — satisfying agent-lib's reconfigure admission rule
    /// (Idle/between-runs only).
    ///
    /// Field mapping from the snapshot's [`DEFAULT_AGENT_NAME`] entry:
    ///
    /// - `model` → [`ReconfigRequest::SetModel`] (only when it actually
    ///   changed; `max_tokens`/`temperature` keep their current values since
    ///   the config schema carries no LLM sampling parameters yet).
    /// - `tools` → [`ReconfigRequest::ReplaceToolSet`] with the declarations
    ///   of the enabled entries, projected from this driver's executable
    ///   [`ToolRegistry`] (only when the effective name set changed). A config
    ///   tool name with no registered plugin is skipped with a warn log — the
    ///   facade would reject the whole set for referencing a tool outside its
    ///   registry. An agent entry with no tool list imposes no constraint and
    ///   leaves the current surface untouched.
    ///
    /// Out of scope on the current agent-lib reconfigure surface (documented
    /// for M3-R/M3-6): the approval policy is baked into the agent at build
    /// time and has no reconfigure variant, so `tools.*.approval` /
    /// `approval.*` changes take effect on the next session (re)build rather
    /// than mid-session; per-run `budget` is likewise build-time only (and
    /// `session` defaults only affect new sessions per decision D2).
    pub(crate) fn apply_config(&mut self, session_id: SessionId, snapshot: &ConfigSnapshot) {
        let requests = self.reconfig_requests(session_id, snapshot);
        self.apply_reconfig_items(session_id, requests);
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

    /// Builds the reconfigure batch projecting the snapshot's
    /// [`DEFAULT_AGENT_NAME`] entry onto this session's agent.
    fn reconfig_requests(
        &mut self,
        session_id: SessionId,
        snapshot: &ConfigSnapshot,
    ) -> Vec<ReconfigRequest> {
        let Some(agent_config) = snapshot.agent(DEFAULT_AGENT_NAME) else {
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

        let wanted: Vec<&str> = agent_config
            .tools()
            .iter()
            .filter(|tool| tool.is_enabled())
            .map(|tool| tool.name())
            .collect();
        if !wanted.is_empty() {
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

/// Drains the run's pivot queue, attempting [`AgentRunStream::interject`] on
/// each queued pivot (`docs/CLI.md` §3.2).
///
/// Called after every stream poll, which is exactly when the run may be
/// parked on a step boundary with the facade's pivot window open. A pivot
/// accepted by the facade is announced as [`Event::PivotApplied`].
/// [`FacadeError::InvalidState`] means the window is not (or no longer) open —
/// a side-effect-free, retry-safe rejection — so the pivot returns to the
/// front of the queue and is retried after the next poll. Any other error is
/// permanent: the pivot leaves the queue as [`Event::PivotDropped`].
fn drain_pivots(
    session_id: SessionId,
    stream: &mut AgentRunStream<'_>,
    pivots: &PivotQueue,
    events: &EventBus,
) {
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

/// Maps one projected [`WireRunEvent`] into a mag [`Event`].
///
/// A terminal [`WireRunEvent::Done`] is folded into `final_output` and produces
/// no streamed event (the caller emits [`RunFinished`](Event::RunFinished) once
/// the stream drains). Tool lifecycle events project into mag's
/// [`ToolStarted`](Event::ToolStarted) / [`ToolFinished`](Event::ToolFinished).
///
/// The facade's [`ApprovalRequested`](WireRunEvent::ApprovalRequested) is
/// intentionally dropped: it is a fire-and-forget notification, and mag's
/// canonical pause event is the [`InteractionRequested`](Event::InteractionRequested)
/// that [`IpcApproval`] emits independently on the pause point (`docs/DESIGN.md`
/// §3.4). Delegation, escalation, and raw variants are not produced by the
/// current milestone, so they are ignored here.
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
/// Each plugin becomes a facade [`Tool`]; a plugin declaring a
/// [`permission`](ToolPlugin::permission) is additionally gated behind
/// [`ApprovalPolicy::ask_tool`] so it pauses through the injected
/// [`IpcApproval`](crate::engine::approval::IpcApproval), while a permission-free
/// (read-only) plugin stays on the default auto-allow tier and never interrupts
/// the run (`docs/DESIGN.md` §3.2/§3.3).
fn tool_surface(tools: &ToolRegistry) -> (Vec<Tool>, ApprovalPolicy) {
    let mut policy = ApprovalPolicy::default();
    let mut facade_tools = Vec::new();
    for plugin in tools.plugins() {
        if plugin.permission().is_some() {
            policy = policy.ask_tool(plugin.name());
        }
        facade_tools.push(facade_tool(Arc::clone(plugin)));
    }
    (facade_tools, policy)
}

/// Projects one facade [`Tool`] from a [`ToolPlugin`].
///
/// The plugin's [`declaration`](ToolPlugin::declaration) supplies the model-facing
/// name, description, and JSON input schema; the executor forwards the run-scoped
/// [`ToolContext`] and raw JSON arguments to
/// [`ToolPlugin::invoke`](ToolPlugin::invoke). The plugin already encodes success
/// and recoverable failure in its [`ToolResult`] status, so the executor is
/// infallible from the facade's point of view.
fn facade_tool(plugin: Arc<dyn ToolPlugin>) -> Tool {
    let declaration = plugin.declaration();
    Tool::function_with_schema(
        declaration.name,
        declaration.description,
        declaration.input_schema,
        move |ctx: ToolContext, args: Value| {
            let plugin = Arc::clone(&plugin);
            async move { Ok::<ToolResult, Infallible>(plugin.invoke(ctx, args).await) }
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
    UsageInfo {
        input_tokens: u64::from(usage.input),
        output_tokens: u64::from(usage.output),
        total_tokens: u64::from(usage.total.unwrap_or_else(|| usage.total_computed())),
    }
}

#[cfg(test)]
mod tests {
    use agent_lib::facade::{ApprovalRequest, ToolTrace as FacadeToolTrace};
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
    use mag_service::{Event, SessionId, ToolCallIdWire, ToolStatusWire, ToolTrace, UsageInfo};
    use serde_json::Map;
    use uuid::Uuid;

    use crate::test_support::{FakeLlmClient, text_stream_with_usage};

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

    fn driver_with_cwd(
        client: std::sync::Arc<FakeLlmClient>,
        cwd: Option<std::path::PathBuf>,
    ) -> super::SessionDriver {
        use crate::EventBus;
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
        let approval = std::sync::Arc::new(IpcApproval::new(
            session_id(),
            EventBus::new(),
            std::sync::Arc::new(AskFrontendDecider),
        ));
        super::SessionDriver::new(
            &config,
            client,
            std::sync::Arc::new(mag_tools::ToolRegistry::with_builtins()),
            approval,
            TurnCompleteHub::default(),
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
    /// agent: a changed model becomes `SetModel`, and the enabled tool list
    /// becomes a filtered `ReplaceToolSet` (`docs/CLI.md` §4.4). Queued
    /// reconfigurations apply at the next turn start (agent-lib turn-boundary
    /// semantics), so the effect is observed on the next run's request.
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
            let names: Vec<&str> = requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect();
            assert_eq!(names, vec!["read_file", "shell"]);
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
            assert_eq!(names, vec!["read_file"]);
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
            assert_eq!(names, vec!["read_file", "list_dir", "grep", "shell"]);
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
