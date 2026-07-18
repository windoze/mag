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

use std::convert::Infallible;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use agent_lib::{
    agent::InteractionHandler,
    client::LlmClient,
    facade::{
        Agent, ApprovalPolicy, FacadeError, Tool, ToolContext, ToolResult,
        ToolTrace as FacadeToolTrace, UsageSummary, WireRunEvent, WireRunOutput,
    },
};
use mag_service::{
    Event, RunId as WireRunId, RunOutput, SessionConfig, SessionId, ToolCallIdWire, ToolStatusWire,
    ToolTrace, UsageInfo,
};
use mag_tools::{ToolPlugin, ToolRegistry};
use serde_json::Value;
use uuid::Uuid;

use crate::{EventBus, engine::approval::IpcApproval, session::CancelToken};

const DEFAULT_MAX_TOKENS: u32 = 512;
const DEFAULT_MAX_STEPS: u32 = 8;

/// One session's stateful facade [`Agent`] plus a run-id source.
///
/// The facade [`Agent`] holds the session's conversation, so reusing one driver
/// across turns accumulates history without mag reassembling any state.
#[derive(Debug)]
pub(crate) struct SessionDriver {
    agent: Agent,
    run_counter: AtomicU64,
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
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while assembling the agent (for
    /// example an invalid model/provider configuration).
    pub(crate) fn new(
        config: &SessionConfig,
        client: Arc<dyn LlmClient>,
        tools: &ToolRegistry,
        approval: Arc<IpcApproval>,
    ) -> Result<Self, FacadeError> {
        let mut builder = Agent::builder()
            .client(client)
            .model(config.model.clone())
            .max_tokens(DEFAULT_MAX_TOKENS)
            .max_steps(DEFAULT_MAX_STEPS)
            .interaction_handler(approval as Arc<dyn InteractionHandler>);

        // Register each tool and gate the ones carrying a permission spec: a
        // gated tool pauses through the injected `IpcApproval`, while a
        // permission-free tool (read-only, `permission() == None`) stays on the
        // default auto-allow tier and runs without interrupting the user.
        let mut policy = ApprovalPolicy::default();
        for plugin in tools.plugins() {
            if plugin.permission().is_some() {
                policy = policy.ask_tool(plugin.name());
            }
            builder = builder.tool(facade_tool(Arc::clone(plugin)));
        }

        let agent = builder.approval(policy).build()?;

        Ok(Self {
            agent,
            run_counter: AtomicU64::new(1),
        })
    }

    /// Drives one user message through the facade [`Agent`] under a
    /// [`CancelToken`], emitting the run's streamed and terminal events.
    ///
    /// The caller (the session actor) mints the run identity and emits
    /// [`RunStarted`](Event::RunStarted) before invoking this; `run_turn` streams
    /// the turn: each text delta becomes an [`Event::TextDelta`] and the run ends
    /// with exactly one terminal event:
    ///
    /// - [`Event::RunFinished`] when the facade stream reaches its terminal
    ///   `Done`.
    /// - [`Event::RunError`] when the facade stream yields a failure, or ends
    ///   without a terminal `Done`.
    /// - [`Event::RunError`] carrying `"run cancelled"` when `cancel` fires while
    ///   the turn is in flight.
    ///
    /// Cancellation and failure both drop the facade stream before the terminal
    /// event is emitted; agent-lib abandons the in-flight turn on drop, so the
    /// agent's committed history is unchanged and the driver stays reusable for
    /// the next turn.
    pub(crate) async fn run_turn(
        &mut self,
        session_id: SessionId,
        text: String,
        events: &EventBus,
        cancel: &CancelToken,
    ) {
        let mut stream = match self.agent.stream(text).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = events.emit(Event::RunError {
                    id: session_id,
                    message: error.to_string(),
                });
                return;
            }
        };

        let mut final_output: Option<RunOutput> = None;
        let outcome = loop {
            tokio::select! {
                item = stream.next() => match item {
                    Some(Ok(event)) => {
                        if let Some(mag_event) =
                            map_wire_event(session_id, event.to_wire(), &mut final_output)
                        {
                            let _ = events.emit(mag_event);
                        }
                    }
                    Some(Err(error)) => break TurnOutcome::Failed(error.to_string()),
                    None => break TurnOutcome::Completed,
                },
                () = cancel.cancelled() => break TurnOutcome::Cancelled,
            }
        };

        // Release the mutable agent borrow before emitting the terminal event; a
        // cancelled or failed turn is abandoned on drop (committed history is
        // left intact) so the next `run_turn` on this driver can proceed.
        drop(stream);

        let terminal = match outcome {
            TurnOutcome::Completed => match final_output {
                Some(output) => Event::RunFinished {
                    id: session_id,
                    output,
                },
                None => Event::RunError {
                    id: session_id,
                    message: "agent stream ended without a terminal `Done` event".to_owned(),
                },
            },
            TurnOutcome::Failed(message) => Event::RunError {
                id: session_id,
                message,
            },
            TurnOutcome::Cancelled => Event::RunError {
                id: session_id,
                message: "run cancelled".to_owned(),
            },
        };
        let _ = events.emit(terminal);
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
enum TurnOutcome {
    /// The facade stream reached its terminal `Done`.
    Completed,
    /// The facade stream yielded a failure carrying this message.
    Failed(String),
    /// The [`CancelToken`] fired while the turn was in flight.
    Cancelled,
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
}
