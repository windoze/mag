//! Cross-transport asynchronous approval: the [`IpcApproval`] handler.
//!
//! `docs/DESIGN.md` §3.3 / §9.1 make asynchronous approval the architectural
//! foundation of mag: a paused tool call must be able to suspend **across a
//! process or transport boundary** — the request is emitted to the interface,
//! the machine parks on an `await`, and the run only resumes once the interface
//! delivers a decision. [`IpcApproval`] implements agent-lib's lower-layer
//! [`InteractionHandler`] to realise exactly that: [`fulfill`](IpcApproval::fulfill)
//! registers a pending [`oneshot`] channel, emits an
//! [`Event::InteractionRequested`], and awaits the response, so the agent
//! machine genuinely stops at `.await` (unlike the facade's synchronous
//! `FacadeApproval`, which decides inline). The paired
//! [`respond`](IpcApproval::respond) delivers the decision from a
//! `RespondInteraction` command and wakes the parked driver.
//!
//! The handler injected into the facade [`Agent`](agent_lib::facade::Agent) is
//! the **sole authority for answering** a paused interaction; *which* tool calls
//! pause remains governed by the facade `ApprovalPolicy` (configured in C3-3).
//! [`InteractionKind::Permission`] requests (local-agent / privileged actions)
//! flow through the same pause, first consulting a [`PermissionDecider`] — the
//! seam future AI-based permission policies plug into (`docs/DESIGN.md` §8.1).
//!
//! Delegated interactions (`docs/CLI.md` §3.3, decision D5): agent-lib routes a
//! child agent's paused interaction to the handler its supervisor injected —
//! this same [`IpcApproval`] — annotated with an
//! [`InteractionOrigin`](agent_lib::agent::InteractionOrigin) (delegate name +
//! delegation depth). [`IpcApproval`] projects that attribution onto the wire
//! [`Event::InteractionRequested`] so the interface can render which delegate
//! is asking, while the single shared pending map routes the interface's
//! response back to the exact delegate that asked.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use agent_lib::agent::{
    AgentId, ApprovalDecision, ApprovalRequirement, ApprovalResponse, CancellationToken,
    Interaction, InteractionHandler, InteractionKind, InteractionOrigin as AgentInteractionOrigin,
    InteractionResponse, PermissionCategory, PermissionDecision, PermissionRequest,
    PermissionResponse, PermissionRisk, RequirementResult, RunContext,
};
use agent_lib::conversation::ToolCallId;
use async_trait::async_trait;
use mag_service::{
    AgentIdWire, ApprovalDecisionWire, ApprovalRequirementWire, Event, InteractionKindWire,
    InteractionOrigin, InteractionResponseWire, PermissionCategoryWire, PermissionDecisionWire,
    PermissionRiskWire, RequestId, ServiceError, SessionId, ToolCallIdWire,
};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::EventBus;

/// Locks `mutex`, recovering the guard from a poisoned lock instead of
/// panicking (mirrors agent-lib's unified poison-recovery policy, M9-1).
fn lock_recovering<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Outcome a [`PermissionDecider`] can reach for an
/// [`InteractionKind::Permission`] request.
///
/// A decider either resolves the request immediately (a rule hit) or defers to
/// the interface by returning `None`, in which case [`IpcApproval`] emits an
/// [`Event::InteractionRequested`] and awaits the interface's answer just like
/// any other interaction.
#[async_trait]
pub(crate) trait PermissionDecider: Send + Sync {
    /// Decides a permission `request`, or returns `None` to ask the interface.
    async fn decide(&self, request: &PermissionRequest) -> Option<PermissionResponse>;
}

/// Default permission decider: always defer to the interface.
///
/// This is the first-version policy from `docs/DESIGN.md` §8.1 — every
/// permission request is surfaced to the front-end (or ACP client). A future
/// `RuleDecider` / `LlmDecider` replaces this without changing the protocol or
/// the driver.
#[derive(Debug, Default)]
pub(crate) struct AskFrontendDecider;

#[async_trait]
impl PermissionDecider for AskFrontendDecider {
    async fn decide(&self, _request: &PermissionRequest) -> Option<PermissionResponse> {
        None
    }
}

/// A registered, still-unanswered interaction.
struct Pending {
    /// The original request, kept to address and validate the response.
    interaction: Interaction,
    /// The channel that wakes the parked [`fulfill`](IpcApproval::fulfill).
    responder: oneshot::Sender<InteractionResponse>,
}

/// Removes a pending interaction if the awaiting future is dropped before the
/// interface answers it.
///
/// `ask_user` races the interaction bridge against tool cancellation
/// (`docs/CLI.md` §5 P6). When cancellation wins, the bridge future is dropped;
/// this guard keeps the shared pending map from retaining a stale request id.
struct PendingCleanup<'a> {
    approval: &'a IpcApproval,
    request_id: RequestId,
}

impl Drop for PendingCleanup<'_> {
    fn drop(&mut self) {
        self.approval.discard_pending(self.request_id);
    }
}

/// Mints monotonic [`RequestId`]s scoped to one session's approval handler.
#[derive(Debug, Default)]
struct RequestIdSource {
    counter: AtomicU64,
}

impl RequestIdSource {
    /// Returns the next request identity for this session.
    fn next_id(&self) -> RequestId {
        let value = self.counter.fetch_add(1, Ordering::Relaxed);
        RequestId::new(Uuid::from_u128(u128::from(value)))
    }
}

/// Asynchronous, transport-neutral approval handler injected into a session's
/// facade [`Agent`](agent_lib::facade::Agent).
///
/// One instance is shared (as `Arc`) between the driver's agent — which reaches
/// it through the [`InteractionHandler`] trait — and the session actor, which
/// resolves pending requests through [`respond`](IpcApproval::respond).
///
/// The same instance also answers **delegated** interactions: agent-lib routes
/// a child agent's paused interaction to the handler its supervisor injected
/// (annotating it with an [`AgentInteractionOrigin`]), so every interaction in
/// a delegation chain — any depth — lands here (`docs/CLI.md` §3.3, decision
/// D5). Because this one handler mints every [`RequestId`] of its session, ids
/// can never collide across concurrently parked delegates, and the pending map
/// routes each response back to the exact parked `fulfill` — and therefore the
/// originating delegate — that emitted the request.
pub(crate) struct IpcApproval {
    session_id: SessionId,
    events: EventBus,
    request_ids: RequestIdSource,
    pending: Mutex<HashMap<RequestId, Pending>>,
    decider: Arc<dyn PermissionDecider>,
}

impl IpcApproval {
    /// Builds an approval handler for `session_id`, emitting requests on
    /// `events` and resolving permission requests through `decider`.
    pub(crate) fn new(
        session_id: SessionId,
        events: EventBus,
        decider: Arc<dyn PermissionDecider>,
    ) -> Self {
        Self {
            session_id,
            events,
            request_ids: RequestIdSource::default(),
            pending: Mutex::new(HashMap::new()),
            decider,
        }
    }

    /// Resolves the pending interaction identified by `request_id` with the
    /// interface's `response`, waking the parked driver.
    ///
    /// The core response is reconstructed from the stored request so the
    /// interface only needs to supply the decision (it never learns the internal
    /// `step_id`); the reconstruction is then validated against the request.
    ///
    /// Routing guarantee (`docs/CLI.md` §3.3): every interaction of the session
    /// — the root agent's own and every delegate's, at any depth — is registered
    /// in this one handler's pending map under an id minted by its own
    /// monotonic [`RequestIdSource`], so `request_id` identifies exactly one
    /// parked `fulfill` no matter how many delegates pause concurrently. Sending
    /// on the stored oneshot wakes that specific park, which resumes the
    /// delegate that originated the interaction; responses can never cross over
    /// to a different delegate's request.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InteractionNotFound`] when `request_id` has no
    /// pending interaction, or [`ServiceError::Backend`] when the response does
    /// not match the request family or the parked awaiter has already gone away.
    pub(crate) fn respond(
        &self,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        let (pending, core) = {
            let mut pending = lock_recovering(&self.pending);
            let Some(entry) = pending.get(&request_id) else {
                return Err(ServiceError::InteractionNotFound { request_id });
            };

            let core = interaction_response_from_wire(&entry.interaction, &response)?;
            entry
                .interaction
                .accepts_response(&core)
                .map_err(|error| ServiceError::Backend {
                    message: format!("invalid interaction response: {error}"),
                })?;
            let entry = pending
                .remove(&request_id)
                .expect("pending entry exists after validation");
            (entry, core)
        };
        pending
            .responder
            .send(core)
            .map_err(|_| ServiceError::Backend {
                message: "interaction awaiter dropped before the response arrived".to_owned(),
            })
    }

    /// Returns whether this session currently has an unanswered interaction.
    pub(crate) fn has_pending(&self) -> bool {
        !lock_recovering(&self.pending).is_empty()
    }

    /// Removes a still-pending entry (used when a parked request is cancelled).
    fn discard_pending(&self, request_id: RequestId) {
        lock_recovering(&self.pending).remove(&request_id);
    }

    /// Registers a pending request, emits it to the interface, and parks until a
    /// response arrives or the run's `cancel` token fires.
    ///
    /// The token is propagated through the [`RunContext`] from the facade's
    /// [`CancelHandle`](agent_lib::facade::CancelHandle), so cancelling the
    /// active run wakes this park without any mag-side arming (`docs/DESIGN.md`
    /// §3.3).
    async fn emit_and_await(
        &self,
        request: &Interaction,
        cancel: &CancellationToken,
    ) -> InteractionResponse {
        let request_id = self.request_ids.next_id();
        let (responder, waiter) = oneshot::channel();
        let kind = interaction_kind_to_wire(request.kind());

        lock_recovering(&self.pending).insert(
            request_id,
            Pending {
                interaction: request.clone(),
                responder,
            },
        );
        let _cleanup = PendingCleanup {
            approval: self,
            request_id,
        };

        let _ = self.events.emit(Event::InteractionRequested {
            id: self.session_id,
            request_id,
            kind,
            // Delegate attribution (`docs/CLI.md` §3.3): a child agent's
            // interaction arrives annotated by agent-lib's routing layer; the
            // root session's own interactions map to the default root origin.
            origin: interaction_origin_to_wire(request.origin()),
        });

        tokio::select! {
            resolved = waiter => resolved.unwrap_or_else(|_| cancelled_response(request)),
            () = cancel.cancelled() => {
                self.discard_pending(request_id);
                cancelled_response(request)
            }
        }
    }
}

#[async_trait]
impl InteractionHandler for IpcApproval {
    async fn fulfill(&self, request: &Interaction, ctx: &RunContext) -> RequirementResult {
        // A permission request first consults the decider; a decided outcome
        // short-circuits the interface round-trip (the AI-permission seam, §8.1).
        if let InteractionKind::Permission {
            request: permission,
        } = request.kind()
            && let Some(decided) = self.decider.decide(permission).await
        {
            return RequirementResult::Interaction(InteractionResponse::Permission(decided));
        }

        RequirementResult::Interaction(self.emit_and_await(request, ctx.cancellation()).await)
    }
}

/// Builds the conservative response used when a parked interaction is cancelled.
///
/// Every family resolves without executing the pending action: an approval or
/// permission is cancelled, and the (mag-unused) question/choice families answer
/// with an inert default.
fn cancelled_response(request: &Interaction) -> InteractionResponse {
    match request.kind() {
        InteractionKind::Approval { call_id, .. } => {
            InteractionResponse::Approval(ApprovalResponse::cancel(
                request.step_id(),
                *call_id,
                Some("run cancelled".to_owned()),
            ))
        }
        InteractionKind::Question { .. } => InteractionResponse::answer(String::new()),
        InteractionKind::Choice { .. } => InteractionResponse::Choice(0),
        InteractionKind::Permission {
            request: permission,
        } => InteractionResponse::Permission(PermissionResponse::cancel(
            permission.action_id().to_owned(),
        )),
    }
}

/// Maps agent-lib's delegated-interaction attribution onto the wire
/// [`InteractionOrigin`] (`docs/CLI.md` §3.3, decision D5).
///
/// agent-lib's delegation routing annotates a child agent's forwarded
/// interaction with an [`AgentInteractionOrigin`] — the delegate's name and its
/// [`RunContext`] depth (already `u32`, so no narrowing is needed). A root
/// interaction carries no annotation and maps to the wire default
/// (`delegate: None`, `depth: 0`), keeping the event identical to the
/// pre-attribution shape for the root's own pauses.
fn interaction_origin_to_wire(origin: Option<&AgentInteractionOrigin>) -> InteractionOrigin {
    match origin {
        Some(origin) => InteractionOrigin {
            delegate: Some(origin.delegate.clone()),
            depth: origin.depth,
        },
        None => InteractionOrigin::default(),
    }
}

/// Projects an agent-lib [`InteractionKind`] onto its wire encoding.
fn interaction_kind_to_wire(kind: &InteractionKind) -> InteractionKindWire {
    match kind {
        InteractionKind::Approval {
            call_id,
            requirement,
        } => InteractionKindWire::Approval {
            call_id: tool_call_id_to_wire(*call_id),
            requirement: approval_requirement_to_wire(requirement),
        },
        InteractionKind::Question { prompt } => InteractionKindWire::Question {
            prompt: prompt.clone(),
        },
        InteractionKind::Choice { prompt, options } => InteractionKindWire::Choice {
            prompt: prompt.clone(),
            options: options.clone(),
        },
        InteractionKind::Permission { request } => InteractionKindWire::Permission {
            action_id: request.action_id().to_owned(),
            actor: agent_id_to_wire(request.actor()),
            category: permission_category_to_wire(request.category()),
            risk: permission_risk_to_wire(request.risk()),
            summary: request.summary.clone(),
            subject: request.subject.clone(),
            reason: request.reason().map(str::to_owned),
        },
    }
}

/// Reconstructs an agent-lib [`InteractionResponse`] from the wire `response`,
/// addressing it with the stored `interaction` so the interface never has to
/// echo internal identities.
fn interaction_response_from_wire(
    interaction: &Interaction,
    response: &InteractionResponseWire,
) -> Result<InteractionResponse, ServiceError> {
    let mismatch = || ServiceError::Backend {
        message: "interaction response family does not match the request".to_owned(),
    };

    let core = match (interaction.kind(), response) {
        (
            InteractionKind::Approval { call_id, .. },
            InteractionResponseWire::Approval {
                decision, message, ..
            },
        ) => InteractionResponse::Approval(ApprovalResponse::new(
            interaction.step_id(),
            *call_id,
            approval_decision_from_wire(*decision),
            message.clone(),
        )),
        (InteractionKind::Question { .. }, InteractionResponseWire::Answer { text }) => {
            InteractionResponse::answer(text.clone())
        }
        (InteractionKind::Choice { .. }, InteractionResponseWire::Choice { index }) => {
            InteractionResponse::Choice(*index)
        }
        (
            InteractionKind::Permission { request },
            InteractionResponseWire::Permission { decision, .. },
        ) => InteractionResponse::Permission(PermissionResponse::new(
            request.action_id().to_owned(),
            permission_decision_from_wire(decision),
        )),
        _ => return Err(mismatch()),
    };
    Ok(core)
}

/// Maps an agent-lib [`ApprovalRequirement`] onto its wire encoding.
fn approval_requirement_to_wire(requirement: &ApprovalRequirement) -> ApprovalRequirementWire {
    match requirement {
        ApprovalRequirement::AutoApprove => ApprovalRequirementWire::AutoApprove,
        ApprovalRequirement::RequireApproval { reason } => {
            ApprovalRequirementWire::RequireApproval {
                reason: reason.clone(),
            }
        }
    }
}

/// Maps a wire [`ApprovalDecisionWire`] onto agent-lib's [`ApprovalDecision`].
fn approval_decision_from_wire(decision: ApprovalDecisionWire) -> ApprovalDecision {
    match decision {
        ApprovalDecisionWire::Approve => ApprovalDecision::Approve,
        ApprovalDecisionWire::Deny => ApprovalDecision::Deny,
        ApprovalDecisionWire::Timeout => ApprovalDecision::Timeout,
        ApprovalDecisionWire::Cancel => ApprovalDecision::Cancel,
        // `ApprovalDecisionWire` is `#[non_exhaustive]`; treat any future decision
        // conservatively as a cancel so an unknown wire value never grants a tool.
        _ => ApprovalDecision::Cancel,
    }
}

/// Maps a wire [`PermissionDecisionWire`] onto agent-lib's [`PermissionDecision`].
fn permission_decision_from_wire(decision: &PermissionDecisionWire) -> PermissionDecision {
    match decision {
        PermissionDecisionWire::Approve => PermissionDecision::Approve,
        PermissionDecisionWire::Deny { reason } => PermissionDecision::Deny {
            reason: reason.clone(),
        },
        PermissionDecisionWire::Cancel => PermissionDecision::Cancel,
        // `PermissionDecisionWire` is `#[non_exhaustive]`; treat any future decision
        // conservatively as a cancel so an unknown wire value never grants access.
        _ => PermissionDecision::Cancel,
    }
}

/// Maps an agent-lib [`PermissionCategory`] onto its wire encoding.
fn permission_category_to_wire(category: PermissionCategory) -> PermissionCategoryWire {
    match category {
        PermissionCategory::Shell => PermissionCategoryWire::Shell,
        PermissionCategory::FileRead => PermissionCategoryWire::FileRead,
        PermissionCategory::FileWrite => PermissionCategoryWire::FileWrite,
        PermissionCategory::Network => PermissionCategoryWire::Network,
        PermissionCategory::SpawnAgent => PermissionCategoryWire::SpawnAgent,
        PermissionCategory::Mcp => PermissionCategoryWire::Mcp,
        PermissionCategory::Other => PermissionCategoryWire::Other,
    }
}

/// Maps an agent-lib [`PermissionRisk`] onto its wire encoding.
fn permission_risk_to_wire(risk: PermissionRisk) -> PermissionRiskWire {
    match risk {
        PermissionRisk::Low => PermissionRiskWire::Low,
        PermissionRisk::Medium => PermissionRiskWire::Medium,
        PermissionRisk::High => PermissionRiskWire::High,
        PermissionRisk::Critical => PermissionRiskWire::Critical,
    }
}

/// Re-wraps an agent-lib [`ToolCallId`] as a wire [`ToolCallIdWire`].
fn tool_call_id_to_wire(id: ToolCallId) -> ToolCallIdWire {
    ToolCallIdWire::new(id.into_uuid())
}

/// Re-wraps an agent-lib [`AgentId`] as a wire [`AgentIdWire`].
fn agent_id_to_wire(id: AgentId) -> AgentIdWire {
    AgentIdWire::new(id.into_uuid())
}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
    };

    use agent_lib::{
        agent::{
            AgentId, ApprovalDecision, ApprovalRequirement, BudgetLimits, Interaction,
            InteractionHandler, InteractionOrigin as AgentInteractionOrigin, InteractionResponse,
            PermissionCategory, PermissionDecision, PermissionRequest, PermissionResponse,
            PermissionRisk, RequirementResult, RunContext, RunId, StepId, TraceNodeId,
        },
        client::LlmClient,
        conversation::ToolCallId,
        facade::{Agent, Approval, CancelHandle, Tool, ToolContext},
        model::usage::Usage,
    };
    use async_trait::async_trait;
    use futures::StreamExt;
    use mag_service::{
        ApprovalDecisionWire, ApprovalRequirementWire, InteractionKindWire, InteractionOrigin,
        InteractionResponseWire, PermissionCategoryWire, PermissionDecisionWire,
        PermissionRiskWire, ServiceError, StepIdWire, ToolCallIdWire,
    };
    use serde_json::{Value, json};
    use uuid::Uuid;

    use crate::{
        EventBus,
        test_support::{FakeLlmClient, text_stream_with_usage, tool_use_stream},
    };

    use super::{
        AskFrontendDecider, Event, IpcApproval, PermissionDecider, RequestId, SessionId,
        agent_id_to_wire, interaction_kind_to_wire, interaction_origin_to_wire,
        interaction_response_from_wire,
    };

    fn session_id() -> SessionId {
        SessionId::new(Uuid::from_u128(42))
    }

    fn usage() -> Usage {
        Usage {
            input: 11,
            output: 7,
            total: Some(18),
            ..Usage::default()
        }
    }

    fn run_ctx() -> RunContext {
        RunContext::new_root(
            RunId::new(Uuid::from_u128(1)),
            BudgetLimits::unbounded(),
            TraceNodeId::new("approval-test"),
        )
    }

    fn permission_request() -> PermissionRequest {
        PermissionRequest::new(
            "act-1".to_owned(),
            AgentId::new(Uuid::from_u128(7)),
            PermissionCategory::Shell,
            "run `ls`".to_owned(),
            json!({ "cmd": "ls" }),
            PermissionRisk::Medium,
            Some("listing".to_owned()),
        )
    }

    /// A weather tool that counts how often it actually runs.
    fn counting_tool(counter: Arc<AtomicUsize>) -> Tool {
        Tool::function_with_schema(
            "get_weather",
            "Look up the current weather for a city.",
            json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
            move |_ctx: ToolContext, args: Value| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let city = args.get("city").and_then(Value::as_str).unwrap_or("?");
                    Ok::<_, Infallible>(format!("{city}: sunny, 26C"))
                }
            },
        )
    }

    /// Builds a facade agent that pauses every tool call (`auto_deny`) and routes
    /// the decision through the injected [`IpcApproval`].
    fn approval_agent(counter: Arc<AtomicUsize>, ipc: Arc<IpcApproval>) -> Agent {
        let client: Arc<dyn LlmClient> = FakeLlmClient::scripted(vec![
            tool_use_stream("get_weather", "call-1", json!({ "city": "Paris" })),
            text_stream_with_usage(&["It is sunny."], usage()),
        ]);
        Agent::builder()
            .client(client)
            .model("test-model")
            .max_tokens(64)
            .tool(counting_tool(counter))
            .approval(Approval::auto_deny())
            .interaction_handler(ipc as Arc<dyn InteractionHandler>)
            .build()
            .expect("build approval agent")
    }

    /// Drives `stream` forward until [`IpcApproval`] emits an
    /// [`Event::InteractionRequested`], returning its request id and origin
    /// attribution. Panics if the stream terminates or errors before pausing.
    async fn drive_until_interaction<S>(
        stream: &mut S,
        events: &mut crate::EventStream,
    ) -> (RequestId, InteractionOrigin)
    where
        S: futures::Stream<
                Item = Result<agent_lib::facade::RunEvent, agent_lib::facade::FacadeError>,
            > + Unpin,
    {
        for _ in 0..5000 {
            match futures::poll!(stream.next()) {
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(Some(Err(error))) => panic!("stream error before pause: {error}"),
                Poll::Ready(None) => panic!("stream ended before pausing for an interaction"),
                Poll::Pending => {}
            }
            if let Poll::Ready(Some(event)) = futures::poll!(events.next()) {
                match event {
                    Event::InteractionRequested {
                        request_id, origin, ..
                    } => return (request_id, origin),
                    other => panic!("unexpected event before the interaction: {other:?}"),
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("driver never paused for an interaction");
    }

    /// Drives `stream` to its terminal `None`, panicking on a stream error.
    async fn drain<S>(stream: &mut S)
    where
        S: futures::Stream<
                Item = Result<agent_lib::facade::RunEvent, agent_lib::facade::FacadeError>,
            > + Unpin,
    {
        for _ in 0..5000 {
            match futures::poll!(stream.next()) {
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(Some(Err(error))) => panic!("stream error while draining: {error}"),
                Poll::Ready(None) => return,
                Poll::Pending => tokio::task::yield_now().await,
            }
        }
        panic!("stream never reached a terminal state");
    }

    fn approval_response(decision: ApprovalDecisionWire) -> InteractionResponseWire {
        // `step_id`/`call_id` are reconstructed from the stored request, so any
        // wire values here are ignored: the interface only supplies the decision.
        InteractionResponseWire::Approval {
            step_id: StepIdWire::new(Uuid::nil()),
            call_id: ToolCallIdWire::new(Uuid::nil()),
            decision,
            message: None,
        }
    }

    #[tokio::test]
    async fn pause_then_approve_runs_the_tool() {
        let events = EventBus::new();
        let ipc = Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            Arc::new(AskFrontendDecider),
        ));
        let counter = Arc::new(AtomicUsize::new(0));
        let mut agent = approval_agent(counter.clone(), ipc.clone());
        let mut subscriber = events.subscribe();

        let mut stream = agent.stream("weather?".to_owned()).await.expect("stream");
        let (request_id, root_origin) = drive_until_interaction(&mut stream, &mut subscriber).await;

        // The root session's own interaction carries the default (root) origin
        // attribution (`docs/CLI.md` §3.3).
        assert_eq!(root_origin, InteractionOrigin::default());
        assert!(root_origin.is_root());

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "the gated tool must not run while the interaction is unresolved"
        );
        assert!(
            matches!(
                futures::poll!(futures::StreamExt::next(&mut stream)),
                Poll::Pending
            ),
            "the run stays paused until the interface responds"
        );

        ipc.respond(request_id, approval_response(ApprovalDecisionWire::Approve))
            .expect("respond approve");

        drain(&mut stream).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "an approved gated tool runs exactly once"
        );
    }

    #[tokio::test]
    async fn pause_then_deny_skips_the_tool() {
        let events = EventBus::new();
        let ipc = Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            Arc::new(AskFrontendDecider),
        ));
        let counter = Arc::new(AtomicUsize::new(0));
        let mut agent = approval_agent(counter.clone(), ipc.clone());
        let mut subscriber = events.subscribe();

        let mut stream = agent.stream("weather?".to_owned()).await.expect("stream");
        let (request_id, _) = drive_until_interaction(&mut stream, &mut subscriber).await;

        ipc.respond(request_id, approval_response(ApprovalDecisionWire::Deny))
            .expect("respond deny");

        drain(&mut stream).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "a denied gated tool never executes; the denial is fed back to the model"
        );
    }

    #[tokio::test]
    async fn cancel_while_paused_resolves_without_running_the_tool() {
        let events = EventBus::new();
        let ipc = Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            Arc::new(AskFrontendDecider),
        ));
        let counter = Arc::new(AtomicUsize::new(0));
        let mut agent = approval_agent(counter.clone(), ipc.clone());
        let mut subscriber = events.subscribe();

        let cancel = CancelHandle::new();
        let mut stream = agent
            .stream_with_cancel("weather?".to_owned(), cancel.clone())
            .await
            .expect("stream");
        let (_request_id, _) = drive_until_interaction(&mut stream, &mut subscriber).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // Cancelling the active run must unblock the parked approval (the
        // facade propagates the cancel token through the fulfill `RunContext`)
        // and resolve it conservatively (the tool never runs).
        cancel.cancel();

        // The facade ends a cancelled run with a stream error rather than a
        // terminal `Done`.
        let mut saw_cancel_error = false;
        for _ in 0..5000 {
            match futures::poll!(futures::StreamExt::next(&mut stream)) {
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(Some(Err(_))) => {
                    saw_cancel_error = true;
                    break;
                }
                Poll::Ready(None) => break,
                Poll::Pending => tokio::task::yield_now().await,
            }
        }
        assert!(saw_cancel_error, "a cancelled run ends with a stream error");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "a cancelled approval never executes the gated tool"
        );
        // The pending entry is discarded, so a late response finds nothing.
        assert_eq!(
            ipc.respond(
                _request_id,
                approval_response(ApprovalDecisionWire::Approve)
            ),
            Err(ServiceError::InteractionNotFound {
                request_id: _request_id,
            })
        );
    }

    #[tokio::test]
    async fn permission_uses_default_decider_to_emit_and_await() {
        let events = EventBus::new();
        let ipc = IpcApproval::new(session_id(), events.clone(), Arc::new(AskFrontendDecider));
        let mut subscriber = events.subscribe();
        let interaction =
            Interaction::permission(StepId::new(Uuid::from_u128(5)), permission_request());
        let ctx = run_ctx();

        let mut fulfilled = Box::pin(ipc.fulfill(&interaction, &ctx));

        // The default decider defers to the interface: the handler must emit an
        // `InteractionRequested` and then park until the interface responds.
        let request_id = loop {
            assert!(
                futures::poll!(fulfilled.as_mut()).is_pending(),
                "fulfill resolved before the interface responded"
            );
            if let Poll::Ready(Some(event)) = futures::poll!(subscriber.next()) {
                match event {
                    Event::InteractionRequested {
                        request_id,
                        kind: InteractionKindWire::Permission { action_id, .. },
                        ..
                    } => {
                        assert_eq!(action_id, "act-1");
                        break request_id;
                    }
                    other => panic!("expected a permission InteractionRequested, got {other:?}"),
                }
            }
            tokio::task::yield_now().await;
        };

        ipc.respond(
            request_id,
            InteractionResponseWire::Permission {
                action_id: "act-1".to_owned(),
                decision: PermissionDecisionWire::Approve,
            },
        )
        .expect("respond permission");

        match fulfilled.await {
            RequirementResult::Interaction(InteractionResponse::Permission(response)) => {
                assert_eq!(response.decision(), &PermissionDecision::Approve);
            }
            other => panic!("expected a permission interaction result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn custom_decider_short_circuits_without_emitting() {
        struct AllowDecider;
        #[async_trait]
        impl PermissionDecider for AllowDecider {
            async fn decide(&self, request: &PermissionRequest) -> Option<PermissionResponse> {
                Some(PermissionResponse::approve(request.action_id().to_owned()))
            }
        }

        let events = EventBus::new();
        let ipc = IpcApproval::new(session_id(), events.clone(), Arc::new(AllowDecider));
        let mut subscriber = events.subscribe();
        let interaction =
            Interaction::permission(StepId::new(Uuid::from_u128(5)), permission_request());
        let ctx = run_ctx();

        match ipc.fulfill(&interaction, &ctx).await {
            RequirementResult::Interaction(InteractionResponse::Permission(response)) => {
                assert_eq!(response.decision(), &PermissionDecision::Approve);
            }
            other => panic!("expected a decided permission result, got {other:?}"),
        }

        assert!(
            matches!(futures::poll!(subscriber.next()), Poll::Pending),
            "a decided permission must not emit an InteractionRequested"
        );
    }

    #[test]
    fn approval_kind_round_trips_through_wire() {
        let step = StepId::new(Uuid::from_u128(9));
        let call = ToolCallId::new(Uuid::from_u128(3));
        let interaction = Interaction::approval(
            step,
            call,
            ApprovalRequirement::RequireApproval {
                reason: Some("gated".to_owned()),
            },
        );

        assert_eq!(
            interaction_kind_to_wire(interaction.kind()),
            InteractionKindWire::Approval {
                call_id: ToolCallIdWire::new(call.into_uuid()),
                requirement: ApprovalRequirementWire::RequireApproval {
                    reason: Some("gated".to_owned()),
                },
            }
        );

        let core = interaction_response_from_wire(
            &interaction,
            &InteractionResponseWire::Approval {
                step_id: StepIdWire::new(Uuid::nil()),
                call_id: ToolCallIdWire::new(Uuid::nil()),
                decision: ApprovalDecisionWire::Deny,
                message: Some("no".to_owned()),
            },
        )
        .expect("reconstruct approval response");
        match core {
            InteractionResponse::Approval(response) => {
                assert_eq!(response.step_id(), step);
                assert_eq!(response.call_id(), call);
                assert_eq!(response.decision(), ApprovalDecision::Deny);
            }
            other => panic!("expected an approval response, got {other:?}"),
        }
    }

    #[test]
    fn question_kind_round_trips_through_wire() {
        let interaction =
            Interaction::question(StepId::new(Uuid::from_u128(9)), "How now?".to_owned());

        assert_eq!(
            interaction_kind_to_wire(interaction.kind()),
            InteractionKindWire::Question {
                prompt: "How now?".to_owned(),
            }
        );

        let core = interaction_response_from_wire(
            &interaction,
            &InteractionResponseWire::Answer {
                text: "brown cow".to_owned(),
            },
        )
        .expect("reconstruct answer");
        assert_eq!(core, InteractionResponse::answer("brown cow".to_owned()));
    }

    #[test]
    fn choice_kind_round_trips_through_wire() {
        let interaction = Interaction::choice(
            StepId::new(Uuid::from_u128(9)),
            "pick".to_owned(),
            vec!["a".to_owned(), "b".to_owned()],
        );

        assert_eq!(
            interaction_kind_to_wire(interaction.kind()),
            InteractionKindWire::Choice {
                prompt: "pick".to_owned(),
                options: vec!["a".to_owned(), "b".to_owned()],
            }
        );

        let core = interaction_response_from_wire(
            &interaction,
            &InteractionResponseWire::Choice { index: 1 },
        )
        .expect("reconstruct choice");
        assert_eq!(core, InteractionResponse::Choice(1));
    }

    #[test]
    fn permission_kind_round_trips_through_wire() {
        let request = permission_request();
        let interaction = Interaction::permission(StepId::new(Uuid::from_u128(9)), request.clone());

        assert_eq!(
            interaction_kind_to_wire(interaction.kind()),
            InteractionKindWire::Permission {
                action_id: "act-1".to_owned(),
                actor: agent_id_to_wire(request.actor()),
                category: PermissionCategoryWire::Shell,
                risk: PermissionRiskWire::Medium,
                summary: "run `ls`".to_owned(),
                subject: json!({ "cmd": "ls" }),
                reason: Some("listing".to_owned()),
            }
        );

        let core = interaction_response_from_wire(
            &interaction,
            &InteractionResponseWire::Permission {
                action_id: "act-1".to_owned(),
                decision: PermissionDecisionWire::Deny {
                    reason: Some("nope".to_owned()),
                },
            },
        )
        .expect("reconstruct permission response");
        match core {
            InteractionResponse::Permission(response) => {
                assert_eq!(response.action_id(), "act-1");
                assert_eq!(
                    response.decision(),
                    &PermissionDecision::Deny {
                        reason: Some("nope".to_owned()),
                    }
                );
            }
            other => panic!("expected a permission response, got {other:?}"),
        }
    }

    #[test]
    fn mismatched_response_family_is_rejected() {
        let interaction =
            Interaction::question(StepId::new(Uuid::from_u128(9)), "How now?".to_owned());
        let error = interaction_response_from_wire(
            &interaction,
            &InteractionResponseWire::Choice { index: 0 },
        )
        .expect_err("family mismatch must be rejected");
        assert!(matches!(error, ServiceError::Backend { .. }));
    }

    #[tokio::test]
    async fn invalid_response_keeps_pending_interaction_retryable() {
        let events = EventBus::new();
        let ipc = IpcApproval::new(session_id(), events.clone(), Arc::new(AskFrontendDecider));
        let mut subscriber = events.subscribe();
        let interaction =
            Interaction::question(StepId::new(Uuid::from_u128(9)), "How now?".to_owned());
        let ctx = run_ctx();
        let mut fulfilled = Box::pin(ipc.fulfill(&interaction, &ctx));

        let request_id = loop {
            assert!(
                futures::poll!(fulfilled.as_mut()).is_pending(),
                "fulfill resolved before the interface responded"
            );
            if let Poll::Ready(Some(Event::InteractionRequested { request_id, .. })) =
                futures::poll!(subscriber.next())
            {
                break request_id;
            }
            tokio::task::yield_now().await;
        };

        assert!(matches!(
            ipc.respond(request_id, InteractionResponseWire::Choice { index: 0 }),
            Err(ServiceError::Backend { .. })
        ));
        assert!(
            futures::poll!(fulfilled.as_mut()).is_pending(),
            "invalid response must not drop the pending waiter"
        );

        ipc.respond(
            request_id,
            InteractionResponseWire::Answer {
                text: "retry answer".to_owned(),
            },
        )
        .expect("retry with matching answer");

        match fulfilled.await {
            RequirementResult::Interaction(InteractionResponse::Answer(answer)) => {
                assert_eq!(answer, "retry answer");
            }
            other => panic!("expected answer interaction result, got {other:?}"),
        }
    }

    #[test]
    fn respond_unknown_request_id_reports_interaction_not_found() {
        let events = EventBus::new();
        let ipc = IpcApproval::new(session_id(), events, Arc::new(AskFrontendDecider));
        let request_id = RequestId::new(Uuid::from_u128(1234));

        assert_eq!(
            ipc.respond(
                request_id,
                InteractionResponseWire::Answer {
                    text: "ok".to_owned(),
                },
            ),
            Err(ServiceError::InteractionNotFound { request_id })
        );
    }

    #[test]
    fn interaction_origin_maps_to_wire() {
        // No attribution = the root session's own interaction.
        assert_eq!(
            interaction_origin_to_wire(None),
            InteractionOrigin::default()
        );

        let delegated = AgentInteractionOrigin::new("codex", 2);
        assert_eq!(
            interaction_origin_to_wire(Some(&delegated)),
            InteractionOrigin {
                delegate: Some("codex".to_owned()),
                depth: 2,
            }
        );
    }

    #[tokio::test]
    async fn delegated_interaction_origin_surfaces_on_the_wire_event() {
        let events = EventBus::new();
        let ipc = IpcApproval::new(session_id(), events.clone(), Arc::new(AskFrontendDecider));
        let mut subscriber = events.subscribe();
        // agent-lib's delegation routing annotates the forwarded interaction;
        // simulate that annotation directly (`docs/CLI.md` §3.3).
        let interaction =
            Interaction::question(StepId::new(Uuid::from_u128(5)), "delegate asks".to_owned())
                .with_origin(AgentInteractionOrigin::new("reviewer", 1));
        let ctx = run_ctx();

        let mut fulfilled = Box::pin(ipc.fulfill(&interaction, &ctx));

        let request_id = loop {
            assert!(
                futures::poll!(fulfilled.as_mut()).is_pending(),
                "fulfill resolved before the interface responded"
            );
            if let Poll::Ready(Some(event)) = futures::poll!(subscriber.next()) {
                match event {
                    Event::InteractionRequested {
                        request_id,
                        kind: InteractionKindWire::Question { prompt },
                        origin,
                        ..
                    } => {
                        assert_eq!(prompt, "delegate asks");
                        assert_eq!(origin.delegate.as_deref(), Some("reviewer"));
                        assert_eq!(origin.depth, 1);
                        assert!(!origin.is_root());
                        break request_id;
                    }
                    other => panic!("expected a delegated InteractionRequested, got {other:?}"),
                }
            }
            tokio::task::yield_now().await;
        };

        ipc.respond(
            request_id,
            InteractionResponseWire::Answer {
                text: "from the interface".to_owned(),
            },
        )
        .expect("respond answer");

        match fulfilled.await {
            RequirementResult::Interaction(response) => {
                assert_eq!(
                    response,
                    InteractionResponse::answer("from the interface".to_owned())
                );
            }
            other => panic!("expected an interaction result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_delegate_interactions_route_each_response_to_its_own_waiter() {
        let events = EventBus::new();
        let ipc = Arc::new(IpcApproval::new(
            session_id(),
            events.clone(),
            Arc::new(AskFrontendDecider),
        ));
        let mut subscriber = events.subscribe();
        let ctx = run_ctx();

        // Two delegates park concurrently on the same session handler
        // (`docs/CLI.md` §3.3: one handler mints every request id of the
        // session, so concurrent delegates can never collide).
        let interaction_a = Interaction::approval(
            StepId::new(Uuid::from_u128(11)),
            ToolCallId::new(Uuid::from_u128(21)),
            ApprovalRequirement::RequireApproval { reason: None },
        )
        .with_origin(AgentInteractionOrigin::new("delegate-a", 1));
        let interaction_b = Interaction::approval(
            StepId::new(Uuid::from_u128(12)),
            ToolCallId::new(Uuid::from_u128(22)),
            ApprovalRequirement::RequireApproval { reason: None },
        )
        .with_origin(AgentInteractionOrigin::new("delegate-b", 2));

        let mut fulfill_a = Box::pin(ipc.fulfill(&interaction_a, &ctx));
        let mut fulfill_b = Box::pin(ipc.fulfill(&interaction_b, &ctx));

        // Collect both requests: distinct ids, each carrying its own origin.
        let mut requests = std::collections::HashMap::new();
        for _ in 0..5000 {
            let _ = futures::poll!(fulfill_a.as_mut());
            let _ = futures::poll!(fulfill_b.as_mut());
            while let Poll::Ready(Some(event)) = futures::poll!(subscriber.next()) {
                match event {
                    Event::InteractionRequested {
                        request_id, origin, ..
                    } => {
                        requests.insert(request_id, origin);
                    }
                    other => panic!("unexpected event before both pauses: {other:?}"),
                }
            }
            if requests.len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let id_of = |delegate: &str| {
            *requests
                .iter()
                .find(|(_, origin)| origin.delegate.as_deref() == Some(delegate))
                .unwrap_or_else(|| panic!("no parked request from {delegate}"))
                .0
        };
        let id_a = id_of("delegate-a");
        let id_b = id_of("delegate-b");
        assert_ne!(id_a, id_b, "concurrent requests get distinct ids");

        // Respond out of order: each parked delegate must receive its own
        // decision — responses never cross over between delegates.
        ipc.respond(id_b, approval_response(ApprovalDecisionWire::Deny))
            .expect("respond delegate-b");
        match fulfill_b.await {
            RequirementResult::Interaction(InteractionResponse::Approval(response)) => {
                assert_eq!(response.decision(), ApprovalDecision::Deny);
                assert_eq!(response.step_id(), StepId::new(Uuid::from_u128(12)));
                assert_eq!(response.call_id(), ToolCallId::new(Uuid::from_u128(22)));
            }
            other => panic!("expected delegate-b's approval result, got {other:?}"),
        }

        ipc.respond(id_a, approval_response(ApprovalDecisionWire::Approve))
            .expect("respond delegate-a");
        match fulfill_a.await {
            RequirementResult::Interaction(InteractionResponse::Approval(response)) => {
                assert_eq!(response.decision(), ApprovalDecision::Approve);
                assert_eq!(response.step_id(), StepId::new(Uuid::from_u128(11)));
                assert_eq!(response.call_id(), ToolCallId::new(Uuid::from_u128(21)));
            }
            other => panic!("expected delegate-a's approval result, got {other:?}"),
        }
    }
}
