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

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use agent_lib::{
    client::LlmClient,
    facade::{Agent, FacadeError, UsageSummary, WireRunEvent, WireRunOutput},
};
use mag_service::{Event, RunId as WireRunId, RunOutput, SessionConfig, SessionId, UsageInfo};
use uuid::Uuid;

use crate::{EventBus, session::CancelToken};

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
    /// # Errors
    ///
    /// Returns any [`FacadeError`] raised while assembling the agent (for
    /// example an invalid model/provider configuration).
    pub(crate) fn new(
        config: &SessionConfig,
        client: Arc<dyn LlmClient>,
    ) -> Result<Self, FacadeError> {
        let agent = Agent::builder()
            .client(client)
            .model(config.model.clone())
            .max_tokens(DEFAULT_MAX_TOKENS)
            .max_steps(DEFAULT_MAX_STEPS)
            .build()?;

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
/// the stream drains). Tool, approval, delegation, and raw variants are produced
/// only by later milestones (C3+); the pure-conversation path never yields them,
/// so they are ignored here.
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
        WireRunEvent::Done(output) => {
            *final_output = Some(run_output_from_wire(&output));
            None
        }
        _ => None,
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
    use mag_service::{Event, SessionId, UsageInfo};
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
}
