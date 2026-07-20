//! Per-session driver actors and the manager that owns them.
//!
//! `docs/DESIGN.md` §3.1 mandates **one driver actor per session**: each actor
//! exclusively owns its session's facade [`Agent`](agent_lib::facade::Agent)
//! (through a [`SessionDriver`]) and serializes that session's commands, which
//! sidesteps agent-lib's `&mut self` run surface. Control commands
//! ([`SessionCommand::CancelRun`] / [`SessionCommand::RespondInteraction`]) must
//! travel a side-channel that never borrows the agent mutably, so an in-flight
//! run can never starve them.
//!
//! This module implements that model with **one dedicated OS thread per
//! session**, each hosting a `current_thread` runtime plus a
//! [`LocalSet`] (agent-lib's facade run stream is not `Send`, so a run is driven
//! by a `spawn_local` task pinned to the session's thread). Cancellation flows
//! through agent-lib's [`CancelHandle`]: the actor cancels the handle and the
//! facade cooperatively abandons the in-flight turn with bounded latency,
//! leaving the machine parked on `Idle` with committed history intact.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    thread,
};

use agent_lib::{
    client::LlmClient,
    facade::{AgentSnapshot, CancelHandle, FacadeError},
};
use mag_service::{
    Event, InteractionResponseWire, RequestId, RunId, ServiceError, SessionConfig, SessionId,
};
use mag_tools::ToolRegistry;
use tokio::{
    runtime::Builder,
    sync::{mpsc, oneshot},
    task::LocalSet,
};

use crate::{
    EventBus,
    driver::{PivotQueue, SessionDriver, TurnOutcome},
    engine::{
        ConfigApplyState,
        approval::{AskFrontendDecider, IpcApproval},
    },
    persistence::Persistence,
    turn_complete::TurnCompleteHub,
};

/// Locks `mutex`, recovering the guard from a poisoned lock instead of
/// panicking.
///
/// Mirrors agent-lib's unified poison-recovery policy (M9-1): a panicking
/// neighbour task must not cascade into every session actor through a poisoned
/// standard-library mutex.
fn lock_recovering<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A command delivered to a single session's actor.
enum SessionCommand {
    /// Start a run for one user message; replies the started run identity or a
    /// backend error when the session's driver failed to build.
    SendMessage {
        /// User-visible message text.
        text: String,
        /// Channel used to report the started run identity or a start failure.
        reply: oneshot::Sender<Result<RunId, ServiceError>>,
    },
    /// Cancel the session's active run, if any (cancel-handle side-channel).
    CancelRun,
    /// Queue a pivot message for the session's in-flight run (pivot-queue
    /// side-channel, `docs/CLI.md` §3.2); replies `NotPivotable` when no run
    /// is in progress.
    PivotMessage {
        /// User-visible pivot text.
        text: String,
        /// Channel used to report the queueing or the `NotPivotable` rejection.
        reply: oneshot::Sender<Result<(), ServiceError>>,
    },
    /// Resolve a pending interaction request, waking the parked driver.
    RespondInteraction {
        /// Request identity emitted by [`Event::InteractionRequested`].
        request_id: RequestId,
        /// Interface-supplied response to the pending interaction.
        response: InteractionResponseWire,
        /// Channel used to acknowledge delivery or report a failure.
        reply: oneshot::Sender<Result<(), ServiceError>>,
    },
    /// Ask the actor to apply the current configuration snapshot when the
    /// agent is at rest (`docs/CLI.md` §4.4): immediately when idle, or at the
    /// next run terminal — the run-completion path re-checks the shared
    /// generation counter, so a command that arrives mid-run is deferred
    /// rather than lost. Carries the generation the request was sent for, so
    /// an actor whose thread started after the bump still honors it.
    ApplyConfig {
        /// Apply-request generation this command was sent for.
        generation: u64,
    },
}

/// Lifecycle state of a session actor's driver.
enum DriverState {
    /// The driver is idle and ready to start the next run.
    ///
    /// Boxed because the facade [`Agent`](agent_lib::facade::Agent) it owns is
    /// far larger than the other variants.
    Idle(Box<SessionDriver>),
    /// A run is in flight; the driver was moved into the run task.
    Running,
    /// The driver failed to build; every run start reports this backend error.
    Failed(String),
}

/// One session's actor: owns the driver and serializes the session's commands.
struct SessionActor {
    session_id: SessionId,
    events: EventBus,
    state: DriverState,
    /// Shared approval handler: injected into the driver's agent and used here to
    /// resolve `RespondInteraction` commands (`docs/DESIGN.md` §3.3).
    approval: Arc<IpcApproval>,
    /// Durable store: each committed run writes its snapshot here (`docs/DESIGN.md`
    /// §3.6).
    store: Arc<Persistence>,
    /// Cancel handle for the active run, present only while a run is in flight.
    cancel: Option<CancelHandle>,
    /// Pivot queue for the active run, present only while a run is in flight
    /// (`docs/CLI.md` §3.2); its presence is the actor's in-progress marker.
    pivots: Option<PivotQueue>,
    /// Commands received while a run was active, replayed once it finishes.
    deferred: VecDeque<SessionCommand>,
    /// Sender the run task uses to hand the driver back on completion.
    run_done_tx: mpsc::UnboundedSender<(Box<SessionDriver>, TurnOutcome)>,
    /// Receiver that reclaims the driver once a run task finishes.
    run_done_rx: mpsc::UnboundedReceiver<(Box<SessionDriver>, TurnOutcome)>,
    /// Shared config-apply plumbing (`docs/CLI.md` §4.4); `None` when the
    /// engine has no configuration backend.
    config_apply: Option<ConfigApplyState>,
    /// Generation of the last config apply this actor performed. Starts at the
    /// generation current when the actor spawns: a freshly created session is
    /// assembled from the caller's own `SessionConfig` and counts as
    /// up-to-date (the session↔snapshot binding becomes explicit with
    /// `Engine::from_config`, M3-6).
    applied_generation: u64,
}

impl SessionActor {
    /// Builds an actor from a freshly created (or failed) driver.
    ///
    /// `applied_generation` is the config-apply generation read synchronously
    /// when the actor was spawned (`docs/CLI.md` §4.4): a session created
    /// after an `apply_config` counts as up-to-date, while a session whose
    /// spawn raced the bump still honors the `ApplyConfig` command carrying
    /// that generation.
    fn new(
        session_id: SessionId,
        events: EventBus,
        driver: Result<SessionDriver, FacadeError>,
        approval: Arc<IpcApproval>,
        store: Arc<Persistence>,
        config_apply: Option<ConfigApplyState>,
        applied_generation: u64,
    ) -> Self {
        let (run_done_tx, run_done_rx) = mpsc::unbounded_channel();
        let state = match driver {
            Ok(driver) => DriverState::Idle(Box::new(driver)),
            Err(error) => DriverState::Failed(error.to_string()),
        };
        Self {
            session_id,
            events,
            state,
            approval,
            store,
            cancel: None,
            pivots: None,
            deferred: VecDeque::new(),
            run_done_tx,
            run_done_rx,
            config_apply,
            applied_generation,
        }
    }

    /// Runs the actor loop until its command channel closes.
    ///
    /// The loop interleaves fresh commands with the reclaimed driver from a
    /// finished run. Control commands (`CancelRun`) are handled even while a run
    /// is in flight because the run advances on a separate `spawn_local` task, so
    /// this loop is always free to service the channel.
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        loop {
            // Replay a deferred command as soon as the driver is idle again.
            if let Some(command) = self.take_idle_deferred() {
                self.handle(command);
                continue;
            }

            tokio::select! {
                maybe_command = commands.recv() => match maybe_command {
                    Some(command) => self.handle(command),
                    None => break,
                },
                Some((driver, outcome)) = self.run_done_rx.recv() => {
                    // A pivot that raced the run's end — queued after the
                    // driver's own final drain but before the actor observed
                    // the run finishing — is dropped here with the same
                    // terminal reason (`docs/CLI.md` §3.2).
                    if let Some(pivots) = self.pivots.take() {
                        let reason = outcome.pivot_drop_reason();
                        for _ in pivots.drain() {
                            let _ = self.events.emit(Event::PivotDropped {
                                id: self.session_id,
                                reason: reason.to_owned(),
                            });
                        }
                    }
                    self.state = DriverState::Idle(driver);
                    self.cancel = None;
                    // Turn boundary: land a pending config apply *before* any
                    // deferred command starts the next run (`docs/CLI.md`
                    // §4.4: the actor checks between runs).
                    self.apply_pending_config();
                }
            }
        }
    }

    /// Pops the next deferred command, but only while the driver is idle.
    fn take_idle_deferred(&mut self) -> Option<SessionCommand> {
        if matches!(self.state, DriverState::Idle(_)) {
            self.deferred.pop_front()
        } else {
            None
        }
    }

    /// Dispatches one command against the current driver state.
    fn handle(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::SendMessage { text, reply } => self.start_run(text, reply),
            SessionCommand::CancelRun => {
                if let Some(cancel) = &self.cancel {
                    cancel.cancel();
                }
            }
            SessionCommand::PivotMessage { text, reply } => {
                let result = match &self.pivots {
                    Some(pivots) => {
                        pivots.push(text);
                        // Announce the queueing before replying, so a
                        // subscriber always observes `PivotQueued` once
                        // `pivot_message` has returned `Ok`.
                        let _ = self.events.emit(Event::PivotQueued {
                            id: self.session_id,
                        });
                        Ok(())
                    }
                    None => Err(ServiceError::NotPivotable {
                        id: self.session_id,
                        reason: "no in-progress run".to_owned(),
                    }),
                };
                let _ = reply.send(result);
            }
            SessionCommand::RespondInteraction {
                request_id,
                response,
                reply,
            } => {
                let _ = reply.send(self.approval.respond(request_id, response));
            }
            SessionCommand::ApplyConfig { generation } => self.apply_config_for(generation),
        }
    }

    /// Applies the current configuration snapshot when the shared generation
    /// moved past this actor's last apply and the driver is at rest
    /// (`docs/CLI.md` §4.4, decision D2).
    ///
    /// A no-op when no apply is owed (the generation did not move), when the
    /// engine has no configuration backend, or when a run is in flight — in
    /// the running case the run-completion path re-checks the generation at
    /// the turn boundary, so the apply is deferred rather than lost
    /// (agent-lib only admits reconfiguration between runs).
    fn apply_pending_config(&mut self) {
        let pending = self
            .config_apply
            .as_ref()
            .map_or(self.applied_generation, ConfigApplyState::pending);
        self.apply_config_for(pending);
    }

    /// Lands the current configuration snapshot on the idle driver when
    /// `generation` is newer than this actor's last apply (`docs/CLI.md`
    /// §4.4, decision D2).
    ///
    /// A no-op when the generation was already applied, when the engine has
    /// no configuration backend, or when a run is in flight — in the running
    /// case the run-completion path re-checks the pending generation at the
    /// turn boundary, so the apply is deferred rather than lost (agent-lib
    /// only admits reconfiguration between runs).
    fn apply_config_for(&mut self, generation: u64) {
        if generation <= self.applied_generation {
            return;
        }
        let Some(config_apply) = &self.config_apply else {
            return;
        };
        let DriverState::Idle(driver) = &mut self.state else {
            return;
        };
        let snapshot = config_apply.service().current();
        driver.apply_config(self.session_id, &snapshot);
        // The generation is marked applied even when individual items were
        // rejected: a rejection is permanent for this snapshot (warned and
        // skipped inside the driver), never a retryable failure.
        self.applied_generation = generation;
    }

    /// Starts a run for `text`, deferring or rejecting when a run cannot start.
    fn start_run(&mut self, text: String, reply: oneshot::Sender<Result<RunId, ServiceError>>) {
        match &self.state {
            DriverState::Failed(message) => {
                let _ = reply.send(Err(ServiceError::Backend {
                    message: message.clone(),
                }));
                return;
            }
            DriverState::Running => {
                // A session's runs are serial: defer until the active run ends.
                self.deferred
                    .push_back(SessionCommand::SendMessage { text, reply });
                return;
            }
            DriverState::Idle(_) => {}
        }

        let DriverState::Idle(mut driver) =
            std::mem::replace(&mut self.state, DriverState::Running)
        else {
            unreachable!("driver is idle in this branch");
        };

        let run_id = driver.next_run_id();
        let _ = self.events.emit(Event::RunStarted {
            id: self.session_id,
            run_id,
        });
        // Reply the run identity before the run drives so the caller can cancel
        // an in-flight run rather than block until it finishes.
        let _ = reply.send(Ok(run_id));

        let cancel = CancelHandle::new();
        self.cancel = Some(cancel.clone());
        let pivots = PivotQueue::new();
        self.pivots = Some(pivots.clone());
        let events = self.events.clone();
        let store = self.store.clone();
        let run_done = self.run_done_tx.clone();
        let session_id = self.session_id;
        tokio::task::spawn_local(async move {
            let outcome = driver
                .run_turn(session_id, text, &events, &cancel, &pivots, &store)
                .await;
            // Hand the driver back so the session can start its next run.
            let _ = run_done.send((driver, outcome));
        });
    }
}

/// Body of a session's dedicated thread: a `current_thread` runtime plus a
/// [`LocalSet`] that drives the actor and its (non-`Send`) run tasks.
///
/// When `restore` is `Some`, the session's facade agent is rebuilt from the
/// persisted [`AgentSnapshot`] (re-injecting the client, tools, and approval
/// handler a snapshot deliberately omits); when `None`, a fresh agent is built
/// from `config` (`docs/DESIGN.md` §3.6).
#[allow(clippy::too_many_arguments)]
fn session_thread(
    session_id: SessionId,
    config: SessionConfig,
    client: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
    event_bus: EventBus,
    store: Arc<Persistence>,
    restore: Option<AgentSnapshot>,
    config_apply: Option<ConfigApplyState>,
    applied_generation: u64,
    turn_complete: TurnCompleteHub,
    commands: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build mag session runtime");
    let local = LocalSet::new();
    // Shared approval handler bridges the driver's paused interactions to the
    // event bus and back through `RespondInteraction` (`docs/DESIGN.md` §3.3).
    let approval = Arc::new(IpcApproval::new(
        session_id,
        event_bus.clone(),
        Arc::new(AskFrontendDecider),
    ));
    let driver = match restore {
        Some(snapshot) => SessionDriver::restore(
            client,
            tools,
            approval.clone(),
            snapshot,
            config.budget.as_ref(),
            turn_complete,
        ),
        None => SessionDriver::new(&config, client, tools, approval.clone(), turn_complete),
    };
    let actor = SessionActor::new(
        session_id,
        event_bus,
        driver,
        approval,
        store,
        config_apply,
        applied_generation,
    );
    local.block_on(&runtime, actor.run(commands));
}

/// Handle to one session's actor thread.
struct SessionHandle {
    /// Entry point for the session's actor commands.
    commands: mpsc::UnboundedSender<SessionCommand>,
    /// The actor thread, joined when the session is deleted or the manager drops.
    thread: thread::JoinHandle<()>,
}

/// Owns the per-session driver actors and routes commands to them by id.
///
/// Without an LLM client no actors are spawned; run-related routing then reports
/// a backend error, mirroring the engine's clientless behaviour.
pub(crate) struct SessionManager {
    client: Option<Arc<dyn LlmClient>>,
    tools: Arc<ToolRegistry>,
    event_bus: EventBus,
    store: Arc<Persistence>,
    config_apply: Option<ConfigApplyState>,
    turn_complete: TurnCompleteHub,
    handles: Mutex<HashMap<SessionId, SessionHandle>>,
}

impl SessionManager {
    /// Creates a manager bound to `event_bus`, spawning actors only when a
    /// `client` is present. Each spawned session assembles its agent with the
    /// shared `tools` registry (`docs/DESIGN.md` §3.2) and persists its committed
    /// snapshots to `store` (`docs/DESIGN.md` §3.6). `config_apply` (when the
    /// engine has a configuration backend) lets actors land `apply_config` at
    /// turn boundaries (`docs/CLI.md` §4.4), and `turn_complete` is the
    /// engine-wide hook notified after every run terminal (`docs/CLI.md` §4.5).
    pub(crate) fn new(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        event_bus: EventBus,
        store: Arc<Persistence>,
        config_apply: Option<ConfigApplyState>,
        turn_complete: TurnCompleteHub,
    ) -> Self {
        Self {
            client,
            tools,
            event_bus,
            store,
            config_apply,
            turn_complete,
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Spawns and registers the actor thread for a newly created session.
    ///
    /// A no-op when the manager has no client. The command sender is registered
    /// synchronously, so a later `send_message` never races the actor thread.
    pub(crate) fn create_session(&self, session_id: SessionId, config: SessionConfig) {
        self.spawn_session(session_id, config, None);
    }

    /// Spawns the actor thread for a persisted session, rebuilding its agent from
    /// `snapshot` when one was captured (`docs/DESIGN.md` §3.6).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Backend`] when the manager has no client and so
    /// cannot host a session actor.
    pub(crate) fn resume_session(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        snapshot: Option<AgentSnapshot>,
    ) -> Result<(), ServiceError> {
        if self.client.is_none() {
            return Err(ServiceError::Backend {
                message: "no LLM client configured".to_owned(),
            });
        }
        self.spawn_session(session_id, config, snapshot);
        Ok(())
    }

    /// Spawns and registers one session actor thread, fresh or restored.
    ///
    /// A no-op when the manager has no client, mirroring the engine's clientless
    /// behaviour.
    fn spawn_session(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        restore: Option<AgentSnapshot>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let tools = self.tools.clone();
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let event_bus = self.event_bus.clone();
        let store = self.store.clone();
        let config_apply = self.config_apply.clone();
        let turn_complete = self.turn_complete.clone();
        // Read the apply generation synchronously *before* the handle is
        // registered: a bump racing the spawn either lands before this read
        // (the new session counts as up-to-date and never sees the command)
        // or after it (the `ApplyConfig` command carries the newer
        // generation and the actor applies it despite the stale baseline).
        let applied_generation = config_apply.as_ref().map_or(0, ConfigApplyState::pending);
        let thread = thread::Builder::new()
            .name(format!("mag-session-{session_id}"))
            .spawn(move || {
                session_thread(
                    session_id,
                    config,
                    client,
                    tools,
                    event_bus,
                    store,
                    restore,
                    config_apply,
                    applied_generation,
                    turn_complete,
                    commands_rx,
                )
            })
            .expect("spawn mag session thread");
        lock_recovering(&self.handles).insert(
            session_id,
            SessionHandle {
                commands: commands_tx,
                thread,
            },
        );
    }

    /// Asks every live session actor to apply the current configuration
    /// snapshot at its next opportunity (`docs/CLI.md` §4.4).
    ///
    /// The engine bumps the shared generation before calling this and hands
    /// the new generation over; an idle actor applies immediately when it
    /// services the command, while an actor with a run in flight applies at
    /// the run's turn boundary instead (reconfiguration mid-turn is
    /// impossible on agent-lib's admission rules). Actors that already
    /// applied this generation no-op. A dead actor's send failure is
    /// ignored — the session is being torn down.
    pub(crate) fn apply_config(&self, generation: u64) {
        for sender in lock_recovering(&self.handles).values() {
            let _ = sender
                .commands
                .send(SessionCommand::ApplyConfig { generation });
        }
    }

    /// Routes a user message to the session actor and awaits the started run id.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Backend`] when no actor exists (no client) or the
    /// actor stopped before replying.
    pub(crate) async fn send_message(
        &self,
        session_id: SessionId,
        text: String,
    ) -> Result<RunId, ServiceError> {
        let sender = self
            .sender(session_id)
            .ok_or_else(|| ServiceError::Backend {
                message: "no LLM client configured".to_owned(),
            })?;
        let (reply, reply_rx) = oneshot::channel();
        sender
            .send(SessionCommand::SendMessage { text, reply })
            .map_err(|_| ServiceError::Backend {
                message: "session actor stopped".to_owned(),
            })?;
        // Drop this cloned sender before parking on the reply so a deferred run
        // never keeps the command channel open and blocks `delete_session`'s join.
        drop(sender);
        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "session actor dropped the run before replying".to_owned(),
        })?
    }

    /// Routes a cancel to the session actor; a no-op when no actor exists.
    pub(crate) fn cancel(&self, session_id: SessionId) {
        if let Some(sender) = self.sender(session_id) {
            let _ = sender.send(SessionCommand::CancelRun);
        }
    }

    /// Routes a pivot message to the session actor and awaits queueing
    /// (`docs/CLI.md` §3.2).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotPivotable`] when no actor exists (a
    /// clientless engine never has an in-progress run) or the session's run
    /// already ended, and [`ServiceError::Backend`] when the actor stopped
    /// before replying.
    pub(crate) async fn pivot_message(
        &self,
        session_id: SessionId,
        text: String,
    ) -> Result<(), ServiceError> {
        let Some(sender) = self.sender(session_id) else {
            // No actor means no in-progress run, so this is an honest
            // `NotPivotable` rather than a backend failure.
            return Err(ServiceError::NotPivotable {
                id: session_id,
                reason: "no in-progress run".to_owned(),
            });
        };
        let (reply, reply_rx) = oneshot::channel();
        sender
            .send(SessionCommand::PivotMessage { text, reply })
            .map_err(|_| ServiceError::Backend {
                message: "session actor stopped".to_owned(),
            })?;
        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "session actor dropped the pivot before replying".to_owned(),
        })?
    }

    /// Routes an interaction response to the session actor, which resolves the
    /// matching pending approval (`docs/DESIGN.md` §3.3).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InteractionNotFound`] when no actor exists for the
    /// session or the request id is unknown, or [`ServiceError::Backend`] when the
    /// actor stopped before replying.
    pub(crate) async fn respond_interaction(
        &self,
        session_id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        let Some(sender) = self.sender(session_id) else {
            return Err(ServiceError::InteractionNotFound { request_id });
        };
        let (reply, reply_rx) = oneshot::channel();
        sender
            .send(SessionCommand::RespondInteraction {
                request_id,
                response,
                reply,
            })
            .map_err(|_| ServiceError::Backend {
                message: "session actor stopped".to_owned(),
            })?;
        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "session actor dropped the response before replying".to_owned(),
        })?
    }

    /// Stops a session's actor thread and forgets it.
    pub(crate) fn delete_session(&self, session_id: SessionId) {
        let handle = lock_recovering(&self.handles).remove(&session_id);
        if let Some(handle) = handle {
            // Dropping the sender closes the command channel so the actor loop
            // ends; joining reaps the thread (any in-flight run is abandoned).
            drop(handle.commands);
            let _ = handle.thread.join();
        }
    }

    /// Clones the command sender for `session_id`, if an actor exists.
    fn sender(&self, session_id: SessionId) -> Option<mpsc::UnboundedSender<SessionCommand>> {
        lock_recovering(&self.handles)
            .get(&session_id)
            .map(|handle| handle.commands.clone())
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        let handles = std::mem::take(&mut *lock_recovering(&self.handles));
        for (_, handle) in handles {
            drop(handle.commands);
            let _ = handle.thread.join();
        }
    }
}
